//! gravity-booth — the aura booth binding (PLAN Phase 4's first
//! landing): the turn execution registered as a Booth type.
//!
//! Two booths, one seam:
//!
//! - `provider_booth()` — the LLM provider as a SEPARATE booth type
//!   (aura ADR-0034; ADR-0031's ruling applied: outbound access lives
//!   in a booth's own code). Python, `httpx.stream` against any
//!   OpenAI-compatible endpoint, SSE parsed into a generator, `yield`
//!   per token event. Key custody via env injection (the env var NAME
//!   rides the args, the key itself never in the module). Transport
//!   retry/backoff lives here.
//! - `turn_booth()` — the turn function as a Rust closure booth
//!   (`Body::Rust`): fetch the session view → assemble the prompt →
//!   `ctx.iterate(provider, ...)` pulling tokens → append the delta →
//!   return the reply. The closure captures the caller-opened
//!   `MemorySurface` + `LlmClient` (compile-time constructs, not
//!   persistable — aura's boot reload skips Rust bodies, so the host
//!   registers this booth explicitly on every start; that is the
//!   deployment shape, not a gap).
//!
//! Streaming note (the structural residual): a Rust booth body carries
//! no stream state (aura instance.rs, ADR-0034 residual) — the turn
//! booth is an INVOKE: it consumes the provider's token stream
//! internally and answers once. The per-token delta stream to the
//! connection plane (prism `turn.delta`) needs the producer to be a
//! session-backed booth; that is the next work unit, recorded in the
//! PLAN, not silently missing here.

use gravity_core::llm::{LlmClient, WireMessage};
use gravity_core::surface::MemorySurface;
use gravity_core::{TurnInput, TurnOutput, now_ms};
use mem_core::channel::{kind, role};
use std::sync::Arc;

/// The provider booth's type name (the `ctx.iterate` target).
pub const PROVIDER_TYPE: &str = "llm_provider";

/// The turn booth's type name.
pub const TURN_TYPE: &str = "gravity_turn";

/// The handler the turn booth invokes on the provider.
pub const PROVIDER_HANDLER: &str = "stream";

/// The provider booth source: `stream(args)` yields
/// `{"delta": "..."}` per token event and returns the terminal value
/// `{"done": true}`. SSE parsing is inline (httpx's line iterator —
/// no extra dependency); errors raise (the pull fails as a value).
///
/// Args: `{"endpoint", "model", "key_env", "messages": [{role,
/// content}, ...]}`. The key is read from the environment PER CALL —
/// the module body never sees it, rotation takes effect without a
/// reload, and the args can be logged without leaking.
pub const PROVIDER_SOURCE: &str = r#"
import httpx, json, os

def stream(args):
    key = os.environ.get(args["key_env"], "")
    url = args["endpoint"].rstrip("/") + "/chat/completions"
    headers = {"Authorization": f"Bearer {key}"}
    body = {"model": args["model"], "messages": args["messages"], "stream": True}
    with httpx.stream("POST", url, headers=headers, json=body, timeout=120) as r:
        r.raise_for_status()
        for line in r.iter_lines():
            if not line.startswith("data: "):
                continue
            data = line[6:]
            if data.strip() == "[DONE]":
                break
            chunk = json.loads(data)
            delta = chunk["choices"][0].get("delta", {}).get("content")
            if delta:
                yield {"delta": delta}
"#;

/// The provider booth type (python). Registered into the same realm as
/// the turn booth; the turn closure addresses it by name.
pub fn provider_booth() -> aura_booth::BoothType {
    aura_booth::BoothType::script(PROVIDER_TYPE, "python", PROVIDER_SOURCE)
}

