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
use gravity_booth::tools::Doors;
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
    // The side branch advances one pull per round, on the same face: the
    // summary is produced alongside the answer, never in front of it.
    step_compression(ctx);
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
        // The answer has streamed; the summary's tail is the only thing
        // still owed (see drain_compression).
        drain_compression(ctx);
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
    // A resident guest serves turn after turn: nothing from the previous
    // one may leak into this one (the branch slot is per turn).
    *COMPRESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
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
    // The prefix the compression side branch summarizes: head + summary +
    // history, WITHOUT this turn's new message (the span k10r proposed to
    // cover ends before it).
    let prefix_len = messages.len();
    messages.push(json!({ "role": "user", "content": user_message }));

    // 3. The compression side branch (Phase 3): k10r's verdict rode the
    // projection, so the summary is produced HERE, concurrently with the
    // answer and from the same prefix — the caller never states a coverage
    // point; k10r derives it from the verdict it served (ADR-0008 §4).
    start_compression(ctx, args, &view, &messages[..prefix_len]);

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
                // The fixed list (ADR-0004): the same bytes every turn, and
                // the reason a real model can call anything at all.
                "tools": gravity_booth::tools::tools_array(),
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
        // Dispatch every call, feed the messages back, and re-open the
        // provider — the loop's own rounds. The list is the contract
        // (ADR-0004): every failure is a value the MODEL reads, never a
        // turn failure — a wrong argument must not cost the user their
        // answer. The result goes to the model and to this turn's
        // transcript, not to the stream (prism's face carries `item.delta`
        // and refuses the rest).
        let parsed = gravity_core::tool::parse_tool_calls(&json!({ "tool_calls": calls }))
            .context("turn: malformed tool calls from the provider")?;
        let memory_scope = args.get("memory_scope").and_then(Value::as_u64);
        // The mount table the deployment declares (prism's manifest, carried in
        // the turn args): operation language → the mount type that serves it.
        // Empty is a fact ("this deployment declares no nodes"), absent is not.
        let mounts: Vec<(String, String)> = args
            .get("mounts")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        Some((
                            entry.get("language")?.as_str()?.to_string(),
                            entry.get("type")?.as_str()?.to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // Residency for a mount call is the CONVERSATION (ADR-0001 §6 leaves
        // the key to the caller): a step script's own state survives the turns
        // of one conversation and nothing else.
        let session = format!("channel-{channel_id}-member-{member_id}");
        for call in parsed {
            let facts = gravity_booth::tools::Facts {
                store_url: &store_url,
                memory_scope,
                mounts: &mounts,
                session: &session,
            };
            // The ctx borrow lives only for this call: the provider re-open
            // below needs it back.
            let content = {
                let mut doors = VenueDoors { ctx: &mut *ctx };
                let mut walked = WALKED
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let outcome = gravity_booth::tools::call(
                    &mut doors,
                    &call.function.name,
                    &call.function.arguments,
                    &facts,
                    &mut walked,
                );
                // The recorder is released before the loop builds messages: a
                // later round's call re-takes it, and nothing else touches it.
                drop(walked);
                match outcome {
                    Ok(value) => value.to_string(),
                    Err(error) => json!({ "error": error }).to_string(),
                }
            };
            // The fed-back wire shape: the assistant's call + the tool's
            // result (OpenAI's loop contract). The result is the MODEL's,
            // not the consumer's: prism's turn stream carries text deltas
            // (`item.delta`) and refuses anything else, so the loop's
            // progress stays inside the turn (PLAN Phase 2.5's transcript).
            // A tool PROGRESS frame for a UI is prism's protocol to add,
            // not a frame gravity invents behind its back.
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
                    "tools": gravity_booth::tools::tools_array(),
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

/// The turn's two doors (tools.rs's `Doors` seam): k10r over HTTP — data
/// access, ADR-0007 as revised — and a node mount over the ctx — code
/// execution, ADR-0001. The turn is the only place both exist, which is why
/// the implementation lives here and the list does not.
struct VenueDoors<'c, 'i> {
    ctx: &'c mut Ctx<'i>,
}

impl Doors for VenueDoors<'_, '_> {
    fn post(&mut self, url: &str, body: &Value) -> Result<Value, String> {
        post_json(url, body)
    }

    fn get(&mut self, url: &str) -> Result<Value, String> {
        get_json(url)
    }

    fn mount(&mut self, type_name: &str, key: &str, args: Value) -> Result<Value, String> {
        // `execute` is the entry convention every runner serves (ADR-0001 §1),
        // never a lookup: the mount's source is the runner, the code is data.
        self.ctx
            .invoke(type_name, key, "execute", args)
            .map_err(|e| format!("mount `{type_name}`: {e}"))
    }
}

/// One POST to k10r, with the door's answer as the tool result and its refusal
/// as the error text (a 4xx is the caller's mistake to read, not a turn
/// failure).
fn post_json(url: &str, body: &Value) -> Result<Value, String> {
    match ureq::post(url).send_json(body.clone()) {
        Ok(resp) => resp
            .into_json::<Value>()
            .map_err(|e| format!("{url}: unreadable answer: {e}")),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("{url}: HTTP {code}: {text}"))
        }
        Err(e) => Err(format!("{url}: {e}")),
    }
}

/// One GET from k10r — the skill row and the code row a step names.
fn get_json(url: &str) -> Result<Value, String> {
    match ureq::get(url).call() {
        Ok(resp) => resp
            .into_json::<Value>()
            .map_err(|e| format!("{url}: unreadable answer: {e}")),
        Err(ureq::Error::Status(code, resp)) => {
            let text = resp.into_string().unwrap_or_default();
            Err(format!("{url}: HTTP {code}: {text}"))
        }
        Err(e) => Err(format!("{url}: {e}")),
    }
}

/// The compression side branch's provider instance key. A provider guest
/// holds ONE SSE connection per process, and the realm keys instances by
/// (booth type, key), so the side branch takes its own instance — sharing
/// the main branch's would make the two streams fight over that one slot.
const SIDE_KEY: &str = "compress";

/// The side branch while it is live: its provider stream and the tokens
/// received so far (process state — the process IS the turn).
struct Compression {
    stream_id: String,
    parts: Vec<String>,
}

static COMPRESSION: std::sync::Mutex<Option<Compression>> = std::sync::Mutex::new(None);

/// Open the compression side branch from k10r's verdict (ADR-0008 §4): the
/// same prefix the main branch reads, the served tail prompt in place of
/// this turn's message. It runs CONCURRENTLY with the answer — the provider
/// pulls are interleaved by the pull face — and every failure is swallowed:
/// compression is maintenance, and a broken side branch must never fail a
/// turn. An unlanded summary leaves k10r's node open, which is its
/// "compressing" marker; the next read skips it.
fn start_compression(ctx: &mut Ctx, args: &Value, view: &Value, prefix: &[Value]) {
    let Some(prompt) = view
        .get("compress")
        .and_then(|c| c.get("prompt"))
        .and_then(Value::as_str)
    else {
        return;
    };
    let mut messages: Vec<Value> = prefix.to_vec();
    messages.push(json!({ "role": "user", "content": prompt }));
    let first = match ctx.iterate_start(
        "llm_provider",
        SIDE_KEY,
        "stream",
        json!({
            "endpoint": args.get("endpoint"),
            "model": args.get("model"),
            "key_env": args.get("key_env"),
            "messages": messages,
        }),
    ) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("[gravity-turn] compression branch failed to open: {e:#}");
            return;
        }
    };
    // A string reply is the provider's error value crossing the bridge.
    if let Some(err) = first.as_str() {
        eprintln!("[gravity-turn] compression branch failed: {err}");
        return;
    }
    // A branch that closed on its first round carries no stream id (the
    // realm merges one only for a non-terminal reply): nothing to land.
    let Some(sid) = first
        .get("stream_id")
        .and_then(Value::as_str)
        .filter(|_| first.get("done").and_then(Value::as_bool) != Some(true))
    else {
        return;
    };
    let mut parts = Vec::new();
    if let Some(delta) = first
        .get("item")
        .and_then(|item| item.get("delta"))
        .and_then(Value::as_str)
    {
        parts.push(delta.to_string());
    }
    *COMPRESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Compression {
        stream_id: sid.to_string(),
        parts,
    });
}

