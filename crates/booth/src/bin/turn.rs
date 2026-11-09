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
//! 3. `gravity_core::llm::live::Live::open` — this process, this thread — and then the
//!    tool rounds and the answer: each envelope item's `delta` leaves as a frame on the
//!    venue wire. A compression or review branch is another `Live` on its own thread.
//! 4. The turn opens WITH its first row (`POST …/messages`, no `turn_id`): the store
//!    written as the turn runs: the question first, one row per SUCCESSFUL tool call when
//!    that round completes, the answer last. The tool rows are the turn's NET EFFECT (a
//!    correction loop's failures stay in the working messages, where the model needed
//!    them, and never reach the store). Each row carries its own clock — gravity's clock
//!    IS the channel's time (krystallizer ADR-0008 §3) — and `sub` is the
//!    same-millisecond tiebreak inside one turn.
//! 5. On success the turn CLOSES with one `kind = FINAL` record naming the rows to keep;
//!    k10r purges the rest (k10r ADR-0024).
//!
//! Failure is a value (ADR-0012): the handler's answer carries the error text, and a
//! drawing caller is told on the same wire (`ui:turn.error`). A failed turn writes no
//! closing record — its rows stay OPEN, working state the projection skips.
//!
//! Args (the envelope's `data`, or the flat call of a direct caller):
//! `{"channel_id", "user_message", "store_url", "endpoint", "model",
//! "key_env"}`, plus the output shape — `protocol: "fluxen"` with `slots {rack, text}`,
//! or `event` naming the event raw deltas are spoken as. The envelope's OWN `channel_id`
//! is an ADDRESS (a device, text) and is a different fact from the conversation's
//! conversation's own ids inside `data`. The venue addresses DEVICES, never channels (prism
//! ADR-0018: a channel is an application's business, the transport knows devices); the turn
//! answers the device that asked and names no grouping.

use anyhow::Context;
use gravity_booth::tools::Doors;
use gravity_core::surface::{Kind, MemoryFormat, Role};
use induction_guest::{Ctx, Guest};
use serde_json::{json, Value};

fn main() -> anyhow::Result<()> {
    Guest::new()
        .named("gravity-turn")
        // Rounds OVERLAP inside this one process (ADR-0046): the host answers each
        // reply by its request id, so a device's next message is served while the
        // previous turn is still streaming. The state a round needs is its own
        // stack and its own streams — nothing is shared between rounds.
        .concurrent(true)
        // AUTH IS NOT DECLARED HERE: the guest names ENTRY points (`turn`, `save`), and the
        // event each one serves is bound host-side (`gravity_booth::register`'s `on_as`), which
        // is where `.gated()` sits. The event name is written once, at the declaration that
        // knows it — a table here keyed by event would be the same name written twice.
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
        "declared_index_base": 4096, "declared_reduce_base": 8192,
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
/// The turn's own opening receipt: `{turn}` the moment the store minted it. It is
/// what lets a client tag the sentence it just sent with the id every later frame
/// will carry — without it the user's own message has no key into its answer, and
/// two turns of one conversation cannot be untangled at the point they start (the
/// reason the induction wire carries `call`, one layer down).
const OPENED_EVENT: &str = "ui:turn.opened";

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

/// The list a turn appends its bubbles to. THE APPLICATION NAMES IT: it serves the layout
/// that declares `messages`, so it does not ask a caller what its own slot is called — and a
/// caller that had to answer would be speaking the renderer's vocabulary (`rack`, `text`)
/// about a business object.
const MESSAGES_SLOT: &str = "messages";

/// What the caller asked its chunks to BE. Both shapes ride `ctx.emit`; the difference is
/// whether a renderer draws them or a consumer reads them.
enum Out<'a> {
    /// Accrete frames: bubbles appended to `messages`, keyed by `<turn>_<n>`.
    View,
    /// Raw deltas at the event the caller named (`event`). Gravity owns no event names:
    /// a consumer that wants `{"delta": …}` says what it wants it called.
    Data(&'a str),
}

/// Whether a turn DRAWS its answer, decided in ONE place for both the frames and the end.
///
/// Three inputs, and the order matters. An explicit `protocol` is the caller's word. Failing
/// that, an `event` is a request for deltas — a program that names the event its chunks are
/// spoken as has already said which of the two it wants, and the CLI and prism's turn test send
/// exactly that. With neither marker the turn DRAWS, because a page is the one caller that
/// cannot say so: a form's uplink carries its fields and nothing else (fluxen builds
/// `{field: {data, payload}}`), so demanding a protocol of it would mean a hidden field whose
/// only job is to tell the application what kind of surface is looking at it.
fn draws(args: &Value) -> bool {
    match args.get("protocol").and_then(Value::as_str) {
        Some("fluxen") => true,
        Some("data") => false,
        _ => args.get("event").and_then(Value::as_str).is_none(),
    }
}

fn out_of(args: &Value) -> anyhow::Result<Out<'_>> {
    if draws(args) {
        return Ok(Out::View);
    }
    Ok(Out::Data(
        args.get("event")
            .and_then(Value::as_str)
            .context("turn: a delta consumer must name the `event` its chunks are spoken as")?,
    ))
}

/// One venue emit. `name` is the recipient event — a compiled view body is not told which
/// event fired (the concrete name is consumed by the dispatcher), so the emitter states
/// it; `device` is the recipient (one device: an application that has groups would name
/// more, but this one has no channel to be a member of); the payload hangs off `data`,
/// which is the shape the venue reads as "a delivery, not a request".
fn emit(
    ctx: &mut Ctx,
    event: &str,
    device: Option<&str>,
    turn: Option<&str>,
    data: Value,
) -> anyhow::Result<()> {
    let mut envelope = json!({ "name": event });
    if let Some(turn) = turn {
        // WHICH turn this chunk belongs to. A device may have several in flight — the type
        // declares `unbounded` (aura ADR-0046) — and every chunk of every turn arrives at the
        // same recipient, so without this a second answer interleaves into the first and the
        // client cannot untangle them. The induction wire learned the same lesson one layer
        // down (`host.call`, induction ADR-0002).
        envelope["turn"] = json!(turn);
    }
    if let Some(device) = device {
        // NO `sender` on a booth's own speech: the venue excludes the sender from its
        // fan-out, so marking the asking device as the sender would send the answer to
        // everyone EXCEPT the one connection that wants it.
        envelope["device"] = json!(device);
    }
    envelope["data"] = data;
    ctx.emit(event, envelope)
}


/// One bubble in the rack whose text child reads the streaming region — so a patch of the
/// The user's own line, so the rack shows the question above the answer.
/// One bubble, appended to the conversation. The row carries its own identity (`data.id`)
/// and a `selector` the LAYOUT's item template matches on — appearance is the template's
/// business, so no class list appears here (fluxen ADR 0020/0024).
fn frame_message(id: &str, selector: &str, text: &str) -> Value {
    json!({
        "action": "append",
        "slot": MESSAGES_SLOT,
        "data": {
            "id": id,
            "type": "case",
            "attrs": { "selector": selector },
            "children": [{
                "type": "text",
                "bind": { "value": { "kind": "default", "default": text } },
            }],
        },
    })
}

/// One chunk into the bubble `id` — ADDRESSED BY ROW, not by a shared slot: two turns in
/// flight patch their own bubbles and cannot interleave.
fn frame_chunk(id: &str, chunk: &str) -> Value {
    json!({
        "action": "patch",
        "slot": MESSAGES_SLOT,
        "id": id,
        "path": "/children/0/bind/value/default",
        "op": "append",
        "value": chunk,
    })
}

