//! The turn guest — an induction process: one turn per activation, start to end.
//!
//! `turn(args)` is a HANDLER, not a producer. The venue tells this booth what happened
//! (`ui:turn.submit`, the subscription registered beside it in `lib.rs`) and the booth
//! says what it drew: every chunk is its own emit, and the handler's return value IS the
//! turn's end. Nothing pulls this booth, so no state has to survive a call boundary — the
//! whole turn is one call stack, and the statics a pull face needed are gone.
//!
//! 1. `GET {store_url}/channels/{c}/members/{m}/session?format=` — the projection
//!    (head + summary + increment), the mem-server wire shape. The head is TEXT: k10r
//!    renders the initialization memory, gravity never assembles the graph (ADR-0010 §5).
//! 2. Prompt assembly: head block (system) + summary preamble (system) + increment + the
//!    user message.
//! 3. `ctx.iterate_start(llm_provider, "", "stream", provider_args)`, and in this same
//!    call the tool rounds and then the answer: each envelope item's `delta` leaves as a
//!    frame on the venue wire.
//! 4. On the provider's `done`: append user + assistant with one shared `ts_ms`, subs 0/1
//!    — ONLY after a successful stream (a failed turn appends nothing; the user re-sends
//!    next turn). Gravity's clock IS the channel's time (krystallizer ADR-0008 §3).
//!
//! Failure is a value (ADR-0012): the handler's answer carries the error text, and a
//! drawing caller is told on the same wire (`ui:turn.error`) — but nothing is appended.
//!
//! Args (the envelope's `data`, or the flat call of a direct caller):
//! `{"channel_id", "member_id", "user_message", "store_url", "endpoint", "model",
//! "key_env"}`, plus the output shape — `protocol: "fluxen"` with `slots {rack, text}`,
//! or `event` naming the event raw deltas are spoken as. The envelope's OWN `channel_id`
//! is an ADDRESS (a device, text) and is a different fact from the conversation's
//! conversation's own ids inside `data`. The venue addresses DEVICES, never channels (prism
//! ADR-0018: a channel is an application's business, the transport knows devices); the turn
//! answers the device that asked and names no grouping.

use anyhow::Context;
use gravity_booth::tools::Doors;
use gravity_core::surface::MemoryFormat;
use induction_guest::{Ctx, Guest};
use serde_json::{json, Value};

fn main() -> anyhow::Result<()> {
    Guest::new()
        .named("gravity-turn")
        // The per-handler auth declaration (prism's gate reads it): the turn needs a
        // bound user — the SAME declaration face the echo plane enforces, no second ACL
        // table. The key is the EVENT the venue's gate names (ADR-0045: a declaration
        // binds an entry, so event and entry may differ — `ui:turn.submit` here,
        // `turn` there).
        .auth(json!({
            "ui:turn.submit": true,
            "ui:gravity-model.save": true,
            "ui:gravity-model": true,
        }))
        // ONE HANDLER. The venue's word for "run a turn" reaches this booth as a
        // subscription and the realm names the instance by the same address, so the
        // handler runs the turn to its end. There is no producer binding: a stream this
        // booth no longer owns is a stream nobody has to pull.
        .on("turn", |ctx: &mut Ctx, args: Value| run(ctx, &args))
        // THE OTHER HANDLER: where a deployment is configured. The facts are rows in this type's
        // ns (below), so something outside the process has to be able to write them — and this
        // booth refuses an admin verb, so that door is an event like the turn's, behind the same
        // auth declaration.
        .on("save", |ctx: &mut Ctx, args: Value| save(ctx, &args))
        // And the SCREEN that asks for it: ADR-0007 §4 said management is a view, so the same
        // booth that performs a save draws the forms that emit it.
        .on("screen", |ctx: &mut Ctx, args: Value| screen(ctx, &args))
        // And the SHELL: a renderer that connects gets a layout, or the page is blank and the
        // screen has nowhere to land. Public on purpose — the shell is chrome; `screen` is gated.
        .on("shell", |ctx: &mut Ctx, args: Value| shell(ctx, &args))
        // THE TURN'S OWN STORAGE (ADR-0007): the deployment facts are rows in the field, in
        // this type's ns, so a venue that serves this application needs no model configuration
        // of its own. The schema is the declaration; `Engine::register` persists it and the
        // realm builds the plan the `ctx.store` arm below executes against.
        .storage(storage_schema())
        .run()
}

/// The declared collections. `settings` holds one row (id 1) with the deployment's facts; `models`
/// holds one row per model recipe, addressed by id (`settings.default_model` names the default).
/// Cold fields are the variable-length ones (a Str cannot be a key, and TLV is where it lives);
/// every number is hot, so a read is one fixed-width segment.
fn storage_schema() -> Value {
    let slots = json!({
        "primary": 0, "dynamic": 1, "dict_id": 2, "dict_name": 3,
        "declared_index_base": 4096, "declared_reduce_base": 8192, "junction_base": 12288,
    });
    // A TYPE OWNS ONE NS (aura ADR-0026): every collection of this type shares it, so the key
    // carries the discriminator — without it two collections with the same `id` are one row.
    let key = json!([
        {"name": "kind", "ty": "U64", "width": 8, "offset": 0, "tag": 0},
        {"name": "id", "ty": "U64", "width": 8, "offset": 8, "tag": 0},
    ]);
    // The induction schema nests this under `storage` already, so the value IS the collections map
    // (wrapping it again would parse as "this type declares nothing").
    json!({
        "collections": {
            "settings": {
                "schema": {
                    "key_len": 16,
                    "key_fields": key,
                    "layout_version": 1,
                    "hot_width": 16,
                    "payload_header_len": 3,
                    "hot_fields": [
                        {"name": "memory_scope", "ty": "U64", "width": 8, "offset": 0, "tag": 0},
                        {"name": "default_model", "ty": "U64", "width": 8, "offset": 8, "tag": 0},
                    ],
                    "cold_fields": [
                        {"name": "store_url", "ty": "Str", "width": 0, "offset": 0, "tag": 1},
                        {"name": "memory_format", "ty": "Str", "width": 0, "offset": 0, "tag": 2},
                    ],
                    "slots": slots.clone(),
                },
            },
            "models": {
                "schema": {
                    "key_len": 16,
                    "key_fields": key,
                    "layout_version": 1,
                    "hot_width": 16,
                    "payload_header_len": 3,
                    "hot_fields": [
                        {"name": "weight", "ty": "U64", "width": 8, "offset": 0, "tag": 0},
                        {"name": "enabled", "ty": "U64", "width": 8, "offset": 8, "tag": 0},
                    ],
                    "cold_fields": [
                        {"name": "alias", "ty": "Str", "width": 0, "offset": 0, "tag": 1},
                        {"name": "endpoint", "ty": "Str", "width": 0, "offset": 0, "tag": 2},
                        {"name": "model", "ty": "Str", "width": 0, "offset": 0, "tag": 3},
                        {"name": "key_env", "ty": "Str", "width": 0, "offset": 0, "tag": 4},
                    ],
                    "slots": slots.clone(),
                },
            },
        },
    })
}

