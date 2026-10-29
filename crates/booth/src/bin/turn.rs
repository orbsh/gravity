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

/// Where the frames go when the caller asked for the view protocol. THE CALLER DECLARES
/// THEM (gravity does not own a layout, so it cannot know a slot's name): `slots.rack` is
/// the list the turn appends to, `slots.text` the region the answer streams into.
struct View {
    rack: String,
    text: String,
}

/// `protocol` absent or `data` = raw deltas (the CLI and every non-UI consumer). `fluxen` =
/// Accrete frames: the venue forwards them to a renderer that only knows how to draw.
fn view_of(args: &Value) -> Option<View> {
    if args.get("protocol").and_then(Value::as_str) != Some("fluxen") {
        return None;
    }
    let slots = args.get("slots")?;
    Some(View {
        rack: slots.get("rack").and_then(Value::as_str)?.to_string(),
        text: slots.get("text").and_then(Value::as_str)?.to_string(),
    })
}

/// The text region the answer streams into: an empty text node bound to a default value.
fn frame_text_region(text: &str) -> Value {
    json!({
        "action": "set",
        "event": text,
        "data": {"type": "text", "bind": {"value": {"kind": "default", "default": ""}}},
    })
}

/// One bubble in the rack whose text child reads the streaming region — so a patch of the
/// region re-renders the paragraph, not the box (fluxen's walkthrough rule).
fn frame_answer_bubble(rack: &str, text: &str) -> Value {
    json!({
        "action": "append",
        "event": rack,
        "data": {
            "type": "case",
            "attrs": {"class": ["nogrow", "as-stretch", "box", "border", "round", "shadow"]},
            "children": [{
                "type": "text",
                "bind": {"value": {"kind": "source", "source": text}},
            }],
        },
    })
}

/// The user's own line, so the rack shows the question above the answer.
fn frame_ask_bubble(rack: &str, ask: &str) -> Value {
    json!({
        "action": "append",
        "event": rack,
        "data": {
            "type": "case",
            "attrs": {
                "class": ["nogrow", "as-end", "accent", "box", "border", "round", "shadow"],
                "selector": "ask",
            },
            "children": [{
                "type": "text",
                "bind": {"value": {"kind": "default", "default": ask}},
            }],
        },
    })
}

/// One streamed chunk into the region (fluxen's frame vocabulary: patch, append, value).
fn frame_chunk(text: &str, chunk: &str) -> Value {
    json!({
        "action": "patch",
        "event": text,
        "path": "/bind/value/default",
        "op": "append",
        "value": chunk,
    })
}

/// The envelope's item: frames when the caller asked for them, else the raw delta. The
/// first round also sets the stage (question, region, bubble) — one batch, in order.
fn item_for(args: &Value, first: Option<&str>, delta: &str) -> Value {
    match view_of(args) {
        Some(v) => {
            let mut frames: Vec<Value> = Vec::new();
            if let Some(ask) = first {
                frames.push(frame_ask_bubble(&v.rack, ask));
                frames.push(frame_text_region(&v.text));
                frames.push(frame_answer_bubble(&v.rack, &v.text));
            }
            frames.push(frame_chunk(&v.text, delta));
            json!({ "frames": frames })
        }
        None => json!({ "delta": delta }),
    }
}

fn turn_round(ctx: &mut Ctx, args: Value) -> anyhow::Result<Value> {
    let round = args.get("round").and_then(Value::as_u64).unwrap_or(0);
    if round == 0 {
        anyhow::bail!("turn: pull round 0 is the Start handler's job");
    }
    // The side branch advances one pull per round, on the same face: the
    // summary is produced alongside the answer, never in front of it.
    step_tail(ctx);
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
        // still owed (see drain_tails).
        drain_tails(ctx);
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
    let turn_args = TURN_ARGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
        .unwrap_or(Value::Null);
    Ok(json!({ "done": false, "item": item_for(&turn_args, None, &delta) }))
}