/// One pull round of the side branch. On `done` the summary goes to k10r's
/// checkpoint door — the summary ALONE: k10r derives the coverage point from
/// the verdict it served, so the caller never states one.
fn step_compression(ctx: &mut Ctx) {
    let Some(mut branch) = COMPRESSION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
    else {
        return;
    };
    let env = match ctx.iterate_next(&branch.stream_id) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("[gravity-turn] compression pull failed: {e:#}");
            return;
        }
    };
    if env.get("done").and_then(Value::as_bool) == Some(true) {
        let _ = ctx.iterate_dispose(&branch.stream_id);
        if let Some(err) = env.get("value").and_then(Value::as_str) {
            eprintln!("[gravity-turn] compression branch ended without a summary: {err}");
            return;
        }
        let summary = branch.parts.concat();
        if summary.trim().is_empty() {
            return;
        }
        land_summary(&summary);
        return;
    }
    if let Some(delta) = env
        .get("item")
        .and_then(|item| item.get("delta"))
        .and_then(Value::as_str)
    {
        branch.parts.push(delta.to_string());
    }
    *COMPRESSION.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(branch);
}

/// Drain the side branch before the turn closes. The carrier releases the
/// turn's stream at `done` and the guest's state goes with it, so a branch
/// still live here is pulled to completion: the summary must land before the
/// next user message (Phase 3), and the answer has already streamed, so the
/// only wait is the summary's own tail. The cap keeps a runaway branch from
/// holding the turn open; what is left unlanded is k10r's open node, which
/// its next read treats as "compressing".
fn drain_compression(ctx: &mut Ctx) {
    for _ in 0..64 {
        let live = COMPRESSION
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_some();
        if !live {
            return;
        }
        step_compression(ctx);
    }
    eprintln!("[gravity-turn] compression branch still live after 64 pulls");
}