/// The key discriminator each collection uses inside the type's ONE ns (see `storage_schema`).
const KIND_SETTINGS: u64 = 1;
const KIND_MODELS: u64 = 2;

/// One row of `settings` (id 1) — the deployment facts a turn needs, which used to ride the call
/// arguments from the gateway (ADR-0007).
struct Settings {
    store_url: String,
    memory_scope: u64,
    memory_format: String,
    default_model: u64,
}

/// Read the settings row. A fresh deployment has none, and that is a NAMED error: the turn says
/// which document and which keys are missing rather than falling back to an environment variable
/// (a fallback is a second home by another name, ADR-0007 §5).
fn read_settings(ctx: &mut Ctx) -> anyhow::Result<Settings> {
    let row = ctx.store(json!({
        "collection": "settings", "op": "get_document", "key": { "kind": KIND_SETTINGS, "id": 1 },
    }))?;
    if row.is_null() {
        anyhow::bail!(
            "turn: this deployment has no settings row — write `settings` id 1 with `store_url`, \
             `memory_scope`, `memory_format` and `default_model` (a model row's id)"
        );
    }
    let text = |key: &str| -> anyhow::Result<String> {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("turn: `settings` id 1 is missing `{key}`"))
    };
    Ok(Settings {
        store_url: text("store_url")?,
        memory_scope: row
            .get("memory_scope")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("turn: `settings` id 1 is missing `memory_scope`"))?,
        memory_format: text("memory_format")?,
        default_model: row
            .get("default_model")
            .and_then(Value::as_u64)
            .ok_or_else(|| anyhow::anyhow!("turn: `settings` id 1 is missing `default_model`"))?,
    })
}

/// The model recipe a turn uses: `endpoint` / `model` / `key_env` off the row `settings` named.
struct Recipe {
    endpoint: String,
    model: String,
    key_env: String,
}

fn read_recipe(ctx: &mut Ctx, id: u64) -> anyhow::Result<Recipe> {
    let row = ctx.store(json!({
        "collection": "models", "op": "get_document", "key": { "kind": KIND_MODELS, "id": id },
    }))?;
    if row.is_null() {
        anyhow::bail!("turn: `settings` names model {id}, and there is no such row in `models`");
    }
    let text = |key: &str| -> anyhow::Result<String> {
        row.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| anyhow::anyhow!("turn: model {id} is missing `{key}`"))
    };
    Ok(Recipe { endpoint: text("endpoint")?, model: text("model")?, key_env: text("key_env")? })
}

/// Where a drawing turn puts its frames. One event for the whole turn: the frames carry
/// their own slots, so the name is an address, not a destination.
const UI_EVENT: &str = "ui:turn";
/// A drawing turn's end, said as an event so the page can stop its streaming state
/// without knowing a verb.
const DONE_EVENT: &str = "ui:turn.done";
/// A drawing turn's failure, said the same way.
const ERROR_EVENT: &str = "ui:turn.error";
/// Where a deployment's configuration is ANSWERED: the save is an event, so its result is said.
const SAVED_EVENT: &str = "ui:gravity-model.saved";
/// A save this booth refused, said the same way (the failure is a value, ADR-0012).
const SAVE_REFUSED_EVENT: &str = "ui:gravity-model.error";

/// The ops that put a line in the notice area. Its own name — the venue's `ui:notice` speaks a map
/// and lands in that slot as a whole value, which is a different shape from a row append.
const NOTICE_EVENT: &str = "ui:gravity-model.notice";

/// Where the frames go when the caller asked for the view protocol. THE CALLER DECLARES
/// THEM (gravity does not own a layout, so it cannot know a slot's name): `slots.rack` is
/// the list the turn appends to, `slots.text` the region the answer streams into.
struct View {
    rack: String,
    text: String,
}

/// What the caller asked its chunks to BE. Both shapes ride `ctx.emit`; the difference is
/// whether a renderer draws them or a consumer reads them.
enum Out<'a> {
    /// Accrete frames against the slots the caller declared.
    View(View),
    /// Raw deltas at the event the caller named (`event`). Gravity owns no event names:
    /// a consumer that wants `{"delta": …}` says what it wants it called.
    Data(&'a str),
}