/// The Start handler (`turn`): fetch the projection, assemble the
/// prompt, open the provider stream, and reply the FIRST envelope
/// (`{done:false, item:{delta}}` — the stream opens, round 1 pulls on).
fn start_envelope(ctx: &mut Ctx, args: &Value) -> anyhow::Result<Value> {
    // Capture the turn's own args for the finish path (the pull
    // handler's args are the guard's `{stream_id, round}`).
    *TURN_ARGS.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(args.clone());
    // A resident guest serves turn after turn: nothing from the previous
    // one may leak into this one (the branch list is per turn).
    TAILS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clear();
    let (first_delta, _sid) = start_turn(ctx, args)?;
    let ask = args.get("user_message").and_then(Value::as_str).map(String::from);
    Ok(json!({ "done": false, "item": item_for(args, ask.as_deref(), &first_delta) }))
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
    // What is OPEN is what the work is: a turn that cannot see its own tasks
    // starts every conversation from zero, which is the state this whole stack
    // exists to end. Best effort — a store that cannot answer is the store's
    // problem, and the model can still ask with `list_tasks`.
    if let Some(scope) = args.get("memory_scope").and_then(Value::as_u64) {
        if let Some(open) = open_tasks(&store_url, scope) {
            messages.push(json!({ "role": "system", "content": open }));
        }
    }
    // The tail branches see the same records, NUMBERED: an operation that must
    // point at one (the compression's cut) answers with an index, because the
    // store owns positions and a model cannot see them.
    let mut numbered: Vec<Value> = Vec::new();
    let mut keys: Vec<(u64, u16)> = Vec::new();
    for (index, entry) in view
        .get("increment")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
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
        if let Some(key) = entry.get("key") {
            let (Some(ts), Some(sub)) = (
                key.get("ts_ms").and_then(Value::as_u64),
                key.get("sub").and_then(Value::as_u64),
            ) else {
                continue;
            };
            keys.push((ts, sub as u16));
            numbered.push(json!({
                "role": role,
                "content": format!("[{}] {content}", index + 1),
            }));
        }
    }
    // The prefix the compression side branch summarizes: head + summary +
    // history, WITHOUT this turn's new message (the span k10r proposed to
    // cover ends before it).
    messages.push(json!({ "role": "user", "content": user_message }));

    // 3. The tail branches (Phase 3): k10r's verdict rode the projection, so
    // each operation here produces its product concurrently with the answer and
    // from the same span — the caller never states a coverage point; k10r
    // derives it from the verdict it served (ADR-0008 §4). The span is head +
    // summary + the numbered records, WITHOUT this turn's new message: what was
    // proposed to be covered ends before it.
    let mut branch_prefix: Vec<Value> = Vec::new();
    if let Some(head) = view.get("head").and_then(Value::as_str) {
        if !head.is_empty() {
            branch_prefix.push(json!({ "role": "system", "content": head }));
        }
    }
    if let Some(summary) = view.get("summary").and_then(Value::as_str) {
        if !summary.is_empty() {
            branch_prefix.push(json!({
                "role": "system",
                "content": format!("Conversation summary so far:\n{summary}"),
            }));
        }
    }
    branch_prefix.extend(numbered);
    start_tails(ctx, args, &view, &branch_prefix, &keys);

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
        // The call round itself, once: the wire shape of "the assistant asked
        // for these N tools", which the results below answer one by one.
        messages.push(json!({ "role": "assistant", "tool_calls": calls }));
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
                let before = walked.len();
                let outcome = gravity_booth::tools::call(
                    &mut doors,
                    &call.function.name,
                    &call.function.arguments,
                    &facts,
                    &mut walked,
                );
                // One report per step that EXTENDED the walk: a retried step
                // leaves the recorder as it was, and reporting it again would
                // count one walk twice.
                let after = walked.len();
                let so_far = walked.clone();
                // The recorder is released before the loop builds messages: a
                // later round's call re-takes it, and nothing else touches it.
                drop(walked);
                // The composition is filed where the walk REACHES it (ADR-0016
                // as re-cut): the turn does not have to end, let alone append
                // its reply first, for the record to exist. Best effort — the
                // proposal is an observation about the walk, never a premise of
                // the step the model asked for.
                if let (true, Some(scope)) = (
                    gravity_booth::tools::should_file(before, after, &so_far),
                    memory_scope,
                ) {
                    file_composition(&store_url, scope, &so_far);
                }
                match outcome {
                    Ok(value) => value.to_string(),
                    Err(error) => json!({ "error": error }).to_string(),
                }
            };
            // The fed-back wire shape: the tool's result (OpenAI's loop
            // contract). The result is the MODEL's, not the consumer's:
            // prism's turn stream carries text deltas (`item.delta`) and
            // refuses anything else, so the loop's progress stays inside the
            // turn (PLAN Phase 2.5's transcript). A tool PROGRESS frame for a
            // UI is prism's protocol to add, not a frame gravity invents
            // behind its back.
            //
            // ONE assistant message carries ALL the calls, and it is pushed
            // before this loop: a model that answers two requests in one
            // round produces one message with two calls, and pushing it per
            // call fed the model its own round twice — the endpoint answered
            // an empty string and the user lost the answer.
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

