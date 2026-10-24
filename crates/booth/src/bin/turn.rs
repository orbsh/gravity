//! The turn guest — an induction process: one turn execution per
//! activation, streaming end to end.
//!
//! `turn(args)` is an iterate PRODUCER whose stream IS the turn:
//!
//! 1. `GET {store_url}/channels/{c}/members/{m}/session?format=` — the
//!    projection (head + summary + increment), the mem-server wire shape.
//!    The head is TEXT: k10r renders the initialization memory, gravity
//!    never assembles the graph (ADR-0010 §5).
//! 2. Prompt assembly: head block (system) + summary preamble (system) +
//!    increment + the user message.
//! 3. `ctx.iterate_start(llm_provider, "", "stream", provider_args)` —
//!    each envelope item's `delta` is re-emitted as this stream's own
//!    item (the stream IS the turn; nothing buffers).
//! 4. On the provider's `done`: append user + assistant with one
//!    shared `ts_ms`, subs 0/1 — ONLY after a successful stream (a
//!    failed turn appends nothing; the user re-sends next turn).
//!    Gravity's clock IS the channel's time (krystallizer ADR-0008 §3).
//!
//! A provider failure fails the round (the pull error value) — the
//! mandatory dispose runs in the round-0 error path, so abandonment is
//! governed, not leaked.
//!
//! Args: `{"channel_id", "member_id", "user_message", "store_url",
//! "endpoint", "model", "key_env"}`.

use anyhow::Context;
use gravity_core::surface::MemoryFormat;
use induction_guest::{Ctx, Guest};
use serde_json::{json, Value};

fn main() -> anyhow::Result<()> {
    Guest::new()
        .named("gravity-turn")
        // The per-handler auth declaration (prism's gate reads it): the
        // turn needs a bound user — the SAME declaration face the echo
        // plane enforces, no second ACL table.
        .auth(serde_json::json!({ "turn": true }))
        // The Start handler: `turn` is the name the realm's iterate job
        // addresses (the handler the submit names). It runs round 0 and
        // its reply opens the stream (a plain `done:false` envelope).
        .on("turn", |ctx: &mut Ctx, args: Value| {
            start_envelope(ctx, &args)
        })
        // The pull handler: `__next__` is the reserved binding the guest
        // guard table routes every pull round to (`{stream_id, round}`
        // in, an envelope out).
        .producer(|ctx: &mut Ctx, args: Value| turn_round(ctx, args))
        .run()
}

/// The provider stream id once opened (a turn opens exactly one;
/// process state — the process IS the turn).
static PROVIDER_STREAM: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

fn turn_round(ctx: &mut Ctx, args: Value) -> anyhow::Result<Value> {
    let round = args.get("round").and_then(Value::as_u64).unwrap_or(0);
    if round == 0 {
        anyhow::bail!("turn: pull round 0 is the Start handler's job");
    }
    // Tool results from the Start-face dispatch loop surface first (the
    // consumer sees the loop's progress before the content deltas).
    {
        let mut results = TOOL_RESULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !results.is_empty() {
            return Ok(json!({ "done": false, "item": results.remove(0) }));
        }
    }
    // Rounds ≥ 1: pull the provider, re-emit the delta.
    let guard = PROVIDER_STREAM.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(sid) = guard.as_ref() else {
        anyhow::bail!("turn: pull with no provider stream");
    };
    let env = ctx
        .iterate_next(sid)
        .context("turn: provider pull failed")?;
    if env.get("done").and_then(Value::as_bool) == Some(true) {
        // The provider finished: dispose the provider stream, append
        // the exchange, close the turn. The turn args live in process
        // state (captured at Start) — the pull handler's args are the
        // guard's `{stream_id, round}`, not the turn's.
        let _ = ctx.iterate_dispose(sid);
        drop(guard);
        let reply = finish_turn()?;
        *PROVIDER_STREAM.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        return Ok(reply);
    }
    let Some(item) = env.get("item") else {
        anyhow::bail!("turn: non-terminal envelope carried no item");
    };
    let delta = item
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    REPLY_PARTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(delta.clone());
    Ok(json!({ "done": false, "item": { "delta": delta } }))
}