fn out_of(args: &Value) -> anyhow::Result<Out<'_>> {
    match args.get("protocol").and_then(Value::as_str) {
        Some("fluxen") => {
            let slots = args
                .get("slots")
                .context("turn: `protocol: fluxen` needs `slots`")?;
            Ok(Out::View(View {
                rack: slots
                    .get("rack")
                    .and_then(Value::as_str)
                    .context("turn: `slots.rack` is the list the turn appends to")?
                    .to_string(),
                text: slots
                    .get("text")
                    .and_then(Value::as_str)
                    .context("turn: `slots.text` is the region the answer streams into")?
                    .to_string(),
            }))
        }
        _ => Ok(Out::Data(
            args.get("event")
                .and_then(Value::as_str)
                .context("turn: a delta consumer must name the `event` its chunks are spoken as")?,
        )),
    }
}

/// One venue emit. `name` is the recipient event — a compiled view body is not told which
/// event fired (the concrete name is consumed by the dispatcher), so the emitter states
/// it; `device` is the recipient (one device: an application that has groups would name
/// more, but this one has no channel to be a member of); the payload hangs off `data`,
/// which is the shape the venue reads as "a delivery, not a request".
fn emit(ctx: &mut Ctx, event: &str, device: Option<&str>, data: Value) -> anyhow::Result<()> {
    let mut envelope = json!({ "name": event });
    if let Some(device) = device {
        // NO `sender` on a booth's own speech: the venue excludes the sender from its
        // fan-out, so marking the asking device as the sender would send the answer to
        // everyone EXCEPT the one connection that wants it.
        envelope["device"] = json!(device);
    }
    envelope["data"] = data;
    ctx.emit(event, envelope)
}

/// The text region the answer streams into: an empty text node bound to a default value.
fn frame_text_region(text: &str) -> Value {
    json!({
        "action": "set",
        "slot": text,
        "data": {"type": "text", "bind": {"value": {"kind": "default", "default": ""}}},
    })
}

/// One bubble in the rack whose text child reads the streaming region — so a patch of the
/// region re-renders the paragraph, not the box (fluxen's walkthrough rule).
fn frame_answer_bubble(rack: &str, text: &str) -> Value {
    json!({
        "action": "append",
        "slot": rack,
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
        "slot": rack,
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
        "slot": text,
        "path": "/bind/value/default",
        "op": "append",
        "value": chunk,
    })
}

/// The frames one chunk draws. The FIRST chunk also sets the stage (the question, the
/// region, the bubble that reads it) — one batch, in order, because the patch that fills
/// the region must find it.
fn frames_for(view: &View, first: Option<&str>, chunk: &str) -> Vec<Value> {
    let mut frames: Vec<Value> = Vec::new();
    if let Some(ask) = first {
        frames.push(frame_ask_bubble(&view.rack, ask));
        frames.push(frame_text_region(&view.text));
        frames.push(frame_answer_bubble(&view.rack, &view.text));
    }
    frames.push(frame_chunk(&view.text, chunk));
    frames
}

/// The caller's payload, whichever way this booth was reached. A venue puts the caller's
/// data under `data` and the speaker in `sender` (`{name, sender, data}` — prism ADR-0017's
/// `ui:` envelope); a direct caller (a test, a sibling booth) sends the args flat. The
/// hidden parameter is "who wrapped this", so unwrap once here instead of at every field.
fn payload(envelope: &Value) -> &Value {
    envelope
        .get("data")
        .filter(|v| v.is_object())
        .unwrap_or(envelope)
}

/// WHO asked: the envelope's sender device. It is also the recipient of every frame this
/// turn draws — one conversation, one device, and no channel in between (prism ADR-0018).
fn device_of(envelope: &Value) -> Option<String> {
    envelope
        .get("sender")
        .and_then(|sender| sender.get("device"))
        .and_then(Value::as_str)
        .or_else(|| envelope.get("device").and_then(Value::as_str))
        .map(str::to_string)
}

fn run(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    let args = payload(envelope);
    let drawing = args
        .get("protocol")
        .and_then(Value::as_str)
        .is_some_and(|protocol| protocol == "fluxen");
    // AN EVENT-ROUTED SUBMIT HAS NO ANSWER IT CAN READ: a delivery discards a return value, so its
    // end is SAID, not returned. A direct caller (iterate/call — no envelope `name`) still reads the
    // terminal value, which is what ADR-0009 states for the data protocol. The drawing caller is
    // always event-routed, and it needs the end regardless: its submit was an emit.
    let said = drawing || envelope.get("name").is_some();
    match turn(ctx, args, device.as_deref()) {
        Ok(reply) => {
            if said {
                emit(ctx, DONE_EVENT, device.as_deref(), json!({}))?;
            }
            Ok(json!({ "done": true, "value": { "reply": reply } }))
        }
        Err(error) => {
            if said {
                let detail = format!("{error:#}");
                let _ = emit(ctx, ERROR_EVENT, device.as_deref(), json!({ "detail": detail }));
            }
            Err(error)
        }
    }
}

/// The deployment facts one turn holds: what the tail doors and the append need. A LOCAL,
/// not a static — the whole turn is one call stack, so nothing has to survive a call
/// boundary (a resident guest no longer carries one turn's state into the next).
struct Turn<'a> {
    /// The caller's payload: what the PAGE knows (its conversation, the text, the mount table).
    args: &'a Value,
    /// Already trailing-slash-trimmed.
    store_url: String,
    channel_id: u64,
    member_id: u64,
    memory_scope: u64,
    /// The head's representation — the deployment's choice, stored beside the other facts.
    memory_format: String,
    /// The model recipe the settings row named.
    endpoint: String,
    model: String,
    key_env: String,
}