/// A tail operation's branch while it is live: which operation it is, its
/// provider stream, the tokens received so far, and the log pointers of the
/// records it was shown (process state — the process IS the turn).
///
/// `keys` is why the branch can be resolved at all: the prompt numbers the
/// records, so the model answers with an INDEX, and this turn — the one that
/// built the messages — is the only place that index maps back to a position.
struct Tail {
    op: String,
    stream_id: String,
    parts: Vec<String>,
    keys: Vec<(u64, u16)>,
}

/// Every live tail branch. A VECTOR, not one slot: the store asks for more than
/// one operation on the same read (compression and a review today, ADR-0017 §4),
/// and a single slot meant the last branch opened silently displaced the one
/// before it — a compression whose product never landed, which k10r then reports
/// as an open node forever.
static TAILS: std::sync::Mutex<Vec<Tail>> = std::sync::Mutex::new(Vec::new());

/// Open the tail branches k10r asked for (ADR-0008 §4): the same prefix the main
/// branch reads, the served prompt in place of this turn's message, the records
/// NUMBERED so the operation can point at one. They run CONCURRENTLY with the
/// answer — the provider pulls are interleaved by the pull face — and every
/// failure is swallowed: a tail operation is maintenance, and a broken branch
/// must never fail a turn. An unlanded compression leaves k10r's node open,
/// which is its "compressing" marker; the next read skips it.
///
/// An operation this guest does not know is SKIPPED with a log, not an error:
/// the store's list can grow before every deployed guest is upgraded, and a
/// version skew must not take a turn down.
fn start_tails(
    ctx: &mut Ctx,
    args: &Value,
    view: &Value,
    prefix: &[Value],
    keys: &[(u64, u16)],
) {
    let tails = view
        .get("tails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for tail in tails {
        let Some(name) = tail.get("name").and_then(Value::as_str) else {
            continue;
        };
        if name != OP_COMPRESS && name != OP_REVIEW {
            eprintln!("[gravity-turn] tail operation `{name}` is not known to this guest");
            continue;
        }
        open_branch(ctx, args, &tail, name, prefix, keys);
    }
}

/// The one operation this guest implements. The name is the dispatch key on both
/// sides — k10r's `COMPRESS_OP.name` and this constant are the same string, and
/// neither side derives it from the other.
const OP_COMPRESS: &str = "compress";
/// The review pass (k10r ADR-0017 §4): what the conversation should have written
/// down and did not. Its product lands through the memory door, the same one
/// `commit_memory` uses — no second schema and no new gate.
const OP_REVIEW: &str = "review";