/// The Start handler (`turn`): fetch the projection, assemble the
/// prompt, open the provider stream, and reply the FIRST envelope
/// (`{done:false, item:{delta}}` — the stream opens, round 1 pulls on).
fn start_envelope(ctx: &mut Ctx, args: &Value) -> anyhow::Result<Value> {
    // Capture the turn's own args for the finish path (the pull
    // handler's args are the guard's `{stream_id, round}`).
    *TURN_ARGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(args.clone());
    let (first_delta, _sid) = start_turn(ctx, args)?;
    Ok(json!({ "done": false, "item": { "delta": first_delta } }))
}

/// Round 0: fetch the projection, assemble the prompt, open the
/// provider stream, return (first delta, envelope-so-far).
fn start_turn(ctx: &mut Ctx, args: &Value) -> anyhow::Result<(String, String)> {
    let store_url = args
        .get("store_url")
        .and_then(Value::as_str)
        .context("turn: missing `store_url`")?
        .trim_end_matches('/')
        .to_string();
    let channel_id = args
        .get("channel_id")
        .and_then(Value::as_u64)
        .context("turn: missing `channel_id`")?;
    let member_id = args
        .get("member_id")
        .and_then(Value::as_u64)
        .context("turn: missing `member_id`")?;
    let user_message = args
        .get("user_message")
        .and_then(Value::as_str)
        .context("turn: missing `user_message`")?
        .to_string();

    // 1. The session projection (mem-server's SessionDto shape). The head's
    // representation is the deployment's choice, and `struct` is refused
    // here: it returns the fact DATA, which this executor does not assemble.
    let format = MemoryFormat::from_config(args.get("memory_format").and_then(Value::as_str))
        .context("turn: the configured memory format is unusable")?;
    let url = format!(
        "{store_url}/channels/{channel_id}/members/{member_id}/session?format={}",
        format.name()
    );
    let resp = ureq::get(&url)
        .call()
        .context("turn: mem-server session request failed")?;
    let view: Value =
        serde_json::from_reader(resp.into_reader()).context("turn: bad session projection")?;

    // 2. Prompt assembly. The head arrives RENDERED (k10r's own text — no
    // wrapper of ours), the summary is the compression point, and every
    // increment entry carries the LLM role name k10r resolved.
    let mut messages: Vec<Value> = Vec::new();
    if let Some(head) = view.get("head").and_then(Value::as_str) {
        if !head.is_empty() {
            messages.push(json!({ "role": "system", "content": head }));
        }
    }
    if let Some(summary) = view.get("summary").and_then(Value::as_str) {
        if !summary.is_empty() {
            messages.push(json!({
                "role": "system",
                "content": format!("Conversation summary so far:\n{summary}"),
            }));
        }
    }
    for entry in view
        .get("increment")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
    {
        // The entry is FLAT: `role` is the resolved NAME (null for a code
        // k10r does not police → the user's turn, the pre-existing fallback).
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("user");
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        messages.push(json!({ "role": role, "content": content }));
    }
    messages.push(json!({ "role": "user", "content": user_message }));

    // 3. Open the provider stream; its first reply carries the first
    // envelope (with the realm-minted stream_id merged in). A
    // tool_calls-first reply = the LLM wants tools BEFORE any prose:
    // dispatch them, append the results to the message list, and
    // reopen — the "next inference round" of the loop (Phase 2). The
    // results ALSO surface as this stream's own items (the consumer
    // sees the loop's progress), re-emitted by the pull face.
    let mut first = ctx
        .iterate_start(
            "llm_provider",
            "",
            "stream",
            json!({
                "endpoint": args.get("endpoint"),
                "model": args.get("model"),
                "key_env": args.get("key_env"),
                "messages": messages,
            }),
        )
        .context("turn: provider stream failed to open")?;
    for _round in 0..8 {
        // A string reply = the provider handler's error value surfaced
        // through the host bridge (errors cross as values, ADR-0012).
        if let Some(msg) = first.as_str() {
            anyhow::bail!("turn: provider stream failed to open: {msg}");
        }
        if first.get("done").and_then(Value::as_bool) == Some(true) {
            anyhow::bail!(
                "turn: provider stream failed to open: {}",
                first
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("no tokens")
            );
        }
        let Some(calls) = first.get("item").and_then(|i| i.get("tool_calls")) else {
            break; // a content stream — the pull face takes over
        };
        // Dispatch every call; feed the messages back; surface each
        // result (the consumer's tool_result items ride the loop rounds
        // through TOOL_RESULTS).
        let parsed = gravity_core::tool::parse_tool_calls(&json!({ "tool_calls": calls }))
            .context("turn: malformed tool calls from the provider")?;
        let target_type = args
            .get("tool_target")
            .and_then(Value::as_str)
            .unwrap_or("effector")
            .to_string();
        for call in parsed {
            let result = ctx.invoke(
                &target_type,
                "",
                &call.function.name,
                call.function.arguments.clone(),
            );
            let content = match result {
                Ok(v) => v.to_string(),
                Err(e) => json!({ "error": format!("{e:#}") }).to_string(),
            };
            let entry = json!({
                "tool_result": { "tool_call_id": call.id, "content": content }
            });
            TOOL_RESULTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(entry);
            // The fed-back wire shape: the assistant's call + the tool's
            // result (OpenAI's loop contract).
            messages.push(json!({ "role": "assistant", "tool_calls": calls }));
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": content,
            }));
        }
        // Reopen the provider with the fed-back messages (the next
        // inference round); loop until it emits content.
        first = ctx
            .iterate_start(
                "llm_provider",
                "",
                "stream",
                json!({
                    "endpoint": args.get("endpoint"),
                    "model": args.get("model"),
                    "key_env": args.get("key_env"),
                    "messages": messages,
                }),
            )
            .context("turn: provider re-open failed")?;
    }
    // The loop must have ENDED on a content stream: the cap is the
    // runaway stop, and a provider still asking for tools after it is a
    // failed turn — never an empty first token.
    anyhow::ensure!(
        first
            .get("item")
            .and_then(|i| i.get("tool_calls"))
            .is_none(),
        "turn: the tool loop did not converge in 8 rounds — the provider kept requesting tools"
    );
    let sid = first
        .get("stream_id")
        .and_then(Value::as_str)
        .context("turn: provider Start reply carried no stream_id")?
        .to_string();

    let delta = first
        .get("item")
        .and_then(|i| i.get("delta"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    *PROVIDER_STREAM.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sid.clone());

    // Remember the reply text accumulated so far.
    REPLY_PARTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(delta.clone());
    Ok((delta, sid))
}