fn turn(ctx: &mut Ctx, args: &Value, device: Option<&str>) -> anyhow::Result<String> {
    // THE DEPLOYMENT FACTS ARE ROWS, NOT ARGUMENTS (ADR-0007): the store, the memory scope, the
    // representation and the model recipe live in this type's own namespace, so a gateway never
    // carries them and a caller cannot forget one.
    let settings = read_settings(ctx)?;
    let recipe = read_recipe(ctx, settings.default_model)?;
    let store_url = settings.store_url.trim_end_matches('/').to_string();
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
    let turn = Turn {
        args,
        store_url: store_url.clone(),
        channel_id,
        member_id,
        memory_scope: settings.memory_scope,
        memory_format: settings.memory_format,
        endpoint: recipe.endpoint,
        model: recipe.model,
        key_env: recipe.key_env,
    };
    let out = out_of(args)?;

    // 1. The session projection (mem-server's SessionDto shape). The head's
    // representation is the deployment's choice, and `struct` is refused
    // here: it returns the fact DATA, which this executor does not assemble.
    let format = MemoryFormat::from_config(Some(turn.memory_format.as_str()))
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
    {
        let scope = turn.memory_scope;
        if let Some(open) = open_tasks(&turn.store_url, scope) {
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
    // derives it from the verdict it served (ADR-0008 §4).
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
    let mut tails: Vec<Tail> = Vec::new();
    start_tails(ctx, &turn, &view, &branch_prefix, &keys, &mut tails);

    // 4. The provider: open, run the tool rounds, and land on a CONTENT stream. A
    //    `tool_calls`-first reply = the model wants tools BEFORE any prose: dispatch them,
    //    feed the results back, re-open — the "next inference round" (Phase 2). The results
    //    are the MODEL's, not the consumer's, so they never ride the drawing wire.
    let mut walked: Vec<gravity_booth::tools::Walked> = Vec::new();
    let mut first = open_provider(ctx, &turn, &messages, "turn: provider stream failed to open")?;
    for _round in 0..8 {
        if first.get("done").and_then(Value::as_bool) == Some(true) {
            return Err(stream_failure(
                &first,
                "turn: provider stream failed to open",
            ));
        }
        let Some(calls) = first.get("item").and_then(|i| i.get("tool_calls")) else {
            break; // a content stream — the stream face takes over
        };
        // Dispatch every call, feed the messages back, and re-open the
        // provider — the loop's own rounds. The list is the contract
        // (ADR-0004): every failure is a value the MODEL reads, never a
        // turn failure — a wrong argument must not cost the user their
        // answer.
        let parsed = gravity_core::tool::parse_tool_calls(&json!({ "tool_calls": calls }))
            .context("turn: malformed tool calls from the provider")?;
        // The mount table the deployment declares (prism's manifest, carried in
        // the turn args): operation language → the mount type that serves it.
        // Empty is a fact ("this deployment declares no nodes"), absent is not.
        let mounts = mounts_of(args);
        // Residency for a mount call is the CONVERSATION (ADR-0001 §6 leaves
        // the key to the caller): a step script's own state survives the turns
        // of one conversation and nothing else.
        let session = format!("channel-{channel_id}-member-{member_id}");
        // The call round itself, once: the wire shape of "the assistant asked
        // for these N tools", which the results below answer one by one.
        messages.push(json!({ "role": "assistant", "tool_calls": calls }));
        for call in parsed {
            let facts = gravity_booth::tools::Facts {
                store_url: &turn.store_url,
                memory_scope: Some(turn.memory_scope),
                mounts: &mounts,
                session: &session,
            };
            // The ctx borrow lives only for this call: the provider re-open
            // below needs it back.
            let content = {
                let mut doors = VenueDoors { ctx: &mut *ctx };
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
                // The composition is filed where the walk REACHES it (ADR-0016
                // as re-cut): the turn does not have to end, let alone append
                // its reply first, for the record to exist. Best effort — the
                // proposal is an observation about the walk, never a premise of
                // the step the model asked for.
                if gravity_booth::tools::should_file(before, after, &so_far) {
                    file_composition(&turn.store_url, turn.memory_scope, &so_far);
                }
                match outcome {
                    Ok(value) => value.to_string(),
                    Err(error) => json!({ "error": error }).to_string(),
                }
            };
            // ONE assistant message carries ALL the calls, and it is pushed
            // before this loop: a model that answers two requests in one round
            // produces one message with two calls, and pushing it per call fed
            // the model its own round twice — the endpoint answered an empty
            // string and the user lost the answer.
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call.id,
                "content": content,
            }));
        }
        first = open_provider(ctx, &turn, &messages, "turn: provider re-open failed")?;
    }
    // The loop must have ENDED on a content stream: the cap is the runaway
    // stop, and a provider still asking for tools after it is a failed turn —
    // never an empty first token.
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

    // 5. The answer, one emit per chunk, with a live side branch advanced between pulls.
    let answer = stream_answer(ctx, &out, device, &sid, delta, &turn, &mut tails);
    // The mandatory dual of Start (ADR-0034 §3): reached on BOTH paths — a guest is
    // resident, so a stream left open outlives the turn that opened it.
    let _ = ctx.iterate_dispose(&sid);
    let reply = match answer {
        Ok(reply) => reply,
        Err(error) => {
            // Nothing lands and nothing appends: the branches were maintenance, and one
            // left open here would be pulled by no one.
            for branch in tails.drain(..) {
                let _ = ctx.iterate_dispose(&branch.stream_id);
            }
            return Err(error);
        }
    };
    // The answer has streamed; the summary's tail is the only thing still owed (see
    // drain_tails).
    drain_tails(ctx, &turn, &mut tails);

    // 6. The exchange, only now: one ts, subs 0/1. Gravity's clock IS the channel's time
    //    (krystallizer ADR-0008 §3).
    if reply.is_empty() {
        anyhow::bail!("turn: provider stream yielded no tokens");
    }
    append_exchange(&turn, &user_message, &reply)?;
    Ok(reply)
}