fn open_branch(
    ctx: &mut Ctx,
    args: &Value,
    tail: &Value,
    name: &str,
    prefix: &[Value],
    keys: &[(u64, u16)],
) {
    let Some(prompt) = tail.get("prompt").and_then(Value::as_str) else {
        return;
    };
    let mut messages: Vec<Value> = prefix.to_vec();
    messages.push(json!({ "role": "user", "content": prompt }));
    // The operation's OWN provider instance (see the note above): the guest keeps
    // one stream per process, so this is what keeps two branches from sharing one.
    let first = match ctx.iterate_start(
        "llm_provider",
        name,
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
            eprintln!("[gravity-turn] `{name}` branch failed to open: {e:#}");
            return;
        }
    };
    // A string reply is the provider's error value crossing the bridge.
    if let Some(err) = first.as_str() {
        eprintln!("[gravity-turn] `{name}` branch failed: {err}");
        return;
    }
    // A branch that closed on its first round carries no stream id (the realm
    // merges one only for a non-terminal reply): nothing to land.
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
    TAILS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Tail {
            op: name.to_string(),
            stream_id: sid.to_string(),
            parts,
            keys: keys.to_vec(),
        });
}

/// One pull round of the live branch. On `done` its product goes to the door the
/// operation is responsible for. k10r derives the coverage point from the verdict
/// it served, so the caller never states WHERE the node ends — but the record a
/// compression says it should have ended at is the caller's observation to make,
/// because only the caller built the messages the model read.
fn step_tail(ctx: &mut Ctx) {
    // One round on ONE branch, taking them in turn: the branches are pulled
    // round-robin so a long tail cannot starve the others, and each keeps its own
    // accumulated text (`parts`) across rounds.
    let mut branch = {
        let mut tails = TAILS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if tails.is_empty() {
            return;
        }
        tails.remove(0)
    };
    let env = match ctx.iterate_next(&branch.stream_id) {
        Ok(value) => value,
        Err(e) => {
            eprintln!("[gravity-turn] `{}` pull failed: {e:#}", branch.op);
            return;
        }
    };
    if env.get("done").and_then(Value::as_bool) == Some(true) {
        let _ = ctx.iterate_dispose(&branch.stream_id);
        if let Some(err) = env.get("value").and_then(Value::as_str) {
            eprintln!("[gravity-turn] `{}` ended without a product: {err}", branch.op);
            return;
        }
        let text = branch.parts.concat();
        if text.trim().is_empty() {
            return;
        }
        land_tail(&branch, &text);
        return;
    }
    if let Some(delta) = env
        .get("item")
        .and_then(|item| item.get("delta"))
        .and_then(Value::as_str)
    {
        branch.parts.push(delta.to_string());
    }
    TAILS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(branch);
}

/// Drain the live branch before the turn closes. The carrier releases the turn's
/// stream at `done` and the guest's state goes with it, so a branch still live
/// here is pulled to completion: the summary must land before the next user
/// message (Phase 3), and the answer has already streamed, so the only wait is
/// the product's own tail. The cap keeps a runaway branch from holding the turn
/// open; what is left unlanded is k10r's open node, which its next read treats
/// as "compressing".
fn drain_tails(ctx: &mut Ctx) {
    for _ in 0..64 {
        let live = !TAILS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty();
        if !live {
            return;
        }
        step_tail(ctx);
    }
    eprintln!("[gravity-turn] a tail branch was still live after 64 pulls");
}

/// What an operation's answer MEANT. A compression answers with one JSON object
/// carrying the summary and the cut it would have chosen; anything else is taken
/// at face value as the summary with no opinion.
///
/// The fallback is deliberate: compression is necessary, and a model that
/// ignored the format still produced a usable summary. Dropping the whole product
/// over a shape slip would lose the only thing the branch exists to make.
fn read_products(text: &str, keys: &[(u64, u16)]) -> (String, Option<(u64, u16)>) {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let parsed: Option<Value> = serde_json::from_str(unfenced).ok();
    let Some(parsed) = parsed.filter(|v| v.is_object()) else {
        return (trimmed.to_string(), None);
    };
    let Some(summary) = parsed
        .get("summary")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
    else {
        return (trimmed.to_string(), None);
    };
    // `cut` is a 1-based index over the records the branch was shown. Absent,
    // zero, or the last one all mean "the mechanical cut was right" — the common
    // case, and the one that must not fill the row with a useless opinion.
    let cut = parsed.get("cut").and_then(Value::as_u64).unwrap_or(0);
    let suggested = if cut == 0 || cut as usize >= keys.len() {
        None
    } else {
        keys.get(cut as usize - 1).copied()
    };
    (summary.to_string(), suggested)
}