/// The turn booth: the gravity turn function wearing a booth body. The
/// closure captures the memory surface and the LLM client — the SAME
/// two arguments `gravity_core::turn::turn` takes, captured instead of
/// passed (booth handlers receive only Ctx + args). One turn event =
/// one execution; the instance key addresses the session (partition =
/// session_id per aura PLAN Phase 6's contract).
///
/// Args: `{"channel_id", "member_id", "user_message"}` — TurnInput's
/// field set verbatim (the wire form of the stable contract).
pub fn turn_booth(
    surface: Arc<dyn MemorySurface>,
    client: LlmClient,
) -> aura_booth::BoothType {
    aura_booth::BoothType {
        name: TURN_TYPE.into(),
        body: aura_booth::Body::Rust(Arc::new(move |ctx, args| {
            let surface = Arc::clone(&surface);
            let client = client.clone();
            Box::pin(async move {
                let channel_id = args
                    .get("channel_id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("channel_id required"))?;
                let member_id = args
                    .get("member_id")
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| anyhow::anyhow!("member_id required"))?;
                let user_message = args
                    .get("user_message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let input = TurnInput { channel_id, member_id, user_message };

                // 1. Fetch the session projection.
                let view = surface.fetch_session(channel_id, member_id).await?;

                // 2. Assemble the prompt (the turn function's shape —
                // summary preamble + increment + user message).
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
                messages.push(WireMessage { role: "user".into(), content: input.user_message.clone() });

                // 3. The LLM call THROUGH the provider booth
                // (ctx.iterate — the native pull loop; the provider is
                // a sibling citizen, not gravity-internal plumbing).
                // The Rust body has no stream state, so the tokens are
                // consumed here and the reply returns once (the
                // streaming residual recorded in the module doc).
                let provider_args = serde_json::json!({
                    "endpoint": client.endpoint(),
                    "model": client.model(),
                    "key_env": client.api_key_env(),
                    "messages": messages,
                });
                let mut cursor = ctx.iterate(
                    aura_booth::InstanceId {
                        booth_type: PROVIDER_TYPE.into(),
                        key: aura_booth::InstanceKey::Singleton,
                    },
                    PROVIDER_HANDLER,
                    provider_args,
                );
                let mut reply = String::new();
                // The stream-state flag: exhaustion (done) releases the
                // stream carrier-side; anything else needs the dispose
                // dual below. The initial value is never read (every
                // loop exit either sets it or bails) — hence the
                // `#[allow]`.
                #[allow(unused_assignments)]
                let mut stream_released = false;
                loop {
                    match cursor.next().await {
                        Ok(env) if env.done => {
                            // Exhaustion — the carrier released the
                            // stream itself.
                            stream_released = true;
                            break;
                        }
                        Ok(env) => {
                            if let Some(delta) = env
                                .item
                                .as_ref()
                                .and_then(|i| i.get("delta"))
                                .and_then(|d| d.as_str())
                            {
                                reply.push_str(delta);
                            }
                        }
                        // A failed pull fails the turn — the oldest
                        // rule (a failed LLM call appends nothing), now
                        // carried by the booth form. Swallowing it
                        // would append an empty exchange.
                        Err(e) => {
                            let _ = cursor.dispose().await;
                            anyhow::bail!("provider pull failed: {e}");
                        }
                    }
                }
                // The mandatory dual (ADR-0034 §3): an abnormal exit
                // leaves the stream live carrier-side. Idempotent — a
                // released stream disposes nothing.
                if !stream_released {
                    let _ = cursor.dispose().await;
                }
                if reply.is_empty() {
                    anyhow::bail!("provider stream yielded no tokens (turn fails through)");
                }

                // 4. Append the delta: one exchange, one timestamp
                // (sub 0/1) — only on success.
                let ts = now_ms();
                surface
                    .append_message(channel_id, ts, 0, role::USER, kind::TEXT, &input.user_message)
                    .await?;
                surface
                    .append_message(channel_id, ts, 1, role::ASSISTANT, kind::TEXT, &reply)
                    .await?;

                let out = TurnOutput { reply };
                Ok(serde_json::to_value(out)?)
            })
        })),
        idle_ttl: None,
        max_exec: None,
        on_sleep: None,
        on_wake: None,
        receives: Vec::new(),
    }
}

/// Storage role byte → wire role string (gravity_core::turn's mapping —
/// duplicated here because the closure cannot call the private fn; one
/// mapping per seam crossing, kept in sync by the contract tests).
fn wire_role(role: u8) -> &'static str {
    match role {
        mem_core::channel::role::ASSISTANT => "assistant",
        mem_core::channel::role::TOOL => "tool",
        mem_core::channel::role::SYSTEM => "system",
        _ => "user",
    }
}