/// One pull round of the answer stream: emit the chunk, advance a live side branch, pull
/// the next envelope. Returns the assembled reply at the provider's `done`.
fn stream_answer(
    ctx: &mut Ctx,
    out: &Out<'_>,
    device: Option<&str>,
    sid: &str,
    mut delta: String,
    turn: &Turn<'_>,
    tails: &mut Vec<Tail>,
) -> anyhow::Result<String> {
    let mut reply = String::new();
    let mut first = true;
    loop {
        match out {
            Out::View(view) => {
                let ask = first
                    .then(|| turn.args.get("user_message").and_then(Value::as_str))
                    .flatten();
                emit(ctx, UI_EVENT, device, json!(frames_for(view, ask, &delta)))?;
            }
            Out::Data(event) => emit(ctx, event, device, json!({ "delta": delta }))?,
        }
        first = false;
        reply.push_str(&delta);

        // The side branch advances one pull per round, on the same face: the summary is
        // produced alongside the answer, never in front of it.
        step_tail(ctx, turn, tails);
        let env = ctx
            .iterate_next(sid)
            .context("turn: provider pull failed")?;
        if env.get("done").and_then(Value::as_bool) == Some(true) {
            // A failed stream ends with the error text as its value; the answer so far is
            // not an exchange.
            if let Some(error) = env.get("value").and_then(Value::as_str) {
                anyhow::bail!("turn: the provider stream failed: {error}");
            }
            return Ok(reply);
        }
        delta = env
            .get("item")
            .and_then(|item| item.get("delta"))
            .and_then(Value::as_str)
            .context("turn: a non-terminal provider envelope carried no delta")?
            .to_string();
    }
}

/// Open (or reopen) the provider stream and answer its first envelope. A string reply is
/// the provider handler's error value crossing the host bridge (errors cross as values,
/// ADR-0012), so it fails HERE with the caller's own wording rather than at a later pull.
fn open_provider(
    ctx: &mut Ctx,
    turn: &Turn<'_>,
    messages: &[Value],
    what: &str,
) -> anyhow::Result<Value> {
    let reply = ctx
        .iterate_start(
            "llm_provider",
            "",
            "stream",
            json!({
                "endpoint": turn.endpoint,
                "model": turn.model,
                "key_env": turn.key_env,
                "messages": messages,
                // The fixed list (ADR-0004): the same bytes every turn, and
                // the reason a real model can call anything at all.
                "tools": gravity_booth::tools::tools_array(),
            }),
        )
        .with_context(|| what.to_string())?;
    if let Some(message) = reply.as_str() {
        anyhow::bail!("{what}: {message}");
    }
    Ok(reply)
}

/// The mount table the deployment declares: operation language → the mount type serving
/// it (prism's manifest, carried in the turn args).
fn mounts_of(args: &Value) -> Vec<(String, String)> {
    args.get("mounts")
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
        .unwrap_or_default()
}

/// What the screen ANSWERS with. Never the request (`ui:gravity-model`, registered in `lib.rs`):
/// this booth is subscribed to that one, and answering under the same name makes it hear its own
/// answer — measured, not feared: `ui:gravity-model` emitted 616 times for one click before the
/// shape was split. The renderer does not care about the name (it draws whatever arrives); a
/// program that wants the frames listens under this one.
const SCREEN_EVENT: &str = "ui:gravity-model.screen";

/// What this booth ANSWERS with. Not `ui:connect`: that is the fact about a connection, and
/// answering it under its own name is a loop — the booth is subscribed to it.
const LAYOUT_EVENT: &str = "ui:layout";

/// The shell: one `create` frame — a heading, the one control that opens management, the empty space
/// a screen's forms fill, and the notice area that floats above both. A renderer's connection is the
/// event this answers (prism says `ui:connect` and forwards whatever an application answers, ADR-0017
/// §8 — the layout is data on the wire). fluxen renders a bare button as nothing (a button IS a
/// form's submit), so the control is a form with no fields: clicking it submits `{}` to
/// `gravity-model`, which the venue turns into `ui:gravity-model` with this device as the sender.
fn shell(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    // THE ANSWER IS THE ACTION LIST ITSELF, never a `{frames: [...]}` wrapper: prism forwards an
    // array as the renderer's message and would INJECT anything else into a slot named by the
    // event — a page that gets a map draws nothing and reports nothing (ADR-0017 §8).
    emit(ctx, LAYOUT_EVENT, device.as_deref(), json!([frame_root(Vec::new())]))?;
    Ok(json!({ "done": true }))
}

/// The screen's tree: the title, the control that asks for the forms, and the forms themselves (or
/// nothing at all when a renderer connects). One tree for both answers, because a screen and the
/// layout a page connects to are the same screen with different contents.
fn frame_root(forms: Vec<Value>) -> Value {
    json!({
        "action": "create",
        "slot": "",
        "data": {
            "type": "case",
            "attrs": {"class": ["maximize"], "horizontal": true},
            "children": [{
                "type": "case",
                "attrs": {"class": ["main", "shrink", "box", "gap"]},
                "children": [
                    {
                        "type": "case",
                        "attrs": {"class": ["--head", "nogrow", "horizontal", "center"]},
                        "children": [
                            {"type": "text", "bind": {"value": {"kind": "default", "default": "this deployment"}}},
                            {
                                "type": "form",
                                "attrs": {"class": ["nogrow"]},
                                "bind": {"value": {"kind": "dest", "dest": "gravity-model"}},
                                "children": [{
                                    "type": "button",
                                    "attrs": {"oneshot": true},
                                    "bind": {"value": {"kind": "submit", "default": "manage"}},
                                }],
                            },
                        ],
                    },
                    {
                        "type": "case",
                        "attrs": {"class": ["main", "box", "border", "round", "scrolly", "gap"], "scroll": true},
                        "children": forms,
                    },
                    frame_notice(),
                ],
            }],
        },
    })
}

