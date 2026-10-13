//! The turn function (ADR-0006's stateless loop, ADR-0008's projection
//! read): fetch the session view → assemble the prompt → one LLM call →
//! append the delta. Everything stateless between turns; the memory
//! system owns assembly and persistence.
//!
//! The function is the stable contract: `turn(&store, &client, input)`
//! takes the memory surface and the LLM surface as ARGUMENTS, so the
//! CLI drives it in-process now and the aura booth binding (Milestone
//! B) drives it with the same signature later — the drivers change, the
//! function does not.

use crate::llm::{LlmClient, WireMessage};
use crate::surface::MemorySurface;
use crate::{now_ms, TurnInput, TurnOutput};
use mem_core::channel::{kind, role};

/// Run one turn. `store` is the caller-opened memory surface (in-process
/// or HTTP — the seam is `MemorySurfaceObj`, ADR-0007 as revised). The
/// delta append happens ONLY on success — a failed LLM call appends
/// nothing (the user message is re-sent next turn; the failed attempt
/// never enters the log).
///
/// Same-turn timestamping: the user message and the assistant reply
/// share one `ts_ms` with distinct `sub` (0 and 1) — they are one
/// exchange, and the LLM call between the two appends cannot collide
/// because both stamps come from the same `now_ms()` read.
pub async fn turn(
    store: &dyn MemorySurface,
    client: &LlmClient,
    input: TurnInput,
) -> anyhow::Result<TurnOutput> {
    let channel_id = input.channel_id;
    let member_id = input.member_id;

    // 1. Fetch the session projection (checkpoint + increment).
    let view = store.fetch_session(channel_id, member_id).await?;

    // 2. Assemble the prompt: summary (if any) as a system preamble,
    // then the increment as the conversation, then the user message.
    // Prompt-view byte stability is Phase 1's contract test — Phase 0
    // just builds a sane list.
    let mut messages: Vec<WireMessage> = Vec::new();
    if let Some(summary) = &view.summary {
        messages.push(WireMessage {
            role: "system".into(),
            content: format!("Conversation summary so far:\n{summary}"),
        });
    }
    for (_, row) in &view.increment {
        messages.push(WireMessage {
            role: wire_role(row.role).to_string(),
            content: row.content.clone(),
        });
    }
    messages.push(WireMessage {
        role: "user".into(),
        content: input.user_message.clone(),
    });

    // 3. The LLM call. On failure: append nothing (see fn doc).
    let reply = client.complete(&messages).await?;

    // 4. Append the delta: one exchange, one timestamp, sub 0/1.
    let ts = now_ms();
    store
        .append_message(
            channel_id,
            ts,
            0,
            role::USER,
            kind::TEXT,
            &input.user_message,
        )
        .await?;
    store
        .append_message(channel_id, ts, 1, role::ASSISTANT, kind::TEXT, &reply)
        .await?;

    Ok(TurnOutput { reply })
}

/// Storage role byte → wire role string. The mapping lives here, at the
/// seam where krystallizer's storage currency meets the LLM wire — the
/// storage layer stores the byte, this function interprets it.
fn wire_role(role: u8) -> &'static str {
    match role {
        mem_core::channel::role::ASSISTANT => "assistant",
        mem_core::channel::role::TOOL => "tool",
        mem_core::channel::role::SYSTEM => "system",
        _ => "user",
    }
}