/// The id of the `n`-th bubble of one turn: `<turn>_0` is the person's message, `<turn>_1`
/// the reply (tool calls fold into it), and anything that owns a bubble of its own — a
/// confirmation form, say — continues the count. The turn id stays the grouping key.
fn bubble_id(turn_id: &str, n: u8) -> String {
    format!("{turn_id}_{n}")
}

/// What the turn draws the moment it OPENS: the person's own message, as a bubble with the
/// turn as its key. It leaves with the receipt, not with the first token — a UI holds no
/// logic of its own (ADR-0010), so "the person's message appears when they send it" is a
/// frame the application owes, and waiting for the first token made a send look like nothing
/// had happened. The reply's bubble is NOT created here: one that never fills would leave an
/// empty bubble on screen when a turn fails.
fn frames_open(turn_id: &str, ask: Option<&str>) -> Vec<Value> {
    ask.map(|ask| vec![frame_message(&bubble_id(turn_id, 0), "user", ask)])
        .unwrap_or_default()
}

/// One chunk draws: the reply's bubble (once, id `<turn>_1`) and then the text in it.
fn frames_chunk(turn_id: &str, first: bool, chunk: &str) -> Vec<Value> {
    let id = bubble_id(turn_id, 1);
    let mut frames = Vec::new();
    if first {
        frames.push(frame_message(&id, "assistant", ""));
    }
    frames.push(frame_chunk(&id, chunk));
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

/// ONE field, TWO shapes: a page's form sends every field as `{data, payload}` (fluxen's message
/// envelope, the same shape `write_row` unwraps for the deployment forms), while a CLI, a sibling
/// booth or a test sends the bare string. Unwrapping here is what lets the composer's own words
/// reach a turn that a direct caller reaches by typing them — read this and the settings form's
/// unwrap as the same rule, not two exceptions.
fn text_field(args: &Value, name: &str) -> Option<String> {
    let value = args.get(name)?;
    match value.get("data").unwrap_or(value) {
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
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

/// WHO is behind this connection, as the venue stamped it. Absent when the socket is anonymous —
/// the caller decides whether that is an error (the turn says so by name).
fn sender_user(envelope: &Value) -> Option<String> {
    envelope
        .get("sender")
        .and_then(|sender| sender.get("user"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// What one turn produced.
struct Turned {
    /// The answer that streamed. A cancelled turn's partial reply goes back to a DIRECT caller
    /// and lands nowhere: the log is never closed, so the projection never sees it.
    reply: String,
    /// The turn stopped on the cancel marker (ADR-0008). The caller is told why on the same wire
    /// it was drawing on.
    cancelled: bool,
}

/// How a streamed answer ended.
enum Ending {
    /// The provider said `done`: the answer is whole and the turn closes its log.
    Done(String),
    /// The cancel marker was seen at a chunk boundary. Nothing closes.
    Cancelled,
}

fn run(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    let user = sender_user(envelope);
    let args = payload(envelope);
    let drawing = draws(args);
    // AN EVENT-ROUTED SUBMIT HAS NO ANSWER IT CAN READ: a delivery discards a return value, so its
    // end is SAID, not returned. A direct caller (iterate/call — no envelope `name`) still reads the
    // terminal value, which is what ADR-0009 states for the data protocol. The drawing caller is
    // always event-routed, and it needs the end regardless: its submit was an emit.
    let said = drawing || envelope.get("name").is_some();
    // The store mints the turn id when the log opens; the branch hands it back here so
    // the end frames carry the same one the chunks did.
    let minted: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
    match turn(ctx, args, device.as_deref(), user.as_deref(), &minted) {
        Ok(turned) => {
            if said {
                // The id rides INSIDE the payload as well as on the envelope: a venue hands a
                // client the payload (`{ev, args}`), so an id written only beside it never
                // arrives (prism ui.rs: the body is `data`).
                let id = minted.borrow().clone();
                let mut data = json!({});
                if let Some(id) = &id {
                    data["turn"] = json!(id);
                }
                // THE REASON RIDES THE SAME EVENT (ADR-0008 point 4): a `done` that says why it
                // ended — no new vocabulary, and no `cancelled: true` flag for a consumer to forget.
                if turned.cancelled {
                    data["reason"] = json!("cancelled");
                }
                emit(ctx, DONE_EVENT, device.as_deref(), id.as_deref(), data)?;
            }
            Ok(json!({ "done": true, "value": { "reply": turned.reply } }))
        }
        Err(error) => {
            if said {
                let detail = format!("{error:#}");
                let id = minted.borrow().clone();
                let mut data = json!({ "detail": detail });
                if let Some(id) = &id {
                    data["turn"] = json!(id);
                }
                let _ = emit(ctx, ERROR_EVENT, device.as_deref(), id.as_deref(), data);
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
    /// `<channel>:<turn_id>` — the id the STORE minted when this turn's log opened (k10r
    /// owns the number: a caller-side one would collide across devices, retries and two
    /// turns opened at once), prefixed by its channel because the store mints per
    /// channel. Every chunk of this turn carries it, so a client can put two answers
    /// arriving at one device back together and find the same pair in the log keys.
    /// Empty until the log is open.
    id: String,
    /// Already trailing-slash-trimmed.
    store_url: String,
    channel_id: u64,
    /// WHO is speaking — the venue's sender envelope, never a form field: a page has no way to
    /// claim another person (prism ADR-0018, ADR-0017 §7 amendment). k10r's member id is the
    /// user's uuid as text.
    member_id: String,
    memory_scope: u64,
    /// The head's representation — the deployment's choice, stored beside the other facts.
    memory_format: String,
    /// The model recipe the settings row named.
    endpoint: String,
    model: String,
    key_env: String,
}

fn turn(
    ctx: &mut Ctx,
    args: &Value,
    device: Option<&str>,
    user: Option<&str>,
    minted: &std::cell::RefCell<Option<String>>,
) -> anyhow::Result<Turned> {
    // THE DEPLOYMENT FACTS ARE ROWS, NOT ARGUMENTS (ADR-0007): the store, the memory scope, the
    // representation and the model recipe live in this type's own namespace, so a gateway never
    // carries them and a caller cannot forget one.
    let settings = read_settings(ctx)?;
    let recipe = read_recipe(ctx, settings.default_model)?;
    let store_url = settings.store_url.trim_end_matches('/').to_string();
    // WHICH conversation. A caller that knows says so; a page does not, because the channel is
    // the application's business (prism ADR-0018) and the deployment serves one person. The
    // fallback is the member's OWN first channel — the same query a chat view makes when it
    // builds, asked from the side that needs the answer.
    let channel_id = match args.get("channel_id").and_then(Value::as_u64) {
        Some(channel_id) => channel_id,
        None => {
            let member = user.or_else(|| args.get("member_id").and_then(Value::as_str));
            let member = member.context("turn: no `channel_id` and no member to look one up for")?;
            first_channel_of(&store_url, member)?
        }
    };
    // WHO is speaking. A PAGE has no field for it — the venue mounts `sender {device, user}`
    // into every page event, and k10r's member id IS the user's uuid (k10r ADR-0025), so the
    // answer comes from the transport, never from a form. A DIRECT caller (the CLI, a sibling
    // booth, the test seam) has no envelope to stamp, and it names the member itself: that is
    // the caller declaring its own identity, not a page claiming another's.
    let member_id = match user {
        Some(user) => user.to_string(),
        None => args
            .get("member_id")
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())))
            .context("turn: no `sender.user` on the envelope and no `member_id` in the args —                       a page is identified by the venue, a direct caller names its member")?,
    };
    let user_message = text_field(args, "user_message")
        .context("turn: missing `user_message`")?;
    let mut turn = Turn {
        args,
        // Filled in below, the moment the store mints one: a turn that never opens its
        // log (a refusal before the first row) has no id, and no chunk to mislabel.
        id: String::new(),
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
    // here: it returns the fact DATA, which this booth does not assemble.
    let format = MemoryFormat::from_config(Some(turn.memory_format.as_str()))
        .context("turn: the configured memory format is unusable")?;
    let url = format!(
        "{store_url}/channels/{channel_id}/members/{}/session?format={}",
        turn.member_id,
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
    //
    // The same rows answer a second question the loop asks: which code rows
    // finish which tasks (ADR-0003 §3). Fetched ONCE here, so the dispatch
    // below reads a local table and never the store.
    let owed = fetch_tasks(&turn.store_url, turn.memory_scope);
    if let Some(block) = owed.as_deref().and_then(gravity_booth::tasks::render_owed) {
        messages.push(json!({ "role": "system", "content": block }));
    }
    let ends_on = gravity_booth::tasks::bindings(owed.as_deref().unwrap_or_default());
    // The tail branches see the same records, NUMBERED: an operation that must
    // point at one (the compression's cut) answers with an index, because the
    // store owns positions and a model cannot see them.
    let mut numbered: Vec<Value> = Vec::new();
    // A position in the log is a POINTER — `(turn, ts, sub)` since k10r ADR-0024:
    // the turn comes before the millisecond, so two turns cannot be confused.
    let mut keys: Vec<Pointer> = Vec::new();
    for (index, entry) in view
        .get("increment")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .enumerate()
    {
        // The entry is FLAT and `role` is the resolved NAME — k10r's mapping, the
        // one place a code becomes a label. `null` means a stored code that
        // mapping does not know (a writer's bug: k10r does not police codes and
        // will not guess). The row happened, so dropping it would hide history;
        // naming it `user` would put words in the user's mouth. `system` says
        // exactly what is true: a record whose speaker cannot be named.
        let role = entry.get("role").and_then(Value::as_str).unwrap_or("system");
        let content = entry
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        messages.push(json!({ "role": role, "content": content }));
        if let Some(key) = entry.get("key") {
            let (Some(turn), Some(ts), Some(sub)) = (
                key.get("turn_id").and_then(Value::as_u64),
                key.get("ts_ms").and_then(Value::as_u64),
                key.get("sub").and_then(Value::as_u64),
            ) else {
                continue;
            };
            keys.push(Pointer {
                turn_id: turn,
                ts_ms: ts,
                sub: sub as u16,
            });
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
    start_tails(&turn, &view, &branch_prefix, &keys, &mut tails, ctx.cancel());

    // 4. The provider: open, run the tool rounds, and land on a CONTENT stream. A
    //    `tool_calls`-first reply = the model wants tools BEFORE any prose: dispatch them,
    //    feed the results back, re-open — the "next inference round" (Phase 2). The results
    //    are the MODEL's, not the consumer's, so they never ride the drawing wire.
    let mut walked: Vec<gravity_booth::tools::Walked> = Vec::new();
    // The NET EFFECT of the tool rounds, for the history: only a call that came
    // back OK is here, so a correction loop leaves its final success and nothing
    // else. The loop's failures stay in the working messages above — the model
    // needed them to fix its call, a later turn does not.
    let mut tool_events: Vec<ToolEvent> = Vec::new();
    // The turn OPENS in the store before the first inference (k10r ADR-0024): the
    // question lands as its first row. A turn that is suspended on a cold call, or
    // that dies, therefore leaves a working set a resumed turn can read back —
    // and a turn still running is OPEN, so no one else's projection sees it.
    // The question OPENS the turn: one write, and the store's answer names the id.
    let (mut log, question) = TurnLog::open(
        &turn.store_url,
        channel_id,
        &user_message,
        Role::User,
        Kind::Text,
    )?;
    // The turn's identity on the wire is the log key's first two segments: an id is
    // minted PER CHANNEL, so a bare number names a different conversation's turn in
    // every channel and two of them in flight at one device would still collide.
    turn.id = format!("{channel_id}:{}", log.turn_id);
    *minted.borrow_mut() = Some(turn.id.clone());
    // The receipt: the id exists now, and whoever sent the message needs it BEFORE
    // the first delta to key that message to its answer.
    emit(
        ctx,
        OPENED_EVENT,
        device,
        Some(&turn.id),
        json!({ "turn": turn.id }),
    )?;
    // And the stage: the person's message, the region its answer will fill, and the
    // bubble that reads it. The question is in `args` from the start, so a renderer that
    // knows nothing but operations can draw the send immediately.
    if matches!(out, Out::View) {
        let ask = text_field(turn.args, "user_message");
        let frames = frames_open(&turn.id, ask.as_deref());
        if !frames.is_empty() {
            emit(ctx, UI_EVENT, device, Some(&turn.id), json!(frames))?;
        }
    }
    // The rows of the calls that SUCCEEDED, in order: the keep set's middle.
    let mut call_rows: Vec<(u64, u16)> = Vec::new();
    let mut pump = Pump::open(&turn, &messages);
    // A stop can land between the open and the first token: the connect is not a
    // boundary anyone reads, so the wait for it is one.
    // A stop before the first token ends the turn the same way a stop mid-stream does:
    // nothing lands, the log keeps its rows OPEN, and the caller is told `cancelled`.
    let Some(opened) = pump.next(ctx).context("turn: provider stream failed to open")? else {
        return Ok(Turned {
            reply: String::new(),
            cancelled: true,
        });
    };
    let mut first = opened;
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
        let session = format!("channel-{channel_id}-member-{}", turn.member_id);
        // The COLD path (PLAN Phase 2.5), before the round is dispatched: a
        // `human`-nature call touches the outside world and may not answer for
        // minutes, so the transcript stops carrying payloads it has already used.
        // Finished rounds collapse into net-effect lines; the round about to run
        // stays raw, because the results of THAT round are what the next
        // inference needs. `fast` rounds never trigger it — zero flushes unless
        // the declaration says the call is cold.
        if parsed
            .iter()
            .any(|call| gravity_booth::tools::nature_of(&call.function.name)
                == Some(gravity_booth::tools::Nature::Human))
        {
            compact_finished_rounds(&mut messages);
        }
        // The call round itself, once: the wire shape of "the assistant asked
        // for these N tools", which the results below answer one by one.
        messages.push(json!({ "role": "assistant", "tool_calls": calls }));
        for call in parsed {
            let facts = gravity_booth::tools::Facts {
                store_url: &turn.store_url,
                memory_scope: Some(turn.memory_scope),
                mounts: &mounts,
                session: &session,
                ends_on: &ends_on,
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
                    Ok(value) => {
                        // One record per call that SUCCEEDED. The same call twice
                        // (a model that re-asks rather than moving on) keeps only
                        // the last answer: what the store owes the next turn is
                        // the effect, not the retries.
                        let event = ToolEvent {
                            tool: call.function.name.clone(),
                            args: call.function.arguments.to_string(),
                            result: value.to_string(),
                        };
                        // ADR-0024: the round lands in the store as it completed, not
                        // at the end of the turn — the same net-effect record the
                        // history keeps, written where a resumed turn can read it.
                        let line = render_tool_event(&event);
                        call_rows.push(log.row(Role::System, Kind::ToolCall, &line)?);
                        match tool_events.last_mut() {
                            Some(last)
                                if last.tool == event.tool && last.args == event.args =>
                            {
                                *last = event;
                            }
                            _ => tool_events.push(event),
                        }
                        value.to_string()
                    }
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
        pump = Pump::open(&turn, &messages);
        let Some(reopened) = pump.next(ctx).context("turn: provider re-open failed")? else {
            return Ok(Turned {
                reply: String::new(),
                cancelled: true,
            });
        };
        first = reopened;
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
    let delta = first
        .get("item")
        .and_then(|i| i.get("delta"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // 5. The answer, one emit per chunk, with a live side branch advanced between pulls.
    let streamed = stream_answer(ctx, &out, device, &pump, delta, &turn, &mut tails);
    // The bit is this ACTIVATION's and it is spent either way — finished, stopped, or failed.
    // Clearing here is what keeps a stop from ending the next turn of the same instance.
    ctx.stop_clear()?;
    let reply = match streamed {
        Ok(Ending::Done(reply)) => reply,
        // A CANCELLED TURN CLOSES NO TURN (ADR-0008 point 4): the summary's tail is maintenance
        // for a turn that will not be recorded, so the branches are dropped rather than drained,
        // and the log keeps its rows OPEN — k10r's projection skips them (ADR-0024).
        Ok(Ending::Cancelled) => {
            tails.clear();
            return Ok(Turned {
                reply: String::new(),
                cancelled: true,
            });
        }
        Err(error) => {
            // Nothing lands and nothing appends: the branches were maintenance. Their
            // threads own their own streams; dropping the receivers is the whole cleanup.
            tails.clear();
            return Err(error);
        }
    };
    // The answer has streamed; the summary's tail is the only thing still owed (see
    // drain_tails).
    drain_tails(&turn, &mut tails);

    // 6. The exchange, only now: one ts, subs 0/1. Gravity's clock IS the channel's time
    //    (krystallizer ADR-0008 §3).
    if reply.is_empty() {
        // No answer, no closing record: the rows already written stay OPEN, and
        // the projection skips them — a failure costs visibility, not correctness.
        anyhow::bail!("turn: provider stream yielded no tokens");
    }
    let answer = log.row(Role::Assistant, Kind::Text, &reply)?;
    let mut keep = vec![question];
    keep.extend(call_rows);
    keep.push(answer);
    log.close(&keep)?;
    // The field on the left shows what the turn just did: the turn as a node, everything it
    // wrote hanging off it. A whole SNAPSHOT, because that is how a forcegraph is fed
    // (fluxen ADR 0018) — it diffs by node id, so resending is idempotent.
    if matches!(out, Out::View) {
        let mut nodes = vec![json!({
            "id": turn.id,
            "label": turn.id,
            "kind": "turn",
        })];
        let mut links = Vec::new();
        for (n, (ts, sub)) in keep.iter().enumerate() {
            let id = bubble_id(&turn.id, n as u8);
            nodes.push(json!({ "id": id, "label": format!("{ts}:{sub}"), "kind": "row" }));
            links.push(json!({ "source": turn.id, "target": id, "kind": "wrote" }));
        }
        emit(
            ctx,
            UI_EVENT,
            device,
            Some(&turn.id),
            json!([{ "action": "set", "slot": SCENE_SLOT, "data": { "nodes": nodes, "links": links } }]),
        )?;
    }
    Ok(Turned {
        reply,
        cancelled: false,
    })
}

/// The provider stream, off this thread (ADR-0047 stage two, for a blocking client).
///
/// `Live::pull` is a blocking `read` on the provider's socket. On this thread it is the
/// one thing a stop cannot reach: a boundary BETWEEN chunks is no help while you sit
/// inside one, and a provider that goes quiet holds the turn hostage for as long as it
/// likes. So the stream moves to a worker and the turn waits with a deadline — a parked
/// turn becomes one that is parked for at most a tick.
///
/// The worker owns `Live`; the turn keeps only the receiving end. Dropping the turn's
/// end is what tells the worker nobody is listening any more — it exits at its next
/// send, which happens when the blocking read returns. Nothing waits for that: an
/// abandoned read costs one thread, and the process that owns it is the one being
/// stopped.
struct Pump {
    rx: std::sync::mpsc::Receiver<anyhow::Result<Value>>,
}

impl Pump {
    /// One tick of the wait: short enough that a click feels like a click, long enough
    /// that an idle stream costs nothing.
    const TICK: std::time::Duration = std::time::Duration::from_millis(50);

    /// Open the stream ON the worker, not before it. The connect is the longest
    /// stretch with no boundary in it (measured against a provider that is slow before
    /// its first byte: 1.5 s), and starting the worker after it left that stretch
    /// unwatched — an interrupt raised during the open landed on a token nobody read.
    /// The open's own failure travels the channel like any other, so there is one way
    /// to learn what went wrong rather than two.
    fn open(turn: &Turn<'_>, messages: &[Value]) -> Self {
        let endpoint = turn.endpoint.clone();
        let model = turn.model.clone();
        let key_env = turn.key_env.clone();
        let messages = json!(messages);
        let tools = gravity_booth::tools::tools_array();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let opened = gravity_core::llm::live::Live::open(&gravity_core::llm::live::Request {
                endpoint: &endpoint,
                model: &model,
                key_env: &key_env,
                messages: &messages,
                tools: Some(&tools),
            });
            let mut live = match opened {
                Ok(live) => live,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    return;
                }
            };
            loop {
                let out = live.pull();
                // An error ends the stream: the next pull would answer the same thing.
                let fatal = out.is_err();
                if tx.send(out).is_err() || fatal {
                    break;
                }
            }
        });
        Self { rx }
    }

    /// One envelope. `Ok(None)` = this turn was stopped while waiting, which the caller
    /// turns into `Ending::Cancelled` — the same ending the per-chunk bit produces, from
    /// the one place the bit could not reach before.
    fn next(&self, ctx: &Ctx) -> anyhow::Result<Option<Value>> {
        loop {
            match self.rx.recv_timeout(Self::TICK) {
                Ok(out) => return out.map(Some),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if ctx.is_cancelled() {
                        return Ok(None);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!(
                    "turn: provider stream ended without an envelope"
                ),
            }
        }
    }
}

/// One pull round of the answer stream: emit the chunk, advance a live side branch, pull
/// the next envelope. Returns the assembled reply at the provider's `done`.
fn stream_answer(
    ctx: &mut Ctx,
    out: &Out<'_>,
    device: Option<&str>,
    pump: &Pump,
    mut delta: String,
    turn: &Turn<'_>,
    tails: &mut Vec<Tail>,
) -> anyhow::Result<Ending> {
    let mut reply = String::new();
    let mut first = true;
    loop {
        match out {
            Out::View => {
                emit(
                    ctx,
                    UI_EVENT,
                    device,
                    Some(&turn.id),
                    json!(frames_chunk(&turn.id, first, &delta)),
                )?;
            }
            Out::Data(event) => {
                emit(ctx, event, device, Some(&turn.id), json!({ "delta": delta }))?
            }
        }
        first = false;
        reply.push_str(&delta);

        // The side branch advances one pull per round, on the same face: the summary is
        // produced alongside the answer, never in front of it.
        step_tail(tails, turn);
        // THE STOP BOUNDARY (ADR-0008 as revised; ADR-0047 stage one): one BIT read per chunk —
        // a boundary this loop already had, not an interruption bolted onto a pull. The bit
        // belongs to this activation and is set by whoever holds authority over the connection
        // (the venue), never delivered as an event: a delivery to a busy instance queues behind
        // the very work it means to stop.
        if ctx.stop_poll()? {
            return Ok(Ending::Cancelled);
        }
        // The pull itself is the boundary now: it returns when the provider sends, or
        // as soon as the stop is raised — a blocking `read` on this thread could not.
        let Some(env) = pump.next(ctx).context("turn: provider pull failed")? else {
            return Ok(Ending::Cancelled);
        };
        if env.get("done").and_then(Value::as_bool) == Some(true) {
            return Ok(Ending::Done(reply));
        }
        delta = env
            .get("item")
            .and_then(|item| item.get("delta"))
            .and_then(Value::as_str)
            .context("turn: a non-terminal provider envelope carried no delta")?
            .to_string();
    }
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
    // The management page ships WITH its forms: the layout is rebuilt on every save anyway
    // (`screen`), so there is no separate "load the forms" step to trigger.
    let forms = frames_model(ctx).unwrap_or_default();
    emit(ctx, LAYOUT_EVENT, device.as_deref(), None, json!(frame_root(forms)))?;
    Ok(json!({ "done": true }))
}

/// The screen's tree: an OUTER FRAME (a navigation bar over a set of PAGES), never a page with
/// everything stacked in one column. Two pages live in the frame — home (the field and the
/// conversation) and management (the model's forms) — and the bar switches between them.
///
/// Management is its own page rather than a panel over the conversation, and the pager keeps
/// every page MOUNTED (`display: dom`): a turn streams into `messages` while a person may be
/// filling a form, and a pager that unmounted the home page would leave those frames naming a
/// slot the layout no longer has.
fn frame_root(forms: Vec<Value>) -> Vec<Value> {
    vec![
        json!({
            "action": "create",
            "slot": "",
            "data": {
                "type": "case",
                "attrs": {"class": ["maximize", "col"]},
                "children": [frame_navbar(), frame_pager(), frame_auth(), frame_notice()],
            },
        }),
        // The pages, as a dictionary: a row's `id` IS its key, and the bar selects by it.
        json!({
            "action": "append",
            "slot": PAGES_SLOT,
            "data": {
                "id": "home",
                "type": "case",
                // THE SPLIT THE EXAMPLE USES (examples/yaml/00.main.yaml): a horizontal case with
                // `stage-split`, a `stage-graph` column on the left and a `stage-stream` column on
                // the right. The widths are the bundle's own CSS — graph `1 1 40%`, stream
                // `1 0 620px`, and under 1000px the graph pins to 240px — which is what makes the
                // GRAPH absorb the compression while the stream keeps its width.
                "attrs": {"class": ["maximize", "stage-split", "shrink"], "horizontal": true},
                "children": [frame_scene(), frame_stream()],
            },
        }),
        json!({
            "action": "append",
            "slot": PAGES_SLOT,
            "data": {
                "id": "manage",
                "type": "case",
                "attrs": {"class": ["main", "col", "gap", "scrolly"]},
                "children": forms,
            },
        }),
        // The bar's items: a short label per page, same ids.
        json!({
            "action": "append",
            "slot": TABS_SLOT,
            "data": {
                "id": "home",
                "type": "text",
                "bind": {"value": {"kind": "default", "default": "对话"}},
            },
        }),
        json!({
            "action": "append",
            "slot": TABS_SLOT,
            "data": {
                "id": "manage",
                "type": "text",
                "bind": {"value": {"kind": "default", "default": "管理"}},
            },
        }),
    ]
}

/// The navigation bar: the deployment's name and the page switch. Clicking an item is LOCAL —
/// the pager reads the same local slot — so switching pages costs no round trip.
fn frame_navbar() -> Value {
    json!({
        "type": "case",
        "attrs": {"class": ["--head", "nogrow", "horizontal", "center", "box", "gap"]},
        "children": [
            {
                "type": "text",
                "attrs": {"class": ["nogrow"]},
                "bind": {"value": {"kind": "default", "default": "this deployment"}},
            },
            {
                "type": "select",
                "attrs": {"horizontal": true},
                "bind": {
                    "value": {
                        "kind": "local",
                        "slot": PAGE_SLOT,
                        "path": "/data",
                        "default": "home",
                    },
                    "options": {"kind": "source", "source": TABS_SLOT},
                },
            },
            // THE PLATFORM'S ONE CONTROL: it says only "open the region" — prism's auth booth draws
            // whatever goes in it, so the application never holds a login form (ADR-0017 §2).
            {
                "type": "form",
                "attrs": {"class": ["nogrow"]},
                "bind": {"value": {"kind": "dest", "dest": "auth.request"}},
                "children": [{
                    "type": "button",
                    "attrs": {"oneshot": true},
                    "bind": {"value": {"kind": "submit", "default": "登录"}},
                }],
            },
        ],
    })
}

/// The pager: every page stays mounted (`dom`), so a conversation being streamed into one page
/// survives a look at another.
fn frame_pager() -> Value {
    json!({
        "type": "pages",
        "attrs": {"class": ["main", "shrink"]},
        "bind": {
            "value": {"kind": "source", "source": PAGES_SLOT},
            "select": {"kind": "local", "slot": PAGE_SLOT, "path": "/data", "default": "home"},
        },
        "display": "dom",
    })
}

/// THE LEFT AREA: the scene is a forcegraph, and what it draws is data the deployment pushes
/// into `scene` as `{nodes, links, focus?}` (fluxen ADR 0018 — a native node, not a canvas
/// module: the layout lives in wasm, no URL, no options bag). Below it a menu, in a rack, so
/// the same column that shows the field of things also lists the ways into them — the shape a
/// channel list would take in a conventional application.
fn frame_scene() -> Value {
    json!({
        "type": "case",
        "attrs": {"class": ["stage-graph", "col", "gap"], "horizontal": false},
        "children": [
            {
                "type": "forcegraph",
                // `100%` of a box that has a height: the graph's own case grows (class `main`), so
                // the percentage resolves against a real size — the example's shape, verbatim.
                "attrs": {"class": ["main"], "width": "100%", "height": "100%"},
                "style": {
                    "node_size_ratio": 2,
                    "link_width_px": 2.5,
                    "link_opacity": 0.85,
                    "highlight_reach": true,
                    "reach_hops": 1,
                    "fit_padding_px": 56,
                    "link_shape": "straight",
                },
                "bind": {"value": {"kind": "source", "source": SCENE_SLOT}},
            },
            frame_menu(),
        ],
    })
}

/// The menu under the scene: a rack over `menu`, rows appended by the deployment. Its row
/// template is a line of text for now — what a row DOES when clicked is a decision the
/// deployment owns (an event name, a channel to open), and the shell will not invent one.
fn frame_menu() -> Value {
    json!({
        "type": "rack",
        "attrs": {
            "class": ["nogrow", "box", "border", "round", "gap"],
            "view": {"layout": "adaptive", "scroll": "bar"},
        },
        "bind": {"value": {"kind": "source", "source": MENU_SLOT}},
        "item": [{
            "type": "case",
            "attrs": {"class": ["nogrow", "horizontal", "center", "gap"]},
            "children": [{
                "type": "text",
                "bind": {"value": {"kind": "default", "default": ""}},
            }],
        }],
    })
}

/// THE CONVERSATION: a rack over `messages`, with the two bubbles declared ONCE as item
/// templates. A row appended to `messages` carries a `selector` and the rack indexes these by
/// it (fluxen rack.rs), so the application decides neither alignment nor accent — it sends rows
/// and says what each one IS. `user` is the accented, end-aligned bubble; the second template
/// carries no selector and is the fallback, which is what the reply and anything else lands on.
fn frame_stream() -> Value {
    json!({
        "type": "case",
        "attrs": {"class": ["stage-stream", "main", "shrink", "col"]},
        "children": [frame_messages(), frame_composer()],
    })
}

/// THE COMPOSER: the one place a person's words enter. A form like any other — the uplink is
/// the bare `turn.submit`, which the venue puts on the plane as `ui:turn.submit` — but with
/// exactly ONE field: what was said. WHO said it and WHICH conversation it lands in are not the
/// form's to claim (prism stamps the sender; the channel is the application's own fact), so a
/// page has nothing with which to speak as somebody else or into somebody else's channel.
fn frame_composer() -> Value {
    json!({
        "type": "form",
        "attrs": {"class": ["nogrow", "horizontal", "gap", "box", "border", "round"]},
        "bind": {"value": {"kind": "dest", "dest": "turn.submit"}},
        "children": [
            {
                "type": "input",
                "attrs": {"class": ["main"]},
                "bind": {"value": {"kind": "field", "field": "user_message", "type": "text", "default": ""}},
            },
            {
                "type": "button",
                "attrs": {"oneshot": true, "class": ["nogrow"]},
                "bind": {"value": {"kind": "submit", "default": "发送"}},
            },
        ],
    })
}

fn frame_messages() -> Value {
    json!({
        "type": "rack",
        "attrs": {
            "class": ["main", "box", "border", "round", "gap"],
            "view": {"layout": "adaptive", "scroll": "bar"},
        },
        "bind": {"value": {"kind": "source", "source": MESSAGES_SLOT}},
        "item": [
            {
                "type": "case",
                "attrs": {
                    "selector": "user",
                    "class": ["nogrow", "as-end", "accent", "box", "border", "round", "shadow"],
                },
            },
            {
                "type": "case",
                "attrs": {"class": ["nogrow", "as-stretch", "box", "border", "round", "shadow"]},
            },
        ],
    })
}

/// THE SLOT WHERE THE DEPLOYMENT SPEAKS TO THE PERSON: one line at a time, floating above the
/// screen, gone when nobody is looking. The frame carries no lifetime — how long a line stays, how
/// it enters, whether it turns over, and what holds it still are the RENDERER's business (rack's
/// `view`, fluxen ADR 0024). The venue only says the line.
const NOTICE_SLOT: &str = "notice";

/// THE SLOT THE DEPLOYMENT DRAWS ITS SCENE IN: whatever the left area shows is content the
/// deployment pushes, not a widget the shell decides on.
const SCENE_SLOT: &str = "scene";

/// The page the frame shows: a LOCAL slot — the bar writes it, the pager reads it.
const PAGE_SLOT: &str = "page";

/// The pager's rows: one per page, `id` is the page's key.
const PAGES_SLOT: &str = "pages";

/// The bar's items.
const TABS_SLOT: &str = "tabs";

/// The menu under the scene: the deployment appends rows, in its own vocabulary.
const MENU_SLOT: &str = "menu";

/// THE PLATFORM'S ONE RESERVED REGION: whoever proves credentials draws here. prism's auth booth
/// answers `ui:auth.request` with a `set` into this slot (ADR-0017 §2), so the shell declares the
/// region — and knows nothing else about login, which is what keeps the form reusable.
const AUTH_SLOT: &str = "auth";

/// THE PLATFORM'S REGION, AS AN OVERLAY. A `placeholder` is the node that RENDERS a data slot as a
/// live node (a plain `case` has no such binding), so the auth booth's `set` lands here and the
/// renderer fades it in. The shell names the slot and nothing else — the form, the fields and the
/// verb behind them are the platform's.
fn frame_auth() -> Value {
    json!({
        "type": "float",
        "attrs": {"class": ["auth"]},
        "children": [{
            "type": "placeholder",
            "id": "auth-region",
            "attrs": {"class": ["--auth"]},
            "bind": {"value": {"kind": "source", "source": AUTH_SLOT}},
        }],
    })
}

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
    emit(ctx, SCREEN_EVENT, device.as_deref(), None, json!(frame_root(forms)))?;
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
                ("alias", "default", FieldKind::Text, &model),
                ("endpoint", "https://…/v1", FieldKind::Text, &model),
                ("model", "model id", FieldKind::Text, &model),
                ("key_env", "the env var holding the key", FieldKind::Text, &model),
                ("weight", "1", FieldKind::Number, &model),
                ("enabled", "1", FieldKind::Number, &model),
            ],
        ),
        frame_form(
            "settings",
            "1",
            "how this deployment remembers",
            &[
                ("store_url", "http://127.0.0.1:7760", FieldKind::Text, &settings),
                ("memory_scope", "37", FieldKind::Number, &settings),
                ("memory_format", "plaintext", FieldKind::Text, &settings),
                ("default_model", "1", FieldKind::Number, &settings),
            ],
        ),
    ])
}

/// How a field is typed on the screen: a text input sends a string, a number input a float, and the
/// row's declared width decides which of them encodes (see `coerce_numbers`).
#[derive(Clone, Copy, PartialEq)]
enum FieldKind {
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
    fields: &[(&str, &str, FieldKind, &Value)],
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
            (FieldKind::Text, Value::String(text)) => Value::String(text.clone()),
            (FieldKind::Number, Value::Number(_)) => current.clone(),
            (FieldKind::Number, _) => Value::from(hint.parse::<f64>().unwrap_or(0.0)),
            _ => Value::String(hint.to_string()),
        };
        children.push(json!({
            "type": "input",
            "bind": {"value": {
                "kind": "field",
                "field": name,
                "type": if *kind == FieldKind::Number { "number" } else { "text" },
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
            emit(ctx, SAVED_EVENT, device.as_deref(), None, saved)?;
            Ok(json!({ "done": true }))
        }
        Err(error) => {
            let detail = format!("{error:#}");
            let _ = emit(
                ctx,
                SAVE_REFUSED_EVENT,
                device.as_deref(),
                None,
                json!({ "detail": detail }),
            );
            // THE PERSON SEES IT TOO. The fact above is for programs; a page that only got that sees
            // nothing at all (prism injects a non-list payload into a slot nobody binds). So the same
            // refusal rides into the notice slot as a row — the door says the line, the renderer
            // decides how long it stays on screen.
            let mut rows = Vec::new();
            let _ = say_notice(&mut rows, &detail)
                .and_then(|()| emit(ctx, NOTICE_EVENT, device.as_deref(), None, json!(rows)));
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
/// THE MEMBER'S FIRST CHANNEL — the answer to "which conversation" when the caller is a page
/// and the channel is the application's own fact. k10r serves it (`GET /members/{id}/channels`,
/// one row per membership), and taking the first is the honest default for a deployment that
/// has not yet built the channel list a person would switch in.
fn first_channel_of(store_url: &str, member: &str) -> anyhow::Result<u64> {
    let url = format!("{store_url}/members/{member}/channels");
    let resp = ureq::get(&url)
        .call()
        .with_context(|| format!("turn: asking the store which channels {member} is in failed"))?;
    let rows: Value =
        serde_json::from_reader(resp.into_reader()).context("turn: bad channel list")?;
    rows.as_array()
        .and_then(|rows| {
            rows.iter()
                .find_map(|row| row.get("channel_id").and_then(Value::as_u64))
        })
        .with_context(|| {
            format!("turn: {member} is in no channel — a turn needs a conversation to write into")
        })
}

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
    /// The product, once the thread's stream ends: its text, or why it failed.
    /// The thread owns the connection and accumulates nothing the caller needs
    /// mid-flight — a branch is maintenance, and no one reads it until it ends.
    rx: std::sync::mpsc::Receiver<Result<String, String>>,
    keys: Vec<Pointer>,
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
    turn: &Turn<'_>,
    view: &Value,
    prefix: &[Value],
    keys: &[Pointer],
    tails: &mut Vec<Tail>,
    cancel: &induction_guest::CancelToken,
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
        open_branch(turn, &tail, name, prefix, keys, tails, cancel.clone());
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
    turn: &Turn<'_>,
    tail: &Value,
    name: &str,
    prefix: &[Value],
    keys: &[Pointer],
    tails: &mut Vec<Tail>,
    cancel: induction_guest::CancelToken,
) {
    let Some(prompt) = tail.get("prompt").and_then(Value::as_str) else {
        return;
    };
    let mut messages: Vec<Value> = prefix.to_vec();
    messages.push(json!({ "role": "user", "content": prompt }));
    let messages = json!(messages);
    let endpoint = turn.endpoint.clone();
    let model = turn.model.clone();
    let key_env = turn.key_env.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    // A THREAD, not a shared pull loop: the endpoint caches the prefix, so the
    // branch pays only its own delta, and nothing about it touches the answer's
    // thread except the day it finishes. The stream dies with the thread.
    std::thread::spawn(move || {
        let opened = gravity_core::llm::live::Live::open(&gravity_core::llm::live::Request {
            endpoint: &endpoint,
            model: &model,
            key_env: &key_env,
            messages: &messages,
            tools: None,
        });
        let mut live = match opened {
            Ok(live) => live,
            Err(e) => {
                let _ = tx.send(Err(format!("{e:#}")));
                return;
            }
        };
        let mut parts = String::new();
        loop {
            // THE BRANCH IS MAINTENANCE, AND MAINTENANCE STOPS TOO: a cancelled turn
            // drops these receivers, so a branch that keeps pulling writes a summary
            // nobody will ever read — and pays the provider for it. The token is the
            // same call's, so the check costs one flag read per chunk.
            if cancel.is_cancelled() {
                return;
            }
            match live.pull() {
                Ok(env) => {
                    if env.get("done").and_then(Value::as_bool) == Some(true) {
                        break;
                    }
                    if let Some(delta) = env
                        .get("item")
                        .and_then(|item| item.get("delta"))
                        .and_then(Value::as_str)
                    {
                        parts.push_str(delta);
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(format!("{e:#}")));
                    return;
                }
            }
        }
        let _ = tx.send(Ok(parts));
    });
    tails.push(Tail {
        op: name.to_string(),
        rx,
        keys: keys.to_vec(),
    });
}

/// One pull round of the live branch. On `done` its product goes to the door the
/// operation is responsible for. k10r derives the coverage point from the verdict
/// it served, so the caller never states WHERE the node ends — but the record a
/// compression says it should have ended at is the caller's observation to make,
/// because only the caller built the messages the model read.
fn step_tail(tails: &mut Vec<Tail>, turn: &Turn<'_>) {
    if tails.is_empty() {
        return;
    }
    let branch = tails.remove(0);
    match branch.rx.try_recv() {
        Ok(Ok(text)) => {
            if !text.trim().is_empty() {
                land_tail(turn, &branch, &text);
            }
        }
        Ok(Err(error)) => eprintln!("[gravity-turn] `{}` branch failed: {error}", branch.op),
        // Still working: put it back for the next round. This never blocks the
        // answer — that is the whole point of the thread.
        Err(std::sync::mpsc::TryRecvError::Empty) => tails.push(branch),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            eprintln!("[gravity-turn] `{}` branch vanished", branch.op)
        }
    }
}

/// Drain the live branch before the turn closes. The reason is NOT that the
/// carrier reclaims the stream when the entry returns (it does not — a stream is
/// released by `iterate_dispose` or by eviction, ADR-0034 §3): it is that a pull
/// stream does not move on its own. The pull loop IS this call stack, so the
/// moment the turn returns, nobody is left to ask for the next envelope and the
/// branch would sit idle until eviction. Draining is therefore the only way to
/// land the summary from here — the summary must be in place before the next user
/// message (Phase 3) and the answer has already streamed, so the wait is the
/// product's own tail. The cap keeps a runaway branch from holding the turn open;
/// what is left unlanded is k10r's open node, which its next read treats as
/// "compressing".
fn drain_tails(turn: &Turn<'_>, tails: &mut Vec<Tail>) {
    for branch in tails.drain(..) {
        match branch
            .rx
            .recv_timeout(std::time::Duration::from_secs(60))
        {
            Ok(Ok(text)) => {
                if !text.trim().is_empty() {
                    land_tail(turn, &branch, &text);
                }
            }
            Ok(Err(error)) => eprintln!("[gravity-turn] `{}` branch failed: {error}", branch.op),
            // The cap keeps a runaway branch from holding the turn open; what is
            // left unlanded is k10r's open node, which its next read treats as
            // "compressing".
            Err(_) => {
                eprintln!("[gravity-turn] `{}` branch outlived the turn", branch.op)
            }
        }
    }
}

/// What an operation's answer MEANT. A compression answers with one JSON object
/// carrying the summary and the cut it would have chosen; anything else is taken
/// at face value as the summary with no opinion.
///
/// The fallback is deliberate: compression is necessary, and a model that
/// ignored the format still produced a usable summary. Dropping the whole product
/// over a shape slip would lose the only thing the branch exists to make.
fn read_products(text: &str, keys: &[Pointer]) -> (String, Option<Pointer>) {
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
fn land_summary(turn: &Turn<'_>, summary: &str, suggested: Option<Pointer>) {
    let url = format!(
        "{}/channels/{}/members/{}/checkpoint",
        turn.store_url, turn.channel_id, turn.member_id
    );
    let mut body = json!({ "summary": summary });
    if let Some(point) = suggested {
        // The whole pointer: k10r refuses a suggestion that names no record, and
        // `(ts, sub)` alone names two once turns can share a millisecond.
        body["suggested_turn"] = json!(point.turn_id);
        body["suggested_ts"] = json!(point.ts_ms);
        body["suggested_sub"] = json!(point.sub);
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

/// A position in k10r's log: `(turn, ts, sub)` (k10r ADR-0024).
///
/// Local to this binary on purpose: the guest speaks the store's WIRE, not its
/// crate, and these three numbers are all a branch needs to resolve the index a
/// model gave it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pointer {
    turn_id: u64,
    ts_ms: u64,
    sub: u16,
}

/// One tool call's net effect, as the history records it: what was asked and what
/// came back. Only the SUCCESS is here (a correction loop's failures are process,
/// and the model already saw them).
struct ToolEvent {
    tool: String,
    args: String,
    result: String,
}

/// How much of a result the history keeps. A step that returns a whole document
/// would otherwise put it in every future turn's prompt and in the summarizer's
/// input; the row is a record of the action, not a copy of its output.
const TOOL_RESULT_LIMIT: usize = 2_000;

/// The line one call reads as, truncated: used for the record a turn leaves in
/// the history AND for the transcript compaction a cold call performs. One
/// renderer, because both are the same claim — this call happened, this came
/// back — and two renderers would drift into two vocabularies.
fn render_call_line(tool: &str, args: &str, result: &str) -> String {
    let mut result = result.to_string();
    if result.len() > TOOL_RESULT_LIMIT {
        let cut = result
            .char_indices()
            .nth(TOOL_RESULT_LIMIT)
            .map(|(i, _)| i)
            .unwrap_or(result.len());
        result.truncate(cut);
        result.push_str("… (truncated)");
    }
    format!("[tool] {tool} {args} -> {result}")
}

/// The line a tool record carries, as it will read in a later turn's prompt:
/// self-describing on purpose — whoever reads the history is not parsing it.
fn render_tool_event(event: &ToolEvent) -> String {
    render_call_line(&event.tool, &event.args, &event.result)
}

/// Compact every FINISHED round of the working transcript into the same
/// net-effect lines the history would carry (PLAN Phase 2.5's cold path: "the
/// transcript takes the minimal replay set").
///
/// A round is one `assistant` message carrying `tool_calls` plus the `tool`
/// messages that answer those ids — the wire shape a provider requires. It
/// cannot be compacted on one side only: an unanswered `tool_calls` message is a
/// malformed request. So the whole group is replaced by one `system` line per
/// call, which is both legal and smaller.
///
/// Called only when the round ABOUT to be dispatched asks for a `human`-nature
/// tool: such a call may not come back for minutes (or at all), and the model
/// has already read the earlier payloads to decide it — what a cold call needs
/// is the shape of what happened, not the bytes. `fast` calls never trigger it:
/// their results are what the next inference is reasoning over, and flushing
/// them would cost the turn its own context for nothing.
fn compact_finished_rounds(messages: &mut Vec<Value>) {
    let mut out: Vec<Value> = Vec::with_capacity(messages.len());
    let mut index = 0;
    while index < messages.len() {
        let calls = messages[index].get("tool_calls").and_then(Value::as_array);
        let Some(calls) = calls.cloned() else {
            out.push(messages[index].clone());
            index += 1;
            continue;
        };
        let ids: Vec<&str> = calls
            .iter()
            .filter_map(|call| call.get("id").and_then(Value::as_str))
            .collect();
        let mut end = index + 1;
        let mut answers: Vec<Value> = Vec::new();
        while end < messages.len() {
            match messages[end].get("tool_call_id").and_then(Value::as_str) {
                Some(id) if ids.contains(&id) => {
                    answers.push(messages[end].clone());
                    end += 1;
                }
                _ => break,
            }
        }
        // An INCOMPLETE group is left as it is: it is not valid to hand the
        // provider either way, and rewriting it here would hide the bug.
        if answers.len() != ids.len() {
            out.push(messages[index].clone());
            index += 1;
            continue;
        }
        for (call, answer) in calls.iter().zip(answers.iter()) {
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .unwrap_or("tool");
            let args = call
                .pointer("/function/arguments")
                .and_then(Value::as_str)
                .unwrap_or("");
            let content = answer.get("content").and_then(Value::as_str).unwrap_or("");
            let line = format!(
                "{} (earlier step, compacted: the raw result was read when it arrived)",
                render_call_line(name, args, content)
            );
            out.push(json!({ "role": "system", "content": line }));
        }
        index = end;
    }
    *messages = out;
}
/// The turn's rows in k10r (ADR-0024), written AS THE TURN RUNS.
///
/// The store mints the id at the first row (`POST /channels/{c}/messages`, no `turn_id`);
/// every round appends its
/// rows under it; the turn closes with ONE `kind = FINAL` record that names the
/// records to keep, and k10r collapses the rest. A turn that never closes keeps
/// its rows as OPEN — working state the projection skips — which is exactly what
/// a suspended or failed turn should leave behind, and what makes a resumed turn
/// able to read its own working set back.
struct TurnLog<'a> {
    store_url: &'a str,
    channel: u64,
    turn_id: u64,
    /// `(ts_ms, sub)` of every row this turn wrote, in order. The keep set is
    /// chosen from this, so it can never name a row that was not written.
    rows: Vec<(u64, u16)>,
}

impl<'a> TurnLog<'a> {
    /// Open the turn by writing its FIRST row — the user's message — and take the id
    /// the store minted for it (k10r ADR-0024 §2). There is no separate open call: a
    /// turn exists because a user said something, and the store reads its id off the
    /// log inside the same append that puts that sentence there, so two turns starting
    /// in one channel cannot be handed one id. A retry that never lands a row costs
    /// nothing but a gap.
    fn open(
        store_url: &'a str,
        channel: u64,
        content: &str,
        role: Role,
        kind: Kind,
    ) -> anyhow::Result<(Self, (u64, u16))> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .context("turn: clock before epoch")?;
        let url = format!("{store_url}/channels/{channel}/messages");
        let resp = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(
                // NO `turn_id`: the omission is what asks the store to mint one.
                &json!({
                    "ts_ms": ts,
                    "sub": 0,
                    "role": role.code(),
                    "kind": kind.code(),
                    "content": content,
                })
                .to_string(),
            )
            .context("turn: opening the turn failed")?;
        anyhow::ensure!(
            (200..300).contains(&resp.status()),
            "turn: opening the turn returned {}",
            resp.status()
        );
        let body: Value =
            serde_json::from_reader(resp.into_reader()).context("turn: bad append answer")?;
        let turn_id = body
            .get("turn_id")
            .and_then(Value::as_u64)
            .context("turn: the append answer named no turn_id")?;
        Ok((
            Self {
                store_url,
                channel,
                turn_id,
                rows: vec![(ts, 0)],
            },
            (ts, 0),
        ))
    }

    /// Write one row, with gravity's clock as its key (k10r ADR-0008 §3) and
    /// `sub` 0: inside a turn the clock already orders the rows, and `sub` is the
    /// same-millisecond tiebreak, which a round boundary never needs.
    fn row(&mut self, role: Role, kind: Kind, content: &str) -> anyhow::Result<(u64, u16)> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .context("turn: clock before epoch")?;
        let sub = self.rows.last().filter(|(ts_last, _)| *ts_last == ts).map_or(0, |(_, last)| last + 1);
        let url = format!(
            "{}/channels/{}/messages",
            self.store_url, self.channel
        );
        let resp = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(
                &json!({
                    "turn_id": self.turn_id,
                    "ts_ms": ts,
                    "sub": sub,
                    "role": role.code(),
                    "kind": kind.code(),
                    "content": content,
                })
                .to_string(),
            )
            .context("turn: append failed")?;
        anyhow::ensure!(
            (200..300).contains(&resp.status()),
            "turn: append returned {}",
            resp.status()
        );
        let pointer = (ts, sub);
        self.rows.push(pointer);
        Ok(pointer)
    }

    /// Close the turn: the closing record plus the records to keep. The list is
    /// what the caller asked to survive — the question, one row per successful
    /// call, the answer — and k10r purges the rest of the turn in the same write.
    fn close(&self, keep: &[(u64, u16)]) -> anyhow::Result<()> {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .context("turn: clock before epoch")?;
        // The closing record must FOLLOW the turn's last row — k10r refuses one
        // that does not, because a FINAL that sorted before a later row would read
        // as an open turn with a stray closing record in it. Same-millisecond rows
        // are the ordinary case here (the answer was just written), so the
        // tiebreak is not optional.
        let sub = self
            .rows
            .last()
            .filter(|(last_ts, _)| *last_ts == ts)
            .map_or(0, |(_, last_sub)| last_sub + 1);
        let url = format!(
            "{}/channels/{}/turns/{}/close",
            self.store_url, self.channel, self.turn_id
        );
        let resp = ureq::post(&url)
            .set("Content-Type", "application/json")
            .send_string(
                &json!({
                    "ts_ms": ts,
                    "sub": sub,
                    "role": Role::Assistant.code(),
                    "content": "",
                    "keep": keep
                        .iter()
                        .map(|(ts_ms, sub)| json!({ "ts_ms": ts_ms, "sub": sub }))
                        .collect::<Vec<_>>(),
                })
                .to_string(),
            )
            .context("turn: closing the turn failed")?;
        anyhow::ensure!(
            (200..300).contains(&resp.status()),
            "turn: close returned {}",
            resp.status()
        );
        Ok(())
    }
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
/// is not this booth's to report.
fn fetch_tasks(store_url: &str, scope: u64) -> Option<Vec<Value>> {
    let resp = ureq::get(&format!("{store_url}/tasks/{scope}"))
        .call()
        .ok()?;
    let tasks: Value = serde_json::from_reader(resp.into_reader()).ok()?;
    tasks.as_array().cloned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::{read_products, read_review_facts, Pointer};

    fn keys() -> Vec<Pointer> {
        vec![
            pointer(1, 100, 0),
            pointer(2, 200, 0),
            pointer(3, 300, 1),
        ]
    }

    fn pointer(turn_id: u64, ts_ms: u64, sub: u16) -> Pointer {
        Pointer {
            turn_id,
            ts_ms,
            sub,
        }
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
            Some(pointer(2, 200, 0)),
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
        assert_eq!(cut, Some(pointer(1, 100, 0)));
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
