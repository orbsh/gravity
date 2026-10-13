//! The turn guest — an induction process: one turn execution per
//! activation, streaming end to end.
//!
//! `turn(args)` is an iterate PRODUCER whose stream IS the turn:
//!
//! 1. `GET {store_url}/channels/{c}/members/{m}/session` — the
//!    projection (summary + increment), the mem-server wire shape.
//! 2. Prompt assembly: summary preamble (system) + increment + the
//!    user message.
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
    // Rounds ≥ 1: pull the provider, re-emit the delta.
    let guard = PROVIDER_STREAM.lock().unwrap();
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
        *PROVIDER_STREAM.lock().unwrap() = None;
        return Ok(reply);
    }
    let Some(item) = env.get("item") else {
        anyhow::bail!("turn: non-terminal envelope carried no item");
    };
    // Tool calls from the LLM: parse, dispatch through ctx.invoke (the
    // target rides the call verbatim — the tool list is the caller's),
    // and surface each result as this stream's own envelope item. The
    // transcript accounting is Phase 2.5's split — the loop keeps the
    // wire flowing; the NEXT provider stream (opened by the finish
    // path's next-turn cycle) carries the fed-back messages.
    if let Some(calls) = item.get("tool_calls") {
        let parsed = gravity_core::tool::parse_tool_calls(&json!({ "tool_calls": calls }))
            .context("turn: malformed tool calls from the provider")?;
        let mut out = Vec::new();
        for call in parsed {
            let result = ctx.invoke(
                "effector",
                "",
                &call.function.name,
                json!({
                    "target": format!("effector:local:{}", call.function.name),
                    "args": call.function.arguments,
                }),
            );
            let content = match result {
                Ok(v) => v.to_string(),
                // Failure is a value into the next inference (the tool
                // failed, not the turn — the loop continues).
                Err(e) => json!({ "error": format!("{e:#}") }).to_string(),
            };
            out.push(json!({
                "tool_result": {
                    "tool_call_id": call.id,
                    "content": content,
                }
            }));
        }
        return Ok(json!({ "done": false, "item": out[0] }));
    }
    let delta = item
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    REPLY_PARTS.lock().unwrap().push(delta.clone());
    Ok(json!({ "done": false, "item": { "delta": delta } }))
}

/// The Start handler (`turn`): fetch the projection, assemble the
/// prompt, open the provider stream, and reply the FIRST envelope
/// (`{done:false, item:{delta}}` — the stream opens, round 1 pulls on).
fn start_envelope(ctx: &mut Ctx, args: &Value) -> anyhow::Result<Value> {
    // Capture the turn's own args for the finish path (the pull
    // handler's args are the guard's `{stream_id, round}`).
    *TURN_ARGS.lock().unwrap() = Some(args.clone());
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

    // 1. The session projection (mem-server's SessionDto shape).
    let url = format!("{store_url}/channels/{channel_id}/members/{member_id}/session");
    let resp = ureq::get(&url)
        .call()
        .context("turn: mem-server session request failed")?;
    let view: Value =
        serde_json::from_reader(resp.into_reader()).context("turn: bad session projection")?;

    // 2. Prompt assembly. Storage role byte → wire role (mem-core's
    // channel constants: 1=assistant, 2=tool, 3=system, else user).
    let mut messages: Vec<Value> = Vec::new();
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
        let role = entry
            .get("row")
            .and_then(|r| r.get("role"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let content = entry
            .get("row")
            .and_then(|r| r.get("content"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        messages.push(json!({ "role": wire_role(role), "content": content }));
    }
    messages.push(json!({ "role": "user", "content": user_message }));

    // 3. Open the provider stream; its first reply carries the first
    // envelope (with the realm-minted stream_id merged in).
    let provider_args = json!({
        "endpoint": args.get("endpoint"),
        "model": args.get("model"),
        "key_env": args.get("key_env"),
        "messages": messages,
    });
    let first = ctx
        .iterate_start("llm_provider", "", "stream", provider_args)
        .context("turn: provider stream failed to open")?;
    // A string reply = the provider handler's error value surfaced through
    // the host bridge (errors cross as values, ADR-0012).
    if let Some(msg) = first.as_str() {
        anyhow::bail!("turn: provider stream failed to open: {msg}");
    }
    if first.get("done").and_then(Value::as_bool) == Some(true) {
        // Terminal-first = the provider failed to open (its error rides
        // the terminal value) or yielded nothing.
        let msg = first
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or("no tokens");
        anyhow::bail!("turn: provider stream failed to open: {msg}");
    }
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
    *PROVIDER_STREAM.lock().unwrap() = Some(sid.clone());

    // Remember the reply text accumulated so far.
    REPLY_PARTS.lock().unwrap().push(delta.clone());
    Ok((delta, sid))
}

/// The accumulated reply text (process state — the turn IS the stream).
static REPLY_PARTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The turn's own args, captured at Start (the pull handler's args are
/// the guard's `{stream_id, round}` — the addressing rides state).
static TURN_ARGS: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// The terminal round: append the one-ts-two-subs exchange, return the
/// terminal envelope (the full reply as the value).
fn finish_turn() -> anyhow::Result<Value> {
    let args = TURN_ARGS
        .lock()
        .unwrap()
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

    let reply_parts: Vec<String> = std::mem::take(&mut *REPLY_PARTS.lock().unwrap());
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

fn wire_role(role: u64) -> &'static str {
    match role {
        1 => "assistant",
        2 => "tool",
        3 => "system",
        _ => "user",
    }
}