/// Hand one product to its door. 2xx is the answer; anything else is REPORTED
/// and the turn goes on (the alternative, failing the turn over maintenance,
/// would trade a user-visible answer for a bookkeeping one).
fn land_tail(branch: &Tail, text: &str) {
    match branch.op.as_str() {
        OP_COMPRESS => {
            let (summary, suggested) = read_products(text, &branch.keys);
            land_summary(&summary, suggested);
        }
        OP_REVIEW => match read_review_facts(text) {
            Some(facts) if facts.is_empty() => {
                eprintln!("[gravity-turn] review: nothing worth writing down");
            }
            Some(facts) => land_review(&facts),
            // A review whose answer is not the shape saves NOTHING: unlike a
            // summary, there is no "usable" version of a fact list, and guessing
            // what prose meant would write invented facts into the graph.
            None => eprintln!("[gravity-turn] review: the answer was not a fact array"),
        },
        other => eprintln!("[gravity-turn] no landing door for `{other}`"),
    }
}

/// What a REVIEW's answer meant: `{"facts": [[8 slots], …]}` or nothing.
///
/// Tolerant of a fenced block, intolerant of anything else. Unlike compression —
/// where prose still summarizes — prose cannot be read as facts, so the range
/// answer is `None` and the caller writes nothing.
fn read_review_facts(text: &str) -> Option<Vec<Value>> {
    let trimmed = text.trim();
    let unfenced = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|rest| rest.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let parsed: Value = serde_json::from_str(unfenced).ok()?;
    let facts = parsed.get("facts")?.as_array()?;
    // Every entry must be an 8-slot tuple: the door rejects anything else, and a
    // partial batch would be a silent half-commit.
    let shaped = facts
        .iter()
        .all(|f| f.as_array().is_some_and(|slots| slots.len() == 8));
    shaped.then(|| facts.clone())
}

/// `POST …/memories/{scope}` — the door `commit_memory` writes through. Same body
/// shape (an array of 8-slot tuples), so a review needs no vocabulary of its own.
fn land_review(facts: &[Value]) {
    let Some(args) = TURN_ARGS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
    else {
        return;
    };
    let (Some(store_url), Some(scope)) = (
        args.get("store_url").and_then(Value::as_str),
        args.get("memory_scope").and_then(Value::as_u64),
    ) else {
        eprintln!("[gravity-turn] review: no store or scope on this deployment");
        return;
    };
    let url = format!("{}/memories/{scope}", store_url.trim_end_matches('/'));
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_json(Value::Array(facts.to_vec()))
    {
        Ok(resp) if (200..300).contains(&resp.status()) => {}
        Ok(resp) => eprintln!("[gravity-turn] review door answered {}", resp.status()),
        Err(e) => eprintln!("[gravity-turn] review door failed: {e}"),
    }
}

/// `POST …/checkpoint { summary, suggested_ts?, suggested_sub? }` — the door k10r
/// opened when it served the verdict. The suggestion rides along and is the
/// only thing this caller states about position; k10r validates it against the
/// span it opened.
fn land_summary(summary: &str, suggested: Option<(u64, u16)>) {
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
    let mut body = json!({ "summary": summary });
    if let Some((ts, sub)) = suggested {
        body["suggested_ts"] = json!(ts);
        body["suggested_sub"] = json!(sub);
    }
    match ureq::post(&url)
        .set("Content-Type", "application/json")
        .send_string(&body.to_string())
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
    Ok(json!({ "done": true, "value": { "reply": reply } }))
}

