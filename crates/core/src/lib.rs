//! gravity-core — the turn executor (ADR-0006/0008's stateless loop).
//!
//! One turn = one execution: fetch the session projection (krystallizer)
//! → run the LLM + tool-call loop → append the delta. The loop holds NO
//! state between turns; persistence and assembly sink into the memory
//! system (ADR-0006's session-as-data ruling).
//!
//! Phase 0 shape: plain chat, no tools. The LLM call goes directly to
//! an OpenAI-compatible endpoint via reqwest — the provider-as-booth
//! split (ADR-0034's `ctx.iterate` form) lands in Phase 1; the in-process
//! call here is Milestone A's sanctioned bring-up path. The turn
//! FUNCTION is the stable part: CLI (Phase 0), the aura booth binding
//! (Milestone B), and the FaaS driver all wrap this same function.

pub mod llm;
pub mod surface;
pub mod tool;
pub mod turn;

/// One turn's input: the user message plus the addressing the memory
/// system needs. `channel_id` addresses the conversation (the channel
/// log); `member_id` addresses the agent's projection inside it.
#[derive(Clone, Debug)]
pub struct TurnInput {
    pub channel_id: u64,
    pub member_id: u64,
    pub user_message: String,
}

/// One turn's output: the assistant reply (Phase 0 — no tool calls, no
/// stream; the envelope grows with Phase 2's tool loop). Serializable:
/// the booth binding returns it as the handler's value.
#[derive(Clone, Debug, serde::Serialize)]
pub struct TurnOutput {
    pub reply: String,
}

/// Timestamps: gravity's clock IS the channel's time (ADR-0008 §3 —
/// a reply provably postdates what it answers because the replier saw
/// it; a shared seq has no such property). Milliseconds from the Unix
/// epoch; the `sub` tiebreaker stays 0 in Phase 0 (one message per
/// write batch — the loop appends exactly one user + one assistant
/// message per turn, and the LLM call between them makes same-ms
/// collision impossible).
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        // A clock before 1970 reads as 0, not as a crash: the value is a
        // timestamp, and a timer that cannot tell the time has no better
        // answer than "the epoch".
        .unwrap_or_default()
        .as_millis() as u64
}