/// The accumulated reply text (process state — the turn IS the stream).
static REPLY_PARTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The turn's own args, captured at Start (the pull handler's args are
/// the guard's `{stream_id, round}` — the addressing rides state).
static TURN_ARGS: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// Tool results from the Start-face dispatch loop (the consumer sees
/// the loop's progress — each surfaces as an envelope item before the
/// content deltas).
static TOOL_RESULTS: std::sync::Mutex<Vec<Value>> = std::sync::Mutex::new(Vec::new());

/// The terminal round: append the one-ts-two-subs exchange, return the
/// terminal envelope (the full reply as the value).
fn finish_turn() -> anyhow::Result<Value> {
    let args = TURN_ARGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .ok_or_else(|| anyhow::anyhow!("turn: finish with no captured args"))?;
    let store_url = args
        .get("store_url")
        .and_then(Value::as_str)
        .context("turn: missing `store_url`")?
        .trim_end_matches('/')
        .to_string();
    let channel_id = args
        .get("channel_id")
        .and_then(Value::as_u64)
        .context("turn: missing `channel_id`")?;
    let user_message = args
        .get("user_message")
        .and_then(Value::as_str)
        .context("turn: missing `user_message`")?
        .to_string();

    let reply_parts: Vec<String> = std::mem::take(&mut *REPLY_PARTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
    let reply = reply_parts.concat();
    if reply.is_empty() {
        anyhow::bail!("turn: provider stream yielded no tokens");
    }

    // Append the exchange: one ts, subs 0/1 — only on success. Gravity's
    // clock IS the channel's time (krystallizer ADR-0008 §3).
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .context("turn: clock before epoch")?;
    for (sub, role, content) in [(0u16, 0u8, user_message), (1, 1, reply.clone())] {
        let url = format!("{store_url}/channels/{channel_id}/messages");
        let resp = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(
                &json!({ "ts_ms": ts, "sub": sub, "role": role, "kind": 0, "content": content })
                    .to_string(),
            )
            .context("turn: append failed")?;
        anyhow::ensure!(
            (200..300).contains(&resp.status()),
            "turn: append returned {}",
            resp.status()
        );
    }
    Ok(json!({ "done": true, "value": { "reply": reply } }))
}