/// THE SLOT WHERE THE DEPLOYMENT SPEAKS TO THE PERSON: one line at a time, floating above the
/// screen, gone when nobody is looking. The frame carries no lifetime — how long a line stays, how
/// it enters, whether it turns over, and what holds it still are the RENDERER's business (rack's
/// `view`, fluxen ADR 0024). The venue only says the line.
const NOTICE_SLOT: &str = "notice";

fn frame_notice() -> Value {
    json!({
        "type": "float",
        "attrs": {"class": ["toast"]},
        "children": [{
            "type": "rack",
            "attrs": {"view": {
                "layout": "exclusive",
                "enter": "fade",
                "scroll": "none",
                "play": "rotate",
                "every": 6000,
                "pause": "click",
            }},
            "bind": {"value": {"kind": "source", "source": NOTICE_SLOT}},
        }],
    })
}

/// One line into the notice area. A row id is required by the vocabulary (the renderer rejects a
/// duplicate), and it has to be unique across the guests that speak — a fresh process per turn — so
/// the id is the clock, not a counter.
fn say_notice(rows: &mut Vec<Value>, message: &str) -> anyhow::Result<()> {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    rows.push(json!({
        "action": "append",
        "slot": NOTICE_SLOT,
        "data": {
            "id": format!("notice-{at}"),
            "type": "case",
            "attrs": {"class": ["box", "border", "round", "shadow", "gap"]},
            "children": [{
                "type": "text",
                "bind": {"value": {"kind": "default", "default": message}},
            }],
        },
    }));
    Ok(())
}

/// The screen: two forms, one per table, prefilled with what the rows hold today (an unconfigured
/// deployment gets an EMPTY form rather than an error — a screen that refuses to draw is one nobody
/// can use to configure anything).
fn screen(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    // THE ANSWER IS THE LAYOUT ITSELF, rebuilt. A screen that appended its rows into a list would
    // stack a second set of forms every time it was asked (the list is keyed by row id, and these
    // rows have none) — so the screen REPLACES the root, exactly as the shell did, and asking twice
    // leaves one screen. The caller names no slot: a click cannot carry one, and this booth owns
    // what a screen looks like anyway.
    let forms = frames_model(ctx)?;
    emit(ctx, SCREEN_EVENT, device.as_deref(), json!([frame_root(forms)]))?;
    Ok(json!({ "done": true }))
}

/// The rows as the screen needs them: whatever is there, `null` when nothing is. Deliberately NOT
/// `read_settings`/`read_recipe` — those refuse by name (a turn must not run on half a deployment),
/// while a screen that cannot read a missing row must still draw the empty form that writes it.
fn rows_for_screen(ctx: &mut Ctx) -> (Value, Value) {
    let settings = ctx
        .store(json!({
            "collection": "settings", "op": "get_document", "key": { "kind": KIND_SETTINGS, "id": 1 },
        }))
        .unwrap_or(Value::Null);
    let model_id = settings
        .get("default_model")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let model = ctx
        .store(json!({
            "collection": "models", "op": "get_document", "key": { "kind": KIND_MODELS, "id": model_id },
        }))
        .unwrap_or(Value::Null);
    (settings, model)
}

/// The frames one screen draws: ONE batch, because a form must exist before anything patches it.
fn frames_model(ctx: &mut Ctx) -> anyhow::Result<Vec<Value>> {
    let (settings, model) = rows_for_screen(ctx);
    let model_id = settings
        .get("default_model")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    Ok(vec![
        frame_form(
            "models",
            &model_id.to_string(),
            "the model this deployment answers with",
            &[
                ("alias", "default", Kind::Text, &model),
                ("endpoint", "https://…/v1", Kind::Text, &model),
                ("model", "model id", Kind::Text, &model),
                ("key_env", "the env var holding the key", Kind::Text, &model),
                ("weight", "1", Kind::Number, &model),
                ("enabled", "1", Kind::Number, &model),
            ],
        ),
        frame_form(
            "settings",
            "1",
            "how this deployment remembers",
            &[
                ("store_url", "http://127.0.0.1:7760", Kind::Text, &settings),
                ("memory_scope", "37", Kind::Number, &settings),
                ("memory_format", "plaintext", Kind::Text, &settings),
                ("default_model", "1", Kind::Number, &settings),
            ],
        ),
    ])
}

/// How a field is typed on the screen: a text input sends a string, a number input a float, and the
/// row's declared width decides which of them encodes (see `coerce_numbers`).
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Number,
}


/// One form. `collection` and `id` are TEXT NODES bound as form fields — static labels that travel
/// with the submit, because a form has no other way to say which row it writes (and an id the
/// operator could retype would be a way to write the wrong row).
fn frame_form(
    collection: &str,
    id: &str,
    heading: &str,
    fields: &[(&str, &str, Kind, &Value)],
) -> Value {
    let mut children = vec![
        json!({"type": "text", "bind": {"value": {"kind": "default", "default": heading}}}),
        json!({"type": "text", "bind": {"value": {"kind": "field", "field": "collection", "default": collection}}}),
        json!({"type": "text", "bind": {"value": {"kind": "field", "field": "id", "default": id}}}),
    ];
    for (name, hint, kind, row) in fields {
        // PREFILL: what the row holds, else the hint the operator reads as an example. A number
        // field takes a JSON number (the renderer's `default_for` reads it as a float).
        let current = row.get(*name).cloned().unwrap_or(Value::Null);
        let default = match (kind, &current) {
            (Kind::Text, Value::String(text)) => Value::String(text.clone()),
            (Kind::Number, Value::Number(_)) => current.clone(),
            (Kind::Number, _) => Value::from(hint.parse::<f64>().unwrap_or(0.0)),
            _ => Value::String(hint.to_string()),
        };
        children.push(json!({
            "type": "input",
            "bind": {"value": {
                "kind": "field",
                "field": name,
                "type": if *kind == Kind::Number { "number" } else { "text" },
                "default": default,
            }},
        }));
    }
    children.push(json!({
        "type": "button",
        "attrs": {"oneshot": true},
        "bind": {"value": {"kind": "submit", "default": "save"}},
    }));
    json!({
        "type": "form",
        // The form's own bind is the uplink: the BARE event name, because a page's uplink carries no
        // marker (prism adds `ui:` on the way in — MODELING's rule).
        "attrs": {"class": ["box", "border", "round", "shadow"]},
        "bind": {"value": {"kind": "dest", "dest": "gravity-model.save"}},
        "children": children,
    })
}