/// `POST …/checkpoint { summary }` — the door k10r opened when it served the
/// verdict. 204 is the answer; anything else is REPORTED and the turn goes
/// on (the alternative, failing the turn over maintenance, would trade a
/// user-visible answer for a bookkeeping one).
fn land_summary(summary: &str) {
    let Some(args) = TURN_ARGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    else {
        return;
    };
    let (Some(store_url), Some(channel_id), Some(member_id)) = (
        args.get("store_url").and_then(Value::as_str),
        args.get("channel_id").and_then(Value::as_u64),
        args.get("member_id").and_then(Value::as_u64),
    ) else {
        return;
    };
    let url = format!(
        "{}/channels/{channel_id}/members/{member_id}/checkpoint",
        store_url.trim_end_matches('/')
    );
    let body = json!({ "summary": summary }).to_string();
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body)
    {
        Ok(resp) if (200..300).contains(&resp.status()) => {}
        Ok(resp) => eprintln!("[gravity-turn] checkpoint door answered {}", resp.status()),
        Err(e) => eprintln!("[gravity-turn] checkpoint door failed: {e}"),
    }
}

/// The accumulated reply text (process state — the turn IS the stream).
static REPLY_PARTS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// The turn's own args, captured at Start (the pull handler's args are
/// the guard's `{stream_id, round}` — the addressing rides state).
static TURN_ARGS: std::sync::Mutex<Option<Value>> = std::sync::Mutex::new(None);

/// The code rows this turn walked, in order (ADR-0016). Turn state like the
/// reply parts: this guest is one turn's process.
static WALKED: std::sync::Mutex<Vec<gravity_booth::tools::Walked>> =
    std::sync::Mutex::new(Vec::new());

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
    // The write side of the skill loop (ADR-0016): a turn that walked two
    // DIFFERENT code rows is a composition worth counting. Reported, never
    // promoted — k10r keeps the count, a human decides what it becomes, and the
    // resolver keeps showing only real skills. Best effort: a proposal is an
    // observation ABOUT the turn, so failing to file it must not fail the turn
    // (the reply is already durable at this point).
    propose_composition(&store_url, &args, ts);

    Ok(json!({ "done": true, "value": { "reply": reply } }))
}

/// File this turn's composition, if it had one. `store_url` and the turn args
/// are already in hand at finish time, and the recorder is the turn's own.
fn propose_composition(store_url: &str, args: &Value, ts_ms: u64) {
    let walked: Vec<gravity_booth::tools::Walked> = std::mem::take(
        &mut *WALKED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    if !gravity_booth::tools::is_composition(&walked) {
        return;
    }
    let Some(scope) = args.get("memory_scope").and_then(Value::as_u64) else {
        // No scope, no graph to propose into: the same rule the tools follow —
        // a missing deployment fact is reported where it is needed, never
        // guessed at here.
        return;
    };
    let body = json!({
        "key": gravity_booth::tools::composition_key(&walked),
        "steps": gravity_booth::tools::steps_json(&walked),
        "ts_ms": ts_ms,
    });
    let url = format!("{store_url}/skills/{scope}/propose");
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
    {
        Ok(resp) if (200..300).contains(&resp.status()) => {}
        Ok(resp) => eprintln!("turn: propose returned {}", resp.status()),
        Err(error) => eprintln!("turn: propose failed: {error}"),
    }
}