/// File one composition: the code rows the walk has visited so far, in order
/// (ADR-0016). Reported, never promoted — k10r keeps the count, a human decides
/// what it becomes, and the resolver keeps showing only real skills.
///
/// Called where the walk grows, so a three-step walk files `a+b` and then
/// `a+b+c`. That is not noise: the prefix WAS walked, and a review queue ranked
/// by count should see both.
fn file_composition(store_url: &str, scope: u64, walked: &[gravity_booth::tools::Walked]) {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let body = json!({
        "key": gravity_booth::tools::composition_key(walked),
        "steps": gravity_booth::tools::steps_json(walked),
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

/// The tasks a turn must know about, as one system block. `None` when none are
/// owed or the store cannot answer: an empty preamble is noise, and a store error
/// is not this executor's to report.
fn open_tasks(store_url: &str, scope: u64) -> Option<String> {
    let resp = ureq::get(&format!("{store_url}/tasks/{scope}")).call().ok()?;
    let tasks: Value = serde_json::from_reader(resp.into_reader()).ok()?;
    gravity_booth::tasks::render_owed(tasks.as_array()?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{read_products, read_review_facts};

    fn keys() -> Vec<(u64, u16)> {
        vec![(100, 0), (200, 0), (300, 1)]
    }

    #[test]
    fn a_compression_answer_carries_both_products() {
        let (summary, cut) = read_products(
            r#"{"summary": "they agreed on the cut", "cut": 2}"#,
            &keys(),
        );
        assert_eq!(summary, "they agreed on the cut");
        assert_eq!(
            cut,
            Some((200, 0)),
            "the index the model gave is resolved by the turn that built the messages"
        );
    }

    #[test]
    fn the_last_index_is_the_mechanical_cut_restated() {
        // The common answer: the span belongs together. Nothing to record —
        // an opinion equal to the mechanical point would fill the row with a
        // suggestion that changes nothing.
        for cut in [3, 4, 0] {
            let (summary, suggested) = read_products(
                &format!(r#"{{"summary": "s", "cut": {cut}}}"#),
                &keys(),
            );
            assert_eq!(summary, "s");
            assert_eq!(suggested, None, "cut {cut} is not a better cut");
        }
    }

    #[test]
    fn a_fenced_answer_is_still_an_answer() {
        let (summary, cut) = read_products(
            "```json\n{\"summary\": \"s\", \"cut\": 1}\n```",
            &keys(),
        );
        assert_eq!(summary, "s");
        assert_eq!(cut, Some((100, 0)));
    }

    #[test]
    fn prose_is_taken_at_face_value_rather_than_dropped() {
        // Compression is necessary and a model that ignored the format still
        // produced a summary. Losing the product over a shape slip would lose
        // the only thing the branch exists to make.
        let (summary, cut) = read_products("The user wants the cut moved earlier.", &keys());
        assert_eq!(summary, "The user wants the cut moved earlier.");
        assert_eq!(cut, None);
    }

    #[test]
    fn json_without_a_summary_is_prose() {
        let (summary, cut) = read_products(r#"{"cut": 2}"#, &keys());
        assert_eq!(summary, r#"{"cut": 2}"#, "no summary means no products");
        assert_eq!(cut, None);
    }

    /// A review's answer is a fact array or nothing: prose cannot be read as
    /// facts, because guessing what it meant would write invented facts into the
    /// graph (compression, by contrast, still has a summary in prose).
    #[test]
    fn a_review_reads_a_fact_array_and_nothing_else() {
        let fact = r#"["acme","repository",{},"OWNS",{},"issues-api","repository",{}]"#;
        let answer = format!("{{\"facts\": [{fact}]}}");
        assert_eq!(
            read_review_facts(&answer).map(|facts| facts.len()),
            Some(1),
            "a shaped tuple is a fact"
        );
        assert_eq!(
            read_review_facts(&format!("```json\n{answer}\n```")).map(|facts| facts.len()),
            Some(1),
            "a fenced block is still the answer"
        );
        assert_eq!(
            read_review_facts(r#"{"facts": []}"#).map(|facts| facts.len()),
            Some(0),
            "nothing to save is a legal answer, and not the same as a bad one"
        );
        assert_eq!(read_review_facts("I looked and nothing stood out."), None);
        assert_eq!(
            read_review_facts(r#"{"facts": [["acme","repository"]]}"#),
            None,
            "a tuple that is not 8 slots is not a fact the door can take"
        );
        assert_eq!(read_review_facts(r#"{"facts": [null]}"#), None);
    }
}