/// THE DOOR A DEPLOYMENT IS CONFIGURED THROUGH (ADR-0007 §4, in the shape that record asks for):
/// the facts are rows in this type's own namespace, so something OUTSIDE the process must be able
/// to write them. That door is an EVENT like the turn's — prism's gate reads the auth declaration
/// above, so only a bound user reaches it — and its answer is SAID, because a handler reached by an
/// event has nothing to return to.
///
/// It is deliberately NARROW: two collections, the row addressed by `id`, and the document checked
/// against the keys a turn reads. A missing key is refused BY NAME here instead of becoming a turn
/// that dies mid-flight with a message about someone else's problem.
fn save(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    let args = payload(envelope);
    match write_row(ctx, args) {
        Ok(saved) => {
            emit(ctx, SAVED_EVENT, device.as_deref(), saved)?;
            Ok(json!({ "done": true }))
        }
        Err(error) => {
            let detail = format!("{error:#}");
            let _ = emit(
                ctx,
                SAVE_REFUSED_EVENT,
                device.as_deref(),
                json!({ "detail": detail }),
            );
            // THE PERSON SEES IT TOO. The fact above is for programs; a page that only got that sees
            // nothing at all (prism injects a non-list payload into a slot nobody binds). So the same
            // refusal rides into the notice slot as a row — the door says the line, the renderer
            // decides how long it stays on screen.
            let mut rows = Vec::new();
            let _ = say_notice(&mut rows, &detail)
                .and_then(|()| emit(ctx, NOTICE_EVENT, device.as_deref(), json!(rows)));
            Err(error)
        }
    }
}

/// Write one deployment row. `settings` is row 1 by construction (one deployment, one settings
/// row); `models` takes whatever id the caller names, so a deployment can hold several recipes.
///
/// TWO SHAPES, ONE RULE: a caller hands the row directly (`{collection, id, doc}` — a script, a
/// test, an AI), or the SCREEN hands the form's own payload, which fluxen builds as
/// `{field: {data, payload}}`. Unwrapping the second here keeps that difference out of the
/// validation and out of every future caller.
fn write_row(ctx: &mut Ctx, args: &Value) -> anyhow::Result<Value> {
    let mut row = args
        .as_object()
        .context("save: the payload is a row document or a form's fields")?
        .clone();
    let explicit = row.remove("doc");
    if explicit.is_none() {
        // The form's shape: every value is a `{data, payload}` message.
        for value in row.values_mut() {
            if let Some(data) = value.get("data").cloned() {
                *value = data;
            }
        }
    }
    let collection = row
        .get("collection")
        .and_then(Value::as_str)
        .context("save: `collection` is `settings` or `models`")?
        .to_string();
    let id = row
        .get("id")
        .and_then(|id| id.as_u64().or_else(|| id.as_str().and_then(|s| s.trim().parse().ok())))
        .context("save: `id` is the row's number (`settings` is 1)")?;
    let kind = match collection.as_str() {
        "settings" => KIND_SETTINGS,
        "models" => KIND_MODELS,
        other => anyhow::bail!(
            "save: `{other}` is not a collection a deployment configures (`settings`, `models`)"
        ),
    };
    // A direct caller's `doc` is used as given; a form's fields ARE the doc, minus the address.
    let mut doc = match explicit {
        Some(doc) => doc,
        None => {
            row.remove("collection");
            row.remove("id");
            Value::Object(row)
        }
    };
    if !doc.is_object() {
        anyhow::bail!("save: `doc` is the row's fields, as an object");
    }
    coerce_numbers(&collection, &mut doc);
    let required: &[&str] = match collection.as_str() {
        "settings" => &["store_url", "memory_scope", "memory_format", "default_model"],
        // Exactly what a turn reads off a recipe: where to post, which model, which env var holds
        // the key (the key itself never lands in a row — ADR-0007 §secrets).
        _ => &["endpoint", "model", "key_env"],
    };
    needs(&doc, &collection, required)?;
    ctx.store(json!({
        "collection": collection,
        "op": "put_document",
        "key": { "kind": kind, "id": id },
        "doc": doc,
    }))?;
    Ok(json!({ "collection": collection, "id": id }))
}

/// The DECLARED U64 fields, made numbers whatever a form sent: an input's text is a string, a
/// `type: number` field is a float, and the row's declared width is 8 bytes — `"37"` does not
/// encode where `37` does. Silence here would be the screen's writes failing on a type the operator
/// never typed.
fn coerce_numbers(collection: &str, doc: &mut Value) {
    let numeric: &[&str] = match collection {
        "settings" => &["memory_scope", "default_model"],
        _ => &["weight", "enabled"],
    };
    for key in numeric {
        let Some(value) = doc.get_mut(*key) else { continue };
        let number = match &*value {
            Value::Number(n) => n
                .as_u64()
                .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as u64)),
            Value::String(text) => text.trim().parse::<u64>().ok(),
            _ => None,
        };
        if let Some(number) = number {
            *value = Value::from(number);
        }
    }
}

/// The keys a row must carry, refused one by one so the caller learns all of them at once.
fn needs(doc: &Value, what: &str, keys: &[&str]) -> anyhow::Result<()> {
    let missing: Vec<&str> = keys
        .iter()
        .copied()
        .filter(|key| doc.get(*key).is_none())
        .collect();
    if !missing.is_empty() {
        anyhow::bail!("save: `{what}` needs {}", missing.join(", "));
    }
    Ok(())
}

/// A provider envelope that closed before any content: its `value` is the error text.
fn stream_failure(envelope: &Value, what: &str) -> anyhow::Error {
    let detail = envelope
        .get("value")
        .and_then(Value::as_str)
        .unwrap_or("no tokens");
    anyhow::anyhow!("{what}: {detail}")
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
/// records it was shown (turn state — one call stack owns it).
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

/// Open the tail branches k10r asked for (ADR-0008 §4): the same prefix the main
/// branch reads, the served prompt in place of this turn's message, the records
/// NUMBERED so the operation can point at one. They run CONCURRENTLY with the
/// answer — the provider pulls are interleaved by the stream face — and every
/// failure is swallowed: a tail operation is maintenance, and a broken branch
/// must never fail a turn. An unlanded compression leaves k10r's node open,
/// which is its "compressing" marker; the next read skips it.
///
/// An operation this guest does not know is SKIPPED with a log, not an error:
/// the store's list can grow before every deployed guest is upgraded, and a
/// version skew must not take a turn down.
fn start_tails(
    ctx: &mut Ctx,
    turn: &Turn<'_>,
    view: &Value,
    prefix: &[Value],
    keys: &[(u64, u16)],
    tails: &mut Vec<Tail>,
) {
    let wanted = view
        .get("tails")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for tail in wanted {
        let Some(name) = tail.get("name").and_then(Value::as_str) else {
            continue;
        };
        if name != OP_COMPRESS && name != OP_REVIEW {
            eprintln!("[gravity-turn] tail operation `{name}` is not known to this guest");
            continue;
        }
        open_branch(ctx, turn, &tail, name, prefix, keys, tails);
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
    turn: &Turn<'_>,
    tail: &Value,
    name: &str,
    prefix: &[Value],
    keys: &[(u64, u16)],
    tails: &mut Vec<Tail>,
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
            "endpoint": turn.endpoint,
            "model": turn.model,
            "key_env": turn.key_env,
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
    tails.push(Tail {
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
fn step_tail(ctx: &mut Ctx, turn: &Turn<'_>, tails: &mut Vec<Tail>) {
    // One round on ONE branch, taking them in turn: the branches are pulled
    // round-robin so a long tail cannot starve the others, and each keeps its own
    // accumulated text (`parts`) across rounds.
    if tails.is_empty() {
        return;
    }
    let mut branch = tails.remove(0);
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
            eprintln!(
                "[gravity-turn] `{}` ended without a product: {err}",
                branch.op
            );
            return;
        }
        let text = branch.parts.concat();
        if text.trim().is_empty() {
            return;
        }
        land_tail(turn, &branch, &text);
        return;
    }
    if let Some(delta) = env
        .get("item")
        .and_then(|item| item.get("delta"))
        .and_then(Value::as_str)
    {
        branch.parts.push(delta.to_string());
    }
    tails.push(branch);
}

/// Drain the live branch before the turn closes. The carrier releases the turn's
/// streams at its end and the guest's state goes with it, so a branch still live
/// here is pulled to completion: the summary must land before the next user
/// message (Phase 3), and the answer has already streamed, so the only wait is
/// the product's own tail. The cap keeps a runaway branch from holding the turn
/// open; what is left unlanded is k10r's open node, which its next read treats
/// as "compressing".
fn drain_tails(ctx: &mut Ctx, turn: &Turn<'_>, tails: &mut Vec<Tail>) {
    for _ in 0..64 {
        if tails.is_empty() {
            return;
        }
        step_tail(ctx, turn, tails);
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
fn land_tail(turn: &Turn<'_>, branch: &Tail, text: &str) {
    match branch.op.as_str() {
        OP_COMPRESS => {
            let (summary, suggested) = read_products(text, &branch.keys);
            land_summary(turn, &summary, suggested);
        }
        OP_REVIEW => match read_review_facts(text) {
            Some(facts) if facts.is_empty() => {
                eprintln!("[gravity-turn] review: nothing worth writing down");
            }
            Some(facts) => land_review(turn, &facts),
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
fn land_review(turn: &Turn<'_>, facts: &[Value]) {
    let scope = turn.memory_scope;
    let url = format!("{}/memories/{scope}", turn.store_url);
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
fn land_summary(turn: &Turn<'_>, summary: &str, suggested: Option<(u64, u16)>) {
    let url = format!(
        "{}/channels/{}/members/{}/checkpoint",
        turn.store_url, turn.channel_id, turn.member_id
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

/// Append the one-ts-two-subs exchange. Reached only after a successful stream: a failed
/// turn appends NOTHING (the user re-sends next turn).
fn append_exchange(turn: &Turn<'_>, user_message: &str, reply: &str) -> anyhow::Result<()> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .context("turn: clock before epoch")?;
    for (sub, role, content) in [(0u16, 0u8, user_message), (1, 1, reply)] {
        let url = format!("{}/channels/{}/messages", turn.store_url, turn.channel_id);
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
    Ok(())
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
    let resp = ureq::get(&format!("{store_url}/tasks/{scope}"))
        .call()
        .ok()?;
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
            let (summary, suggested) =
                read_products(&format!(r#"{{"summary": "s", "cut": {cut}}}"#), &keys());
            assert_eq!(summary, "s");
            assert_eq!(suggested, None, "cut {cut} is not a better cut");
        }
    }

    #[test]
    fn a_fenced_answer_is_still_an_answer() {
        let (summary, cut) =
            read_products("```json\n{\"summary\": \"s\", \"cut\": 1}\n```", &keys());
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
