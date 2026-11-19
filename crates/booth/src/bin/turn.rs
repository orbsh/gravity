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
//! `{"channel_id", "member_id", "user_message"}` — the conversation, the speaker, the text,
//! and nothing about the deployment, which is a row (ADR-0007) — plus the output shape:
//! `protocol: "fluxen"` for frames into the slots the booth's own layout declares, or
//! `event` naming the event raw deltas are spoken as. The envelope's OWN `channel_id`
//! is an ADDRESS (a device, text) and is a different fact from the conversation's
//! conversation's own ids inside `data`. The venue addresses DEVICES, never channels (prism
//! ADR-0018: a channel is an application's business, the transport knows devices); the turn
//! answers the device that asked and names no grouping.

use anyhow::Context;
use gravity_booth::tools::Doors;
use gravity_booth::wire;
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
        //
        // NO CODEC IN THIS REGISTRATION: the codec is not per request. This booth declared
        // one when it entered (`register`'s `.encoded(…)`, `wire::DECLARED`), the carrier
        // echoed it back as `INDUCTION_ENCODING`, and every frame it emits is that codec.
        // The venue transcodes for each client — codec is per LEG, never end-to-end
        // (ADR-0013 §2).
        .on("turn", |ctx: &mut Ctx, args: Value| run(ctx, &args))
        // THE OTHER HANDLER: where a deployment is configured. The facts are rows in this type's
        // ns (below), so something outside the process has to be able to write them — and this
        // booth refuses an admin verb, so that door is an event like the turn's, behind the same
        // auth declaration.
        .on("save", |ctx: &mut Ctx, args: Value| save(ctx, &args))
        // A PAGE ANSWERING A FORM. The answer is the run's next completion, so this
        // handler does what `advance` does and nothing more: no model is called, because
        // the mechanical part of a run does not need one and the person already decided.
        .on("answer", |ctx: &mut Ctx, args: Value| answer(ctx, &args))
        // THE RESOURCE DOOR (ADR-0003 §7): the Effector asks by (owner, type, sequence) and gets
        // material for ONE transfer; after it publishes, it reports the reference it produced and
        // this booth writes the version row and advances the pointer. The node never holds a
        // credential of its own and never names a location — it names a resource.
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
                    "layout_version": 2,
                    "hot_width": 16,
                    "payload_header_len": 3,
                    "hot_fields": [
                        {"name": "memory_scope", "ty": "U64", "width": 8, "offset": 0, "tag": 0},
                        {"name": "default_model", "ty": "U64", "width": 8, "offset": 8, "tag": 0},
                    ],
                    "cold_fields": [
                        {"name": "store_url", "ty": "Str", "width": 0, "offset": 0, "tag": 1},
                        {"name": "memory_format", "ty": "Str", "width": 0, "offset": 0, "tag": 2},
                        // THE MOUNT TABLE, where a deployment is configured: which operation
                        // language a mount type serves. It is deployment knowledge, not
                        // something a page should carry on every keystroke (layout v2).
                        {"name": "mounts", "ty": "Str", "width": 0, "offset": 0, "tag": 3},
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
    /// The mount table as stored: a JSON array of `{language, type}`. Absent on a
    /// deployment that never declared one — a turn then falls back to what its caller
    /// carries, and a page's answer has nothing to run steps with.
    mounts: Option<String>,
}

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
        mounts: row.get("mounts").and_then(Value::as_str).map(str::to_string),
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
/// A run that stopped on a `request`: the step's form, as data, with the address
/// the answer goes back through. The PAGE draws it; the venue forwards any emit
/// unchanged (a non-array payload becomes a vals slot named by the event), so this
/// adds no frame vocabulary — and the answer returns the way every answer does, as
/// the next turn's text, where the model calls `advance`.
const RUN_PAGE_EVENT: &str = "ui:run.page";
/// WHAT THE MODEL IS TOLD, and nothing else.
///
/// A run's answer is written for the EDGE: the page needs every waiting form and the
/// addresses of the walk. The model needs to know whether its call landed and what
/// comes next — the rest is the run's own ledger (it is in k10r) and the page's
/// business (it is in the frames). Feeding the whole answer back as the tool message
/// put the engine's bookkeeping into the conversation, where it stays forever and
/// shifts every later turn's context.
///
/// So: a status, the run it is about, and at most what a person is being asked for.
fn conclusion(value: &Value) -> Value {
    let about_a_run = value.get("requests").is_some()
        || value.get("complete").is_some()
        || value.get("ready").is_some();
    if !about_a_run {
        return value.clone();
    }
    let mut said = json!({ "ok": value.get("ok").cloned().unwrap_or(json!(true)) });
    if let Some(run_id) = value.get("run_id").and_then(Value::as_u64) {
        said["run_id"] = json!(run_id);
    }
    // A failed step is the one thing a reader must not lose: it is why the run did
    // not do what was asked.
    if let Some(failed) = value
        .get("ran")
        .and_then(Value::as_array)
        .and_then(|ran| ran.iter().find(|entry| entry.get("failed").is_some()))
    {
        said["status"] = json!("failed");
        said["failed"] = json!({ "step": failed["step"], "reason": failed["reason"] });
        return said;
    }
    let asks = value
        .get("requests")
        .and_then(Value::as_array)
        .filter(|asks| !asks.is_empty());
    if let Some(asks) = asks {
        said["status"] = json!("waiting");
        // Field NAMES, not declarations: the page drew the form, and a second copy
        // here would drift from it.
        said["asks"] = json!(asks
            .iter()
            .map(|ask| json!({
                "step": ask["step"],
                "fields": ask["form"]["fields"]
                    .as_array()
                    .map(|fields| fields.iter().map(|f| f["name"].clone()).collect::<Vec<_>>())
                    .unwrap_or_default(),
            }))
            .collect::<Vec<_>>());
        return said;
    }
    if value.get("complete").and_then(Value::as_bool) == Some(true) {
        said["status"] = json!("complete");
        said["result"] = value.get("result").cloned().unwrap_or(Value::Null);
        return said;
    }
    if value.get("stalled").and_then(Value::as_bool) == Some(true) {
        said["status"] = json!("stalled");
        return said;
    }
    let next: Vec<Value> = value
        .get("ready")
        .and_then(Value::as_array)
        .map(|ready| {
            ready
                .iter()
                .map(|a| json!({ "step": a["step"], "pass": a["pass"], "attempt": a["attempt"] }))
                .collect()
        })
        .unwrap_or_default();
    if next.is_empty() {
        said["status"] = json!("blocked");
        said["blocked"] = value.get("blocked").cloned().unwrap_or(json!([]));
    } else {
        said["status"] = json!("ready");
        said["next"] = json!(next);
    }
    said
}

/// What the plan's `send` to the page becomes, now that a deployment has bound it.
///
/// The plan said "the ask is over" (`{"subject": "ui", "payload": {"clear": "ask"}}`);
/// this turns that into the frames the renderer understands — a `remove` of the row
/// whose id is that step's answer address. Same slot the forms were appended to, so
/// the order the plan implies is the order the page sees, without the plan knowing
/// anything about frames.
/// The same, for a tool answer that names its run: `start` answers one, `advance`
/// was GIVEN one.
fn page_updates(value: &Value, args: &Value) -> Vec<Value> {
    let run_id = value
        .get("run_id")
        .and_then(Value::as_u64)
        .or_else(|| args.get("run_id").and_then(Value::as_u64));
    let Some(run_id) = run_id else {
        return Vec::new();
    };
    let ran: &[Value] = value
        .get("ran")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    ui_frames(run_id, ran)
}

fn ui_frames(run_id: u64, ran: &[Value]) -> Vec<Value> {
    let mut frames = Vec::new();
    for entry in ran {
        if entry.get("subject").and_then(Value::as_str) != Some("ui") {
            continue;
        }
        let Some(addresses) = entry.get("clear").and_then(Value::as_array) else {
            continue;
        };
        for address in addresses {
            let (Some(step), Some(pass), Some(attempt)) = (
                address.get("step").and_then(Value::as_str),
                address.get("pass").and_then(Value::as_u64),
                address.get("attempt").and_then(Value::as_u64),
            ) else {
                continue;
            };
            frames.push(json!({
                "action": "remove",
                "slot": MESSAGES_SLOT,
                "id": format!("{run_id}/{step}/{pass}/{attempt}"),
            }));
        }
    }
    frames
}

/// A run that moved and is not waiting on anybody: finished, or blocked on something
/// this deployment cannot do. The page stops asking when it hears this.
const RUN_DONE_EVENT: &str = "ui:run.done";
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
/// cannot say so: a bundle's uplink carries its packed fields and nothing else (fluxen builds
/// `{tag: value}`), so demanding a protocol of it would mean a hidden field whose
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

/// What a tool answer says the page must ask, if anything: a run's unanswered
/// `request` steps, with the run they belong to and the skill it was started from.
///
/// A tool answer with no `requests` (or an empty one) says nothing — a run that is
/// merely finished, or waiting on a delivery, is not a question for a person.
fn awaiting_of(value: &Value, args: &Value) -> Option<Value> {
    let requests = value
        .get("requests")
        .and_then(Value::as_array)
        .filter(|requests| !requests.is_empty())?;
    Some(json!({
        "run_id": value.get("run_id"),
        "skill_id": args.get("skill_id"),
        "requests": requests,
    }))
}

/// The uplink a form's answer takes. A control's target is its `sink` and never `event`
/// (acCrete ADR-0023: downlink writes a slot, uplink calls a handler, and the two are
/// not symmetrical); the venue prefixes it, so this is `ui:run.answer` on the wire.
const ANSWER_DEST: &str = "run.answer";

/// A waiting run, as PAGE.
///
/// One `bundle` per waiting step, and the bundle carries its ADDRESS in its own `id`
/// (`<run>/<step>/<pass>/<attempt>`). Submitting sends `Outflow { dest, id, data }` — the
/// address rides the answer itself, which is what makes a batch of answers and a second
/// device ordinary traffic instead of a guessing game about which chat message meant what.
///
/// Two things moved in fluxen ADR 0032 and both are load-bearing here: a field is named by
/// its own `tag` (the bundle packs `{tag: value}`; the field's `sink` is no longer its name),
/// and the exit belongs to the bundle (`sink`), not to each field.
fn request_forms(awaiting: &Value) -> Vec<Value> {
    let run_id = awaiting.get("run_id").and_then(Value::as_u64).unwrap_or(0);
    let empty = Vec::new();
    let requests = awaiting
        .get("requests")
        .and_then(Value::as_array)
        .unwrap_or(&empty);
    requests
        .iter()
        .map(|request| {
            let step = request.get("step").and_then(Value::as_str).unwrap_or("");
            let pass = request.get("pass").and_then(Value::as_u64).unwrap_or(0);
            let attempt = request.get("attempt").and_then(Value::as_u64).unwrap_or(0);
            let address = format!("{run_id}/{step}/{pass}/{attempt}");
            let form = request.get("form").cloned().unwrap_or(Value::Null);
            let mut children = Vec::new();
            let empty_fields = Vec::new();
            for field in form
                .get("fields")
                .and_then(Value::as_array)
                .unwrap_or(&empty_fields)
            {
                let name = field.get("name").and_then(Value::as_str).unwrap_or("");
                // The declared kind decides the WIDGET (`source.type`), which is accrete's
                // vocabulary (`JsType`), smaller than the plan's. `choice` has no form
                // widget: a `select` draws its rows from a LIST slot (`options`) and the
                // plan's options live in its declaration, not in a slot — so a choice falls
                // back to text rather than drawing a select nobody can pick from.
                let declared = field.get("type").and_then(Value::as_str).unwrap_or("string");
                let (widget, blank) = match declared {
                    "integer" | "number" => ("number", json!(0)),
                    "bool" => ("bool", json!(false)),
                    _ => ("text", json!("")),
                };
                children.push(json!({
                    "type": "input",
                    // THE FIELD'S NAME IS ITS TAG, not an address: this is the key the
                    // bundle packs, and it is the only thing that makes the answer readable
                    // (`{branch: "left"}`, not `{<slot>: "left"}`).
                    "tag": name,
                    "source": { "default": blank, "type": widget },
                }));
            }
            children.push(json!({
                "type": "button",
                "attrs": { "oneshot": true },
                // A button's label is its `source`; its `sink` is what makes it the bundle's
                // confirm key (a button with no sink is not drawn at all).
                "source": { "default": "Send" },
                "sink": {},
            }));
            json!({
                // THE STREAM, not a region of our own: a form a person must answer is
                // part of the conversation, and the tiles already draw it (selector
                // `assistant`: the deployment is the one asking). No new slot, no new
                // template — the message list is where an exchange belongs.
                "action": "append",
                "slot": MESSAGES_SLOT,
                "data": {
                    // A BUNDLE, not a `form`: it packs these fields into one object and
                    // owns the exit. `id` is this submission (the address), never a field.
                    "type": "bundle",
                    "id": address,
                    // WHEN it leaves. Enter is the chat gesture and the button is the
                    // visible one; a bundle that declared only `enter` would not DRAW the
                    // button.
                    "attrs": { "selector": "assistant", "submit": ["enter", "button"] },
                    "sink": { "slot": ANSWER_DEST },
                    "children": children,
                },
            })
        })
        .collect()
}

/// WHO A FRAME IS FOR. The venue names ONE address per emit: a `user` reaches every device that
/// account is live on (the fan-out is the venue's — a booth cannot enumerate devices, ADR-0018
/// §3), a `device` reaches one connection. A person has one conversation, not one per machine,
/// so the conversation's frames name the account; the connection is the fallback for a page that
/// has not signed in and therefore has no account to address.
struct To<'a> {
    user: Option<&'a str>,
    device: Option<&'a str>,
}

fn to<'a>(user: Option<&'a str>, device: Option<&'a str>) -> To<'a> {
    To { user, device }
}

/// One venue emit. `name` is the recipient event — a compiled view body is not told which
/// event fired (the concrete name is consumed by the dispatcher), so the emitter states
/// it; `to` is the address (see `To`); the payload hangs off `data`, which is the shape the
/// venue reads as "a delivery, not a request".
/// The page's name for this booth's region — a DEPLOYMENT fact, not a code fact: the frame that
/// declares the region and this process must agree, so the name comes from the environment the
/// deployment sets for both. The default is the reference frame's (`prism/assets/shell.json`).
fn island() -> String {
    std::env::var("GRAVITY_ISLAND").unwrap_or_else(|_| "gravity-model".to_string())
}

fn emit(
    ctx: &mut Ctx,
    event: &str,
    to: To<'_>,
    turn: Option<&str>,
    data: Value,
) -> anyhow::Result<()> {
    // WHICH REGION THIS SURFACE BELONGS TO (prism ADR-0019). The venue namespaces the slots of an
    // island's frames, and this is the one fact it cannot derive: the event's name says what
    // happened (`ui:layout`, `ui:turn.done`), never which part of the page it is about. A booth
    // that says it cannot write into another island by accident; a booth that does not is left
    // alone (its slots keep their bare names, which is correct for a venue-owned surface).
    let mut envelope = json!({ "name": event, "island": island() });
    if let Some(turn) = turn {
        // WHICH turn this chunk belongs to. A device may have several in flight — the type
        // declares `unbounded` (aura ADR-0046) — and every chunk of every turn arrives at the
        // same recipient, so without this a second answer interleaves into the first and the
        // client cannot untangle them. The induction wire learned the same lesson one layer
        // down (`host.call`, induction ADR-0002).
        envelope["turn"] = json!(turn);
    }
    if let Some(user) = to.user {
        // ONE ADDRESS, and the account is the stronger one: the venue already knows which
        // connections are live for it, and a signed-in person's second machine is part of the
        // same conversation rather than a stranger to it.
        envelope["user"] = json!(user);
    } else if let Some(device) = to.device {
        // NO `sender` on a booth's own speech: the venue excludes the sender from its
        // fan-out, so marking the asking device as the sender would send the answer to
        // everyone EXCEPT the one connection that wants it.
        envelope["device"] = json!(device);
    }
    envelope["data"] = data;
    emit_frame(ctx, event, envelope)
}

/// THE ONE PLACE A FINISHED FRAME IS HANDED TO THE GUEST — and therefore the one place the
/// CONTENT CODEC applies (ADR-0013).
///
/// gravity has two content modes (text, accrete frames) and two encodings (JSON, CBOR);
/// encoding is a transport axis, so it lives here rather than in either mode, and every
/// caller of [`emit`] — both modes, every event — inherits the same answer. JSON is
/// today's frame, unchanged; CBOR is the same content encoded once and handed over as
/// bytes, which the receiving side recognizes by the payload's shape, never by
/// configuration.
///
/// WHICH answer is the booth's DECLARATION ([`wire::declared`]): what this process told its
/// venue it can produce. It is fixed for the process's life — a request does not renegotiate
/// it, and nothing about a client's handshake reaches this line. The venue transcodes for
/// whatever each client negotiated.
fn emit_frame(ctx: &mut Ctx, event: &str, envelope: Value) -> anyhow::Result<()> {
    ctx.emit(event, carried(envelope, wire::declared())?)
}

/// The frame as it goes on the venue wire: the JSON path hands the envelope over untouched;
/// the CBOR path replaces `data` with `{"$cbor": "<base64>"}` and leaves everything else
/// exactly as it was.
///
/// ONLY `data` IS ENCODED, and that is a contract with the venue, not an optimisation: the
/// head (`name`, `island`, `turn`, `user`/`device`) is what prism ROUTES on, so it has to be
/// readable without decoding content — a venue that had to unwrap a CBOR document to learn
/// where a frame is addressed would be a venue with a codec dependency in its router.
/// `data` is the only field whose value is content, so it is the only field the axis touches.
///
/// The wrapper is base64 inside JSON rather than a byte string because the payload is a
/// `serde_json::Value` at every hop between here and prism — the carrier decodes every guest
/// frame into one (`effector`'s `carrier/exec.rs`, the `Encoding::Cbor` read arm), and that
/// type has no bytes variant. See ADR-0013 §3 for what real binary would take.
fn carried(envelope: Value, codec: wire::Codec) -> anyhow::Result<Value> {
    match codec {
        wire::Codec::Json => Ok(envelope),
        wire::Codec::Cbor => {
            let mut frame = envelope;
            let content = frame
                .get_mut("data")
                .map(Value::take)
                .unwrap_or(Value::Null);
            if let Some(head) = frame.as_object_mut() {
                head.insert(
                    "data".to_string(),
                    induction_guest::cbor_payload(&codec.encode(&content)?),
                );
            }
            Ok(frame)
        }
    }
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
            "type": "frame",
            "attrs": { "selector": selector },
            "children": [{
                "type": "text",
                "source": { "default": text },
            }],
        },
    })
}

/// One chunk into the bubble `id` — ADDRESSED BY ROW, not by a shared slot: two turns in
/// flight patch their own bubbles and cannot interleave.
///
/// The pointer is the NODE's wire shape (fluxen ADR 0005): a channel's value IS the channel
/// since ADR 0031, so the text child's value sits at `/source/default`, not under a `bind`.
fn frame_chunk(id: &str, chunk: &str) -> Value {
    json!({
        "action": "patch",
        "slot": MESSAGES_SLOT,
        "id": id,
        "path": "/children/0/source/default",
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
/// The answer's values. A bundle packs `{tag: value}` (fluxen ADR 0032 — the per-field
/// `{data, payload}` wrapper is gone), so a page's answer is already the values and this is the
/// identity. A sender that still wraps a field in `{data: …}` is unwrapped: one rule, two
/// shapes, and no second place that has to know which side of the wire it is on.
fn answer_values(args: &Value) -> Value {
    let Some(map) = args.as_object() else {
        return args.clone();
    };
    Value::Object(
        map.iter()
            .map(|(field, sent)| {
                let value = sent.get("data").cloned().unwrap_or_else(|| sent.clone());
                (field.clone(), value)
            })
            .collect(),
    )
}

fn payload(envelope: &Value) -> &Value {
    envelope
        .get("data")
        .filter(|v| v.is_object())
        .unwrap_or(envelope)
}

/// ONE field, TWO shapes: a bundle packs its fields as bare values (`{user_message: "hi"}`,
/// fluxen ADR 0032), while a sender that still wraps one in `{data, payload}` is unwrapped here —
/// the same rule `write_row` follows for the deployment forms. Unwrapping here is what lets the
/// composer's own words reach a turn that a direct caller reaches by typing them.
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
                emit(ctx, DONE_EVENT, to(user.as_deref(), device.as_deref()), id.as_deref(), data)?;
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
                let _ = emit(ctx, ERROR_EVENT, to(user.as_deref(), device.as_deref()), id.as_deref(), data);
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
            channel_of(&store_url, member)?
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
        memory_format: settings.memory_format.clone(),
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
        to(user.as_deref(), device),
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
            emit(ctx, UI_EVENT, to(user.as_deref(), device), Some(&turn.id), json!(frames))?;
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
        let mounts = mounts_of_settings(&settings, args);
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
            let world = gravity_booth::tools::World {
                store_url: &turn.store_url,
                memory_scope: Some(turn.memory_scope),
                mounts: &mounts,
                session: &session,
                ends_on: &ends_on,
            };
            // The waiting requests of a run that stopped on a person, captured
            // inside the call (where the answer is in hand) and emitted after the
            // ctx borrow ends.
            let mut awaiting: Option<Value> = None;
            // What this call decided the page should do (a `send` to `ui`), captured
            // where the answer is in hand and emitted once the ctx borrow ends.
            let mut page: Vec<Value> = Vec::new();
            // The ctx borrow lives only for this call: the provider re-open
            // below needs it back.
            let content = {
                let mut doors = VenueDoors { ctx: &mut *ctx };
                let before = walked.len();
                let outcome = gravity_booth::tools::call(
                    &mut doors,
                    &call.function.name,
                    &call.function.arguments,
                    &world,
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
                        // A run that is waiting for a PERSON is the one tool
                        // answer a page has to act on: `start`/`advance` name the
                        // steps whose form is unanswered, and the address
                        // (run, step, pass, attempt) an answer comes back with.
                        awaiting = awaiting_of(&value, &call.function.arguments);
                        page = page_updates(&value, &call.function.arguments);
                        // One record per call that SUCCEEDED. The same call twice
                        // (a model that re-asks rather than moving on) keeps only
                        // the last answer: what the store owes the next turn is
                        // the effect, not the retries.
                        let told = conclusion(&value);
                        let event = ToolEvent {
                            tool: call.function.name.clone(),
                            args: call.function.arguments.to_string(),
                            result: told.to_string(),
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
                        told.to_string()
                    }
                    Err(error) => json!({ "error": error }).to_string(),
                }
            };
            // A `send` to the page happens where the plan put it: the row goes as soon
            // as the step that said so completed, not at the end of the turn.
            if !page.is_empty() {
                emit(
                    ctx,
                    RUN_PAGE_EVENT,
                    to(user.as_deref(), device.as_deref()),
                    Some(&turn.id),
                    json!(page),
                )?;
            }
            if let Some(awaiting) = awaiting.take() {
                let forms = request_forms(&awaiting);
                if !forms.is_empty() {
                    emit(
                        ctx,
                        RUN_PAGE_EVENT,
                        to(user.as_deref(), device.as_deref()),
                        Some(&turn.id),
                        json!(forms),
                    )?;
                }
            }
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
    let streamed = stream_answer(ctx, &out, device, user.as_deref(), &pump, delta, &turn, &mut tails);
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
            to(user.as_deref(), device),
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
    user: Option<&str>,
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
                    to(user.as_deref(), device),
                    Some(&turn.id),
                    json!(frames_chunk(&turn.id, first, &delta)),
                )?;
            }
            Out::Data(event) => {
                emit(ctx, event, to(user.as_deref(), device), Some(&turn.id), json!({ "delta": delta }))?
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


/// The mount table, from the deployment's row first and the caller's list second.
///
/// The row wins because it is where the deployment is configured; the argument stays for
/// a direct caller (the CLI, a test seam) that has no row to read, and for a deployment
/// that has not declared one yet. Both shapes are the same: `{language, type}`.
fn mounts_of_settings(settings: &Settings, args: &Value) -> Vec<(String, String)> {
    if let Some(text) = settings.mounts.as_deref() {
        if let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(text) {
            let declared: Vec<(String, String)> = entries
                .iter()
                .filter_map(|entry| {
                    Some((
                        entry.get("language")?.as_str()?.to_string(),
                        entry.get("type")?.as_str()?.to_string(),
                    ))
                })
                .collect();
            if !declared.is_empty() {
                return declared;
            }
        }
    }
    mounts_of(args)
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
/// §8 — the layout is data on the wire). fluxen draws a button only when it is a bundle's confirm key
/// (a button with no `sink` is nothing), so the control is a bundle with ONE button and
/// `submit: [button]`: clicking it sends `{}` to `gravity-model`, which the venue turns into
/// `ui:gravity-model` with this device as the sender.
/// Whether the venue drew a frame around this page (prism ADR-0019). The field rides every delivery
/// the venue makes — top level, so one question has one place — and an OLD venue that does not say
/// it gets the old behaviour: this booth draws the whole page.
fn hosted(envelope: &Value) -> bool {
    envelope
        .get("shell")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn shell(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    let user = sender_user(envelope);
    // THE ANSWER IS THE ACTION LIST ITSELF, never a `{frames: [...]}` wrapper: prism forwards an
    // array as the renderer's message and would INJECT anything else into a slot named by the
    // event — a page that gets a map draws nothing and reports nothing (ADR-0017 §8).
    // The management page ships WITH its forms: the layout is rebuilt on every save anyway
    // (`screen`), so there is no separate "load the forms" step to trigger.
    let forms = frames_model(ctx).unwrap_or_default();
    // TWO ARRANGEMENTS (prism ADR-0019). HOSTED: the venue already drew the frame from a file, and
    // this booth owns ONE region inside it — a `create` here would replace the root, erasing the
    // navigation and every other island along with it. STANDALONE: nothing was drawn, so this booth
    // is the page. The venue says which; the booth never guesses, because guessing wrong is silent.
    let hosted = hosted(envelope);
    emit(
        ctx,
        LAYOUT_EVENT,
        to(user.as_deref(), device.as_deref()),
        None,
        json!(frame_root(forms, hosted)),
    )?;
    Ok(json!({ "done": true }))
}

/// The screen's tree: an OUTER FRAME (a navigation bar over a set of PAGES), never a page with
/// everything stacked in one column. Two pages live in the frame — home (the field and the
/// conversation) and management (the model's forms) — and the bar switches between them.
///
/// Management is its own page rather than a panel over the conversation, and the pager holds
/// every page as a row of `pages` with a window of one: the row that is not on stage is still in
/// the slot, so a turn streaming into `messages` while a person fills a form keeps landing in a
/// slot the pager has, and coming back re-renders from it.
fn frame_root(forms: Vec<Value>, hosted: bool) -> Vec<Value> {
    // THE CONVERSATION PAGE: the split the example uses (examples/yaml/00.main.yaml) — a
    // horizontal case with `stage-split`, a `stage-graph` column on the left and a `stage-stream`
    // column on the right. The widths are the bundle's own CSS — graph `1 1 40%`, stream
    // `1 0 620px`, and under 1000px the graph pins to 240px — which is what makes the GRAPH absorb
    // the compression while the stream keeps its width.
    let conversation = json!({
        "type": "frame",
        "attrs": {"class": ["maximize", "stage-split", "shrink"], "horizontal": true},
        "children": [frame_scene(), frame_stream()],
    });

    // HOSTED: the region's content and nothing else. The shell — navbar, page switch, tabs — is
    // the host's, and everything below is what happens when a booth brings its own anyway:
    //
    //   - a second navbar renders on top of the host's (measured: "this deployment" twice in the
    //     DOM);
    //   - our `home`/`manage` rows land in the HOST's `pages` slot, next to the rows it writes for
    //     its regions;
    //   - and our own pager bound that same slot, while the host's rows hold our whole tree —
    //     so the tree contained itself and rendering recursed until the wasm stack died
    //     (`RangeError: Maximum call stack size exceeded`, reported far away in the navbar,
    //     because that is where the stack ran out, not where the loop was).
    //
    // The settings forms have no home here: a host declares the regions a booth draws into, and
    // management is a standalone-only surface — this booth's own page, reached by its own pager.
    if hosted {
        let root = json!({
            "type": "frame",
            "attrs": {"class": ["maximize", "col"]},
            "children": [conversation, frame_notice()],
        });
        return vec![json!({ "action": "set", "slot": "", "data": root })];
    }

    // STANDALONE: it IS a page, so it brings its own shell, its own pager and its own bar.
    let root = json!({
        "type": "frame",
        "attrs": {"class": ["maximize", "col"]},
        "children": [frame_navbar(), frame_pager(), frame_auth(), frame_notice()],
    });
    vec![
        json!({ "action": "create", "slot": "", "data": root }),
        json!({ "action": "append", "slot": PAGES_SLOT, "data": {
            "id": "home",
            "type": "frame",
            "attrs": {"class": ["maximize", "stage-split", "shrink"], "horizontal": true},
            "children": [frame_scene(), frame_stream()],
        }}),
        json!({ "action": "append", "slot": PAGES_SLOT, "data": {
            "id": "manage",
            "type": "frame",
            "attrs": {"class": ["main", "col", "gap", "scrolly"]},
            "children": forms,
        }}),
        // The bar's items: a short label per page, same ids.
        json!({ "action": "append", "slot": TABS_SLOT, "data": {
            "id": "home",
            "type": "text",
            "source": { "default": "对话" },
        }}),
        json!({ "action": "append", "slot": TABS_SLOT, "data": {
            "id": "manage",
            "type": "text",
            "source": { "default": "管理" },
        }}),
    ]
}

/// The navigation bar: the deployment's name and the page switch. Clicking an item is LOCAL —
/// the pager reads the same local slot — so switching pages costs no round trip.
fn frame_navbar() -> Value {
    json!({
        "type": "frame",
        "attrs": {"class": ["--head", "nogrow", "horizontal", "center", "box", "gap"]},
        "children": [
            {
                "type": "text",
                "attrs": {"class": ["nogrow"]},
                "source": { "default": "this deployment" },
            },
            {
                "type": "select",
                "attrs": {"horizontal": true},
                // ONE VALUE, TWO DIRECTIONS (ADR 0031 §2): `source` is the page this bar is
                // showing, `sink` is the same page written back. Both are LOCAL — the pager
                // reads the slot this writes, so a click costs no round trip — and the local
                // write is wrapped as `{dest, id, data}`, which is why the read names a path.
                "source": { "slot": PAGE_SLOT, "scope": "local", "path": "/data", "default": "home" },
                "sink": { "slot": PAGE_SLOT, "scope": "local" },
                "options": { "slot": TABS_SLOT },
            },
            // THE PLATFORM'S ONE CONTROL: it says only "open the region" — prism's auth booth draws
            // whatever goes in it, so the application never holds a login form (ADR-0017 §2).
            {
                "type": "bundle",
                "attrs": {"class": ["nogrow"], "submit": ["button"]},
                "sink": { "slot": "auth.request" },
                "children": [{
                    "type": "button",
                    "attrs": {"oneshot": true},
                    "source": { "default": "登录" },
                    "sink": {},
                }],
            },
        ],
    })
}

/// The pager: every page is a row of `pages`, and a window of ONE shows the page the bar picked
/// (`step` names the slot the selection lives in — that is what the retired `pages` node was).
/// The rows stay in the slot whether or not they are staged, so a conversation streaming into
/// one page keeps landing in it while a person looks at another.
fn frame_pager() -> Value {
    json!({
        "type": "tiles",
        "attrs": {
            "class": ["main", "shrink"],
            "window": { "size": 1 },
            "step": { "slot": PAGE_SLOT },
        },
        "source": { "slot": PAGES_SLOT },
    })
}

/// THE LEFT AREA: the scene is a forcegraph, and what it draws is data the deployment pushes
/// into `scene` as `{nodes, links, focus?}` (fluxen ADR 0018 — a native node, not a canvas
/// module: the layout lives in wasm, no URL, no options bag). Below it a menu, in a tiles, so
/// the same column that shows the field of things also lists the ways into them — the shape a
/// channel list would take in a conventional application.
fn frame_scene() -> Value {
    json!({
        "type": "frame",
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
                "source": { "slot": SCENE_SLOT },
            },
            frame_menu(),
        ],
    })
}

/// The menu under the scene: a tiles over `menu`, rows appended by the deployment. Its row
/// template is a line of text for now — what a row DOES when clicked is a decision the
/// deployment owns (an event name, a channel to open), and the shell will not invent one.
fn frame_menu() -> Value {
    json!({
        "type": "tiles",
        "attrs": { "class": ["nogrow", "box", "border", "round", "gap"] },
        "source": { "slot": MENU_SLOT },
        // The LAYOUT of a tile (the field `item` used to be): a template, matched on the
        // row's own selector, with the row replacing its children.
        "children": [{
            "type": "frame",
            "attrs": {"class": ["nogrow", "horizontal", "center", "gap"]},
            "children": [{
                "type": "text",
                "source": { "default": "" },
            }],
        }],
    })
}

/// THE CONVERSATION: `tiles` over `messages`, with the two bubbles declared ONCE as item
/// templates. A row appended to `messages` carries a `selector` and the tiles index these by
/// it, so the application decides neither alignment nor accent — it sends rows
/// and says what each one IS. `user` is the accented, end-aligned bubble; the second template
/// carries no selector and is the fallback, which is what the reply and anything else lands on.
fn frame_stream() -> Value {
    json!({
        "type": "frame",
        "attrs": {"class": ["stage-stream", "main", "shrink", "col"]},
        "children": [frame_messages(), frame_composer()],
    })
}

/// THE COMPOSER: the one place a person's words enter. A `bundle` — it packs one field and
/// owns the exit — whose uplink is the bare `turn.submit`, which the venue puts on the plane as
/// `ui:turn.submit` and whose ONE field is what was said. WHO said it and WHICH conversation it
/// lands in are not the form's to claim (prism stamps the sender; the channel is the
/// application's own fact), so a page has nothing with which to speak as somebody else or into
/// somebody else's channel.
///
/// `submit: [enter, button]` is what makes it a chat box: Enter sends, and the button is drawn
/// BECAUSE the bundle declares `button` (a button the bundle did not declare is not drawn).
fn frame_composer() -> Value {
    json!({
        "type": "bundle",
        "attrs": {
            "class": ["nogrow", "horizontal", "gap", "box", "border", "round"],
            "submit": ["enter", "button"],
        },
        "sink": { "slot": "turn.submit" },
        "children": [
            {
                "type": "input",
                "attrs": {"class": ["main"]},
                // THE FIELD'S NAME: the bundle packs `{user_message: …}`. The field has no
                // `sink` of its own — the bundle is the exit.
                "tag": "user_message",
                "source": { "type": "text", "default": "" },
            },
            {
                "type": "button",
                "attrs": {"oneshot": true, "class": ["nogrow"]},
                "source": { "default": "发送" },
                "sink": {},
            },
        ],
    })
}

fn frame_messages() -> Value {
    json!({
        "type": "tiles",
        "attrs": { "class": ["main", "box", "border", "round", "gap"] },
        "source": { "slot": MESSAGES_SLOT },
        "children": [
            {
                "type": "frame",
                "attrs": {
                    "selector": "user",
                    "class": ["nogrow", "as-end", "accent", "box", "border", "round", "shadow"],
                },
            },
            {
                "type": "frame",
                "attrs": {"class": ["nogrow", "as-stretch", "box", "border", "round", "shadow"]},
            },
        ],
    })
}

/// THE SLOT WHERE THE DEPLOYMENT SPEAKS TO THE PERSON: one line at a time, floating above the
/// screen, gone when nobody is looking. The frame names only the slot (`notice`); the tiles that
/// draws it sets `window` and `step` (see `frame_notice`), and how a line enters and what holds it
/// still are the RENDERER's business (fluxen ADR 0024). The venue only says the line.
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

/// THE PLATFORM'S REGION, AS AN OVERLAY. A `panel` at the viewport, shown while the auth slot
/// holds anything (`state: bound` — non-empty means on screen), and the body is a `frame` bound
/// to that slot: whatever the auth booth `set`s there is rendered. (This was a `float`, which
/// ADR 0031 §3 deleted: it was this panel with no trigger and no mask.) The shell names the slot
/// and nothing else — the form, the fields and the verb behind them are the platform's.
fn frame_auth() -> Value {
    json!({
        "type": "panel",
        "attrs": {"class": ["auth"], "place": "viewport", "state": "bound"},
        "source": { "slot": AUTH_SLOT },
        "children": [{
            "type": "frame",
            "id": "auth-region",
            "attrs": {"class": ["--auth"]},
            "source": { "slot": AUTH_SLOT },
        }],
    })
}

/// THE NOTICE AREA: a list slot drawn as tiles, one line on stage at a time, turning over on a
/// clock (`window: {size: 1}` + `step: {timer}` — the two attributes that replaced the rack's
/// `view.play`/`every`). The frame carries no lifetime of its own, and it needs no "hide while
/// empty" rule either: an empty list draws nothing, and an empty box says nothing (ADR 0031 §3).
fn frame_notice() -> Value {
    json!({
        "type": "frame",
        "attrs": {"class": ["toast"]},
        "children": [{
            "type": "tiles",
            "attrs": {
                "window": { "size": 1 },
                "step": { "timer": 6000 },
            },
            "source": { "slot": NOTICE_SLOT },
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
            "type": "frame",
            "attrs": {"class": ["box", "border", "round", "shadow", "gap"]},
            "children": [{
                "type": "text",
                "source": { "default": message },
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
    let user = sender_user(envelope);
    // THE ANSWER IS THE LAYOUT ITSELF, rebuilt. A screen that appended its rows into a list would
    // stack a second set of forms every time it was asked (the list is keyed by row id, and these
    // rows have none) — so the screen REPLACES the root, exactly as the shell did, and asking twice
    // leaves one screen. The caller names no slot: a click cannot carry one, and this booth owns
    // what a screen looks like anyway.
    let forms = frames_model(ctx)?;
    // The screen is the same tree as the shell, so it lands the same way: a region inside the
    // venue's frame, or the root of the page when there is no frame.
    emit(
        ctx,
        SCREEN_EVENT,
        to(user.as_deref(), device.as_deref()),
        None,
        json!(frame_root(forms, hosted(envelope))),
    )?;
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


/// One form. `collection` and `id` are the ROW'S ADDRESS, and they ride the bundle's `id` as
/// `<collection>:<row>` — the submission's instance, which the venue hands back beside the
/// payload (`Outflow {dest, id, data}`, the same place a run answer's address travels), so the
/// pair cannot be retyped and no field has to carry it. fluxen ADR 0032 ended the other way of
/// saying this: only an `input`/`select` can be a packed field (named by its `tag`), and a
/// `text` node cannot — so the old "text nodes bound as form fields" is gone, and the two
/// labels below are labels, not fields.
fn frame_form(
    collection: &str,
    id: &str,
    heading: &str,
    fields: &[(&str, &str, FieldKind, &Value)],
) -> Value {
    let mut children = vec![
        json!({"type": "text", "source": { "default": heading }}),
        json!({"type": "text", "source": { "default": collection }}),
        json!({"type": "text", "source": { "default": id }}),
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
            // THE FIELD'S NAME IS ITS TAG: the bundle packs `{name: value}`, which is exactly
            // the row document `write_row` validates.
            "tag": name,
            "source": {
                "type": if *kind == FieldKind::Number { "number" } else { "text" },
                "default": default,
            },
        }));
    }
    children.push(json!({
        "type": "button",
        "attrs": {"oneshot": true},
        "source": { "default": "save" },
        "sink": {},
    }));
    json!({
        "type": "bundle",
        // THE ADDRESS OF THIS SUBMISSION (`<collection>:<row>`), read back by `write_row` from
        // the uplink's `id`. A bundle's `id` never names a field.
        "id": format!("{collection}:{id}"),
        "attrs": {
            "class": ["box", "border", "round", "shadow"],
            // A settings form sends on the BUTTON, not on Enter: nothing here is a chat box,
            // and a stray Enter in a row of numbers should not write the deployment.
            "submit": ["button"],
        },
        // The form's own sink is the uplink: the BARE event name, because a page's uplink
        // carries no marker (prism adds `ui:` on the way in — MODELING's rule).
        "sink": { "slot": "gravity-model.save" },
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
/// A PAGE'S ANSWER TO A WAITING RUN — and nothing else.
///
/// No turn, no model: the answer is in hand, the remaining work is mechanical (append
/// the completion, recompute what is ready, run what the engine can run), and a run may
/// outlive every turn that touched it. The address is the form's own `id`
/// (`<run>/<step>/<pass>/<attempt>`), so a page answering two forms in one gesture is two
/// uplinks, and a second device answering the same one is the same (step, pass, attempt)
/// — which `advance` already treats as a repeat, not a second completion.
fn answer(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    let user = sender_user(envelope);
    let args = payload(envelope);
    let settings = read_settings(ctx)?;
    let store_url = settings.store_url.trim_end_matches('/').to_string();
    let member = match user.as_deref() {
        Some(user) => user.to_string(),
        None => args
            .get("member_id")
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())))
            .unwrap_or_else(|| "page".to_string()),
    };
    // THE ADDRESS RIDES THE ENVELOPE, not the fields: a page's form sends `Outflow {dest, id,
    // data}`, and the venue puts `id` beside the device and the sender — it is a fact about the
    // delivery, not something the person typed. A direct caller (a CLI, a test) has no venue to
    // say it, so it may put `id` in the payload instead; both are the same address, read in that
    // order rather than duplicated into the payload by every sender.
    let address = args
        .get("id")
        .and_then(Value::as_str)
        .or_else(|| envelope.get("id").and_then(Value::as_str))
        .context("answer: neither the envelope nor the payload names the step — a form's id IS its address")?;
    let (run_id, step, pass, attempt) = parse_address(address)?;
    // WHAT THE PERSON SAID, as values. A bundle uplink's payload is the packed object itself
    // (`{tag: value}` — fluxen ADR 0032 §5, no per-field wrapper), which is exactly what a plan
    // reads (`$.ask.side`). The unwrap stays for a sender that still wraps one field in `{data}`;
    // the answer's address is the bundle's `id`, read off the envelope below.
    let output = answer_values(args);
    let mounts = mounts_of_settings(&settings, &json!({}));
    // A finished call closes the tasks bound to the code row that ran (ADR-0003 §3) — the
    // same fact a turn reads, fetched from the same place, so an answer and a turn agree
    // about what work a step ends.
    let owed = fetch_tasks(&store_url, settings.memory_scope);
    let ends_on = gravity_booth::tasks::bindings(owed.as_deref().unwrap_or_default());

    let world = gravity_booth::tools::World {
        store_url: &store_url,
        memory_scope: Some(settings.memory_scope),
        mounts: &mounts,
        session: &member,
        ends_on: &ends_on,
    };
    let value = {
        let mut doors = VenueDoors { ctx };
        let mut walked = Vec::new();
        gravity_booth::tools::call(
            &mut doors,
            "advance",
            &json!({
                "run_id": run_id,
                "step": step,
                "pass": pass,
                "attempt": attempt,
                "output": output,
            }),
            &world,
            &mut walked,
        )
        .map_err(anyhow::Error::msg)?
    };

    // Say what is next on the SAME channel the question went out on: another form, or
    // the run's answer. Nothing is said when the answer was a repeat — the page has
    // already heard the consequence of the first delivery.
    let page = ui_frames(
        run_id,
        value
            .get("ran")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default(),
    );
    if !page.is_empty() {
        emit(ctx, RUN_PAGE_EVENT, to(user.as_deref(), device.as_deref()), None, json!(page))?;
    }
    let awaiting = awaiting_of(&value, &json!({}));
    if let Some(awaiting) = awaiting {
        let forms = request_forms(&awaiting);
        if !forms.is_empty() {
            emit(ctx, RUN_PAGE_EVENT, to(user.as_deref(), device.as_deref()), None, json!(forms))?;
        }
    } else if value.get("written").and_then(Value::as_bool).unwrap_or(false) {
        emit(
            ctx,
            RUN_DONE_EVENT,
            to(user.as_deref(), device.as_deref()),
            None,
            json!({
                "run_id": run_id,
                "complete": value.get("complete").cloned().unwrap_or(json!(false)),
                "blocked": value.get("blocked").cloned().unwrap_or(json!([])),
            }),
        )?;
    }
    Ok(json!({ "done": true, "written": value.get("written") }))
}

/// `<run>/<step>/<pass>/<attempt>` — the address a form's `id` carries.
fn parse_address(address: &str) -> anyhow::Result<(u64, String, u32, u32)> {
    let mut parts = address.rsplitn(4, '/');
    let attempt = parts.next().context("answer: no attempt in the address")?;
    let pass = parts.next().context("answer: no pass in the address")?;
    let step = parts.next().context("answer: no step in the address")?;
    let run_id = parts.next().context("answer: no run in the address")?;
    Ok((
        run_id.parse().context("answer: the run id is not a number")?,
        step.to_string(),
        pass.parse().context("answer: the pass is not a number")?,
        attempt.parse().context("answer: the attempt is not a number")?,
    ))
}

/// `<collection>:<row>` — the address a deployment screen's bundle carries as its `id`.
///
/// One bundle, one submission, one address; the pack underneath is the row document. The
/// separator is `:` so it cannot be confused with a run answer's four-part `/` address.
fn parse_save_address(address: Option<&str>) -> anyhow::Result<(String, u64)> {
    let address = address.context(
        "save: the payload names neither `collection` nor `id`, and the delivery carried no \
         bundle id to read them from",
    )?;
    let (collection, id) = address
        .split_once(':')
        .with_context(|| format!("save: the bundle's id is `<collection>:<row>`, got {address:?}"))?;
    Ok((
        collection.to_string(),
        id.trim()
            .parse()
            .with_context(|| format!("save: the row in {address:?} is not a number"))?,
    ))
}

fn save(ctx: &mut Ctx, envelope: &Value) -> anyhow::Result<Value> {
    let device = device_of(envelope);
    let user = sender_user(envelope);
    let args = payload(envelope);
    match write_row(ctx, args, envelope.get("id").and_then(Value::as_str)) {
        Ok(saved) => {
            emit(ctx, SAVED_EVENT, to(user.as_deref(), device.as_deref()), None, saved)?;
            Ok(json!({ "done": true }))
        }
        Err(error) => {
            let detail = format!("{error:#}");
            let _ = emit(
                ctx,
                SAVE_REFUSED_EVENT,
                to(user.as_deref(), device.as_deref()),
                None,
                json!({ "detail": detail }),
            );
            // THE PERSON SEES IT TOO. The fact above is for programs; a page that only got that sees
            // nothing at all (prism injects a non-list payload into a slot nobody binds). So the same
            // refusal rides into the notice slot as a row — the door says the line, the renderer
            // decides how long it stays on screen.
            let mut rows = Vec::new();
            let _ = say_notice(&mut rows, &detail)
                .and_then(|()| emit(ctx, NOTICE_EVENT, to(user.as_deref(), device.as_deref()), None, json!(rows)));
            Err(error)
        }
    }
}

/// Write one deployment row. `settings` is row 1 by construction (one deployment, one settings
/// row); `models` takes whatever id the caller names, so a deployment can hold several recipes.
///
/// TWO SHAPES, ONE RULE: a caller hands the row directly (`{collection, id, doc}` — a script, a
/// test, an AI), or the SCREEN hands the bundle's pack, which is `{tag: value}` (fluxen ADR
/// 0032) — the row document itself, minus the address, which rides the bundle's `id`
/// (`<collection>:<row>`, handed back as the uplink's `id`).
fn write_row(ctx: &mut Ctx, args: &Value, address: Option<&str>) -> anyhow::Result<Value> {
    let mut row = args
        .as_object()
        .context("save: the payload is a row document or a form's fields")?
        .clone();
    let explicit = row.remove("doc");
    if explicit.is_none() {
        // An older/other sender's shape: every value is a `{data, payload}` message. Kept
        // because it costs one check and a sender that still wraps is not worth refusing.
        for value in row.values_mut() {
            if let Some(data) = value.get("data").cloned() {
                *value = data;
            }
        }
    }
    // WHERE THE ROW IS: stated by a program, or read off the bundle's id by a screen. Never a
    // field — a field is a name the pack carries, and the address is not one.
    let stated = row.contains_key("collection") || row.contains_key("id");
    let (collection, id) = if stated {
        let collection = row
            .remove("collection")
            .and_then(|c| c.as_str().map(str::to_string))
            .context("save: `collection` is `settings` or `models`")?;
        let id = row
            .remove("id")
            .and_then(|id| id.as_u64().or_else(|| id.as_str().and_then(|s| s.trim().parse().ok())))
            .context("save: `id` is the row's number (`settings` is 1)")?;
        (collection, id)
    } else {
        parse_save_address(address)?
    };
    let kind = match collection.as_str() {
        "settings" => KIND_SETTINGS,
        "models" => KIND_MODELS,
        other => anyhow::bail!(
            "save: `{other}` is not a collection a deployment configures (`settings`, `models`)"
        ),
    };
    // A direct caller's `doc` is used as given; a form's pack IS the doc.
    let mut doc = match explicit {
        Some(doc) => doc,
        None => Value::Object(row),
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

    fn put(&mut self, url: &str, body: &Value) -> Result<Value, String> {
        put_json(url, body)
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

/// THE CONVERSATION IS THE PERSON'S, and this engine opens it.
///
/// A turn used to need a channel that somebody else had already made; on a fresh deployment
/// nobody had, so the first thing a person said was refused. Channels are gravity's business
/// (`k10r` stores them, the engine decides them), and the decision here is the smallest one
/// that can hold: ONE conversation per member, opened on first use.
///
/// The id is DERIVED from the member rather than allocated, so two turns racing on a member's
/// first message open the same channel instead of two — `put_channel` is idempotent, and a
/// derived id keeps it so without a mapping table to keep in step. A u64 hash over the member's
/// text collides only if two members hash alike, which one deployment will not see; the day a
/// deployment does, the fix is a mapping row, not a different hash.
const GRAVITY_APP_ID: &str = "4f1b6c1e-0d3a-5f2b-9c47-6a1d0e5b8c22";

fn channel_id_for(member: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in member.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// The member's conversation: the first channel they are in, or the one this engine opens.
fn channel_of(store_url: &str, member: &str) -> anyhow::Result<u64> {
    let listed = ureq::get(&format!("{store_url}/members/{member}/channels"))
        .call()
        .with_context(|| format!("turn: asking the store which channels {member} is in failed"))?;
    let rows: Value =
        serde_json::from_reader(listed.into_reader()).context("turn: bad channel list")?;
    if let Some(open) = rows.as_array().and_then(|rows| {
        rows.iter()
            .find_map(|row| row.get("channel_id").and_then(Value::as_u64))
    }) {
        return Ok(open);
    }

    let channel = channel_id_for(member);
    ureq::post(&format!("{store_url}/channels/{channel}"))
        .send_json(json!({
            "app_id": GRAVITY_APP_ID,
            "scene": "conversation",
            "created_ms": now_ms(),
        }))
        .map(|_| ())
        .or_else(|e| match e {
            // 204 is what a fresh open answers; a repeat open is the same row, so a body-less
            // answer is success too — the door does not say which, and does not need to.
            ureq::Error::Status(code, _) if (200..300).contains(&code) => Ok(()),
            other => Err(format!("turn: opening channel {channel} failed: {other}")),
        })
        .map_err(anyhow::Error::msg)?;
    ureq::post(&format!("{store_url}/channels/{channel}/members"))
        .send_json(json!({
            "member_id": member,
            // THE NAME IS NOT OURS TO COPY. A member row carries a `display_name` that somebody
            // SETS (the person, or the booth that owns names); the login name lives in the venue's
            // auth store and never crosses into this one, and echoing the id into the display
            // column is a copy that reads as a name and is not one. Empty until somebody sets it.
            "display_name": "",
            "role": 0,
            "since_ms": now_ms(),
        }))
        .map(|_| ())
        .or_else(|e| match e {
            ureq::Error::Status(code, _) if (200..300).contains(&code) => Ok(()),
            other => Err(format!("turn: joining channel {channel} failed: {other}")),
        })
        .map_err(anyhow::Error::msg)?;
    Ok(channel)
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

/// One PUT to k10r — the run snapshot, replaced whole.
fn put_json(url: &str, body: &Value) -> Result<Value, String> {
    match ureq::put(url).send_json(body.clone()) {
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

    /// A person's answer reaches the log as VALUES, not as the wire's envelope: the form sends
    /// `{side: {data: "right", payload: {...}}}`, and a plan binding `$.ask.side` must read
    /// "right". The first real browser answer was recorded as `null` because the handler looked
    /// for a second `data` inside the field map — the shape nobody sends.
    #[test]
    fn an_answer_is_recorded_as_the_values_the_person_gave() {
        let sent = json!({
            "side": { "data": "right", "payload": { "kind": "field", "field": "side" } },
        });
        assert_eq!(answer_values(&sent), json!({ "side": "right" }));
        // A direct caller already sends values; nothing is rewritten.
        assert_eq!(answer_values(&json!({ "side": "right" })), json!({ "side": "right" }));
    }

    /// The form a waiting run sends IS the answer's address: the row lands in the
    /// conversation, its `id` is the step it answers, and its `sink` names the door. The
    /// fields are named by their own `tag` — not by the slot they land in, and not by the
    /// bundle's id (fluxen ADR 0032 §2).
    #[test]
    fn a_waiting_run_is_drawn_as_an_addressed_bundle_in_the_stream() {
        let waiting = json!({
            "run_id": 17,
            "requests": [{
                "step": "ask", "pass": 0, "attempt": 0,
                "form": { "title": "How many?", "fields": [
                    { "name": "branch", "type": "string" },
                    { "name": "count", "type": "integer" },
                    { "name": "sure", "type": "bool" },
                    { "name": "side", "type": "choice", "options": ["left", "right"] },
                ]},
            }],
        });
        let frames = request_forms(&waiting);
        assert_eq!(frames.len(), 1);
        let frame = &frames[0];
        assert_eq!(frame["action"], "append");
        assert_eq!(frame["slot"], MESSAGES_SLOT, "the form is a message, not a region");
        let bundle = &frame["data"];
        assert_eq!(bundle["type"], "bundle", "accrete's own bundle, not a form");
        assert_eq!(bundle["id"], "17/ask/0/0", "the address the answer comes back on");
        assert_eq!(bundle["attrs"]["selector"], "assistant");
        // WHEN it leaves, and it is the button it drew: a bundle that declared only `enter`
        // would not draw a button at all.
        assert_eq!(
            bundle["attrs"]["submit"],
            json!(["enter", "button"]),
            "enter sends and the button is the one the bundle declared: {bundle}"
        );
        // THE EXIT IS THE BUNDLE'S (ADR 0032 §1), and `remote` is the default so it stays off
        // the wire.
        assert_eq!(bundle["sink"], json!({ "slot": "run.answer" }), "{bundle}");
        // The widgets follow the DECLARED kind, in accrete's smaller vocabulary. A choice has
        // no form widget (a select reads its rows from a list slot), so it is a text field
        // rather than a select nobody could pick from.
        let children = bundle["children"].as_array().unwrap();
        let kinds: Vec<&str> = children
            .iter()
            .map(|child| child["type"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(kinds, vec!["input", "input", "input", "input", "button"], "{bundle}");
        let tags: Vec<&str> = children
            .iter()
            .filter_map(|child| child["tag"].as_str())
            .collect();
        assert_eq!(
            tags,
            vec!["branch", "count", "sure", "side"],
            "a field is named by its tag: {bundle}"
        );
        let widgets: Vec<&str> = children
            .iter()
            .filter_map(|child| child["source"]["type"].as_str())
            .collect();
        assert_eq!(widgets, vec!["text", "number", "bool", "text"], "{bundle}");
        // No field borrows an exit: the pack's shape is `{tag: value}`. (The BUTTON's `sink` is
        // not an exit for a value — it is what makes it the confirm key.)
        assert!(
            children
                .iter()
                .filter(|child| child["type"] != "button")
                .all(|child| child.get("sink").is_none()),
            "an inner field has no exit of its own: {bundle}"
        );
        let button = children.last().unwrap();
        assert_eq!(
            button["sink"],
            json!({}),
            "a sink is what makes a button the bundle's confirm key"
        );
        assert_eq!(button["source"]["default"], "Send", "a label, not a type");
        assert_eq!(button["attrs"]["oneshot"], true);

        assert_eq!(parse_address("17/ask/0/0").unwrap(), (17, "ask".into(), 0, 0));
        assert!(parse_address("17/ask").is_err(), "an address is four parts");
    }

    /// The composer: one tagged field, an exit on the bundle, and BOTH timings — the wire shape
    /// the chat box depends on. A field that borrowed a slot name as its key, or a button
    /// without a `sink`, would draw something that cannot send.
    #[test]
    fn the_composer_packs_one_tagged_field() {
        let composer = frame_composer();
        assert_eq!(composer["type"], "bundle");
        assert_eq!(composer["sink"], json!({ "slot": "turn.submit" }));
        assert_eq!(composer["attrs"]["submit"], json!(["enter", "button"]));
        let field = &composer["children"][0];
        assert_eq!(field["tag"], "user_message", "the field names itself");
        assert_eq!(field["source"]["default"], "");
        assert!(
            field.get("sink").is_none(),
            "the field keeps no exit — the bundle is the exit"
        );
        let button = &composer["children"][1];
        assert_eq!(button["sink"], json!({}), "a button with no sink is not drawn");
        assert_eq!(button["source"]["default"], "发送");
    }

    /// The chunk pointer names the text child's value where it now lives — `/source/default`,
    /// with no `bind` map around it (ADR 0031 §2). A pointer one key off is a stream that
    /// silently appends nowhere.
    #[test]
    fn a_chunk_is_patched_into_the_text_childs_source() {
        let bubble = frame_message("9_1", "assistant", "");
        assert_eq!(bubble["data"]["children"][0]["source"]["default"], "");
        let chunk = frame_chunk("9_1", "hi");
        assert_eq!(chunk["path"], "/children/0/source/default");
        assert_eq!(chunk["slot"], MESSAGES_SLOT);
        assert_eq!(chunk["id"], "9_1", "addressed by row, never by a shared slot");
    }

    use super::*;

    /// Every node `type` a frame tree carries, and a refusal of the retired `bind` map,
    /// wherever either hides. A key that a decoder DROPS (no `deny_unknown_fields`) is the
    /// failure mode this catches: the frame parses and draws nothing.
    ///
    /// A `type` is a NODE's only when the object is one: a channel carries `type` too
    /// (`source.type` is the widget's `JsType`), and reading that as a node name would make
    /// this check lie.
    fn walk(node: &Value, kinds: &mut Vec<String>) {
        match node {
            Value::Object(map) => {
                let is_node = ["attrs", "children", "source", "sink", "tag", "style"]
                    .iter()
                    .any(|key| map.contains_key(*key));
                if is_node {
                    if let Some(kind) = map.get("type").and_then(Value::as_str) {
                        kinds.push(kind.to_string());
                    }
                }
                assert!(
                    !map.contains_key("bind"),
                    "the `bind` map is retired (ADR 0031 §2): {node}"
                );
                for value in map.values() {
                    walk(value, kinds);
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, kinds);
                }
            }
            _ => {}
        }
    }

    /// THE REPRESENTATIVE FRAME SET: one turn's whole surface as one action list (a form, the
    /// person's line, a chunk, a run's request forms, a notice, and the hosts + settings
    /// bundle) — the ~10 frames a real answer draws, not a hand-picked scalar. Both the dump
    /// below and the size measurement use it, so the numbers in ADR-0013 are this set's.
    fn sample_frames() -> Vec<Value> {
        let mut frames = frame_root(
            vec![frame_form(
                "models",
                "1",
                "the model this deployment answers with",
                &[("alias", "default", FieldKind::Text, &Value::Null)],
            )],
            false,
        );
        frames.push(frame_message("9_0", "user", "hi"));
        frames.push(frame_chunk("9_1", "there"));
        frames.extend(request_forms(&json!({
            "run_id": 17,
            "requests": [{
                "step": "ask", "pass": 0, "attempt": 0,
                "form": { "fields": [
                    { "name": "branch", "type": "string" },
                    { "name": "count", "type": "integer" },
                    { "name": "sure", "type": "bool" },
                    { "name": "side", "type": "choice", "options": ["left", "right"] },
                ]},
            }],
        })));
        frames.push(json!({ "action": "append", "slot": NOTICE_SLOT, "data": {
            "id": "notice-1", "type": "frame", "attrs": {"class": ["box"]},
            "children": [{ "type": "text", "source": { "default": "saved" } }],
        }}));

        // The hosts and the settings bundle are in the list too, so the sample covers what a
        // person actually walks: shell → hosted region → form → answer.
        frames.push(json!({ "action": "set", "slot": "", "data": frame_root(vec![], true)[0]["data"] }));
        frames
    }

    /// THE WHOLE SURFACE IN ONE ACTION LIST, written where fluxen's own decoder can judge it:
    ///
    ///     cd ../fluxen && ./target/debug/stage tojson <printed path>
    ///
    /// The assertions here are the cheap half — every node name is one the model still has,
    /// and no frame carries the retired `bind` map. The model is the half that cannot drift,
    /// because it is the thing that refuses.
    #[test]
    fn the_frames_are_written_for_the_model_to_take_or_refuse() {
        let frames = sample_frames();

        const CURRENT: &[&str] = &[
            "frame", "canvas", "forcegraph", "panel", "bundle", "svg", "path", "tiles", "entry",
            "button", "image", "input", "select", "text", "stamp",
        ];
        let mut kinds = Vec::new();
        for frame in &frames {
            walk(frame, &mut kinds);
        }
        for kind in &kinds {
            assert!(
                CURRENT.contains(&kind.as_str()),
                "`{kind}` is not a node any more: {kinds:?}"
            );
        }
        // Belt and braces for the words that moved rather than vanished.
        assert!(!kinds.iter().any(|k| k == "rack"), "rack is `tiles`");
        assert!(!kinds.iter().any(|k| k == "form"), "form is `bundle`");
        assert!(!kinds.iter().any(|k| k == "pages"), "pages is `tiles` + a window/step");
        assert!(!kinds.iter().any(|k| k == "float"), "float is `panel` / a frame");
        assert!(
            kinds.iter().filter(|k| *k == "bundle").count() >= 3,
            "the settings form, the composer and the run answer: {kinds:?}"
        );

        let path = std::env::var("GRAVITY_FRAMES_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir().join("gravity-accrete-frames.json"));
        // THE DUMP FOLLOWS THE CONTENT CODEC TOO (ADR-0013): in JSON mode this is the file it
        // has always been, byte for byte; in CBOR mode it is the SAME list, one CBOR
        // document — which is what lets the verification decode it back to JSON and hand
        // the result to `stage tojson` unchanged. A test runs outside a carrier, so nothing
        // declared a codec: this reads the SERVICE DEFAULT (`GRAVITY_WIRE`), which is what
        // that variable still means.
        let codec = wire::declared();
        let bytes = match codec {
            wire::Codec::Json => serde_json::to_string_pretty(&frames)
                .expect("the frame list serializes")
                .into_bytes(),
            wire::Codec::Cbor => codec.encode(&Value::Array(frames.clone()))
                .expect("the frame list encodes as one CBOR document"),
        };
        std::fs::write(&path, bytes).unwrap();
        println!(
            "accrete frames ({}) → {}",
            codec.as_str(),
            path.display()
        );
    }

    /// THE WIRE CONTRACT OF THE FUNNEL, both encodings: JSON is the envelope that went in,
/// field for field (the regression that matters), and CBOR changes exactly ONE field —
/// `data` — leaving the head readable so the venue can route without decoding content.
    #[test]
    fn the_funnel_encodes_only_the_content_and_leaves_the_head_readable() {
        let envelope = json!({
            "name": "ui:turn.chunk",
            "island": "gravity-model",
            "turn": "7",
            "device": "d-1",
            "data": [{ "action": "patch", "slot": "messages", "id": "7_1",
                       "path": "/children/0/source/default", "op": "append", "value": "!" }],
        });

        // JSON: untouched. This is today's wire, byte for byte.
        assert_eq!(
            carried(envelope.clone(), wire::Codec::Json).unwrap(),
            envelope
        );

        // CBOR: the head is the same, `data` is the same CONTENT after one unwrap, and no
        // field was added or lost.
        let cbor = carried(envelope.clone(), wire::Codec::Cbor).unwrap();
        for head in ["name", "island", "turn", "device"] {
            assert_eq!(cbor[head], envelope[head], "the routing head stays plain: {head}");
        }
        assert_eq!(
            cbor.as_object().map(serde_json::Map::len),
            envelope.as_object().map(serde_json::Map::len),
            "the axis swaps one value, it does not reshape the frame"
        );
        let bytes =
            induction_guest::payload_bytes(&cbor["data"]).expect("the carried data is tagged");
        assert_eq!(
            ciborium::from_reader::<Value, _>(bytes.as_slice()).expect("decodes"),
            envelope["data"],
            "what the venue gets back is what the JSON path sends"
        );
    }

    /// THE FOUR SIZES OF ONE FRAME SET, measured rather than asserted — the numbers the
    /// decision to keep the CBOR path rests on (ADR-0013, "is it worth it"), re-taken with the
    /// BYTE PATH in place.
    ///
    /// The four are one frame at four points, in the order it travels:
    ///
    /// (a) pretty JSON, (b) compact JSON — the content as `data`, no codec (today's debug wire);
    /// (c) the frame AT THE REALM SEAM: the carrier lifted the byte string and re-spelled it as
    ///     `{"$cbor": base64}` because the host fn it hands frames to takes a `serde_json::Value`;
    /// (d) the frame ON THE PIPE, before that seam: one CBOR document whose `data` is a real
    ///     byte string (`0x5a …`), which is what `Ctx::emit_bytes` writes and the carrier reads.
    ///
    /// (d) is the one that used to be impossible to measure here — the doc comment on this test
    /// said a `Value`-typed hop could not carry bytes, so (c) stood in for it. The pipe CAN
    /// carry them now (`induction_protocol::split_bytes` lifts the byte string out of the tree
    /// instead of failing); the hop that re-spells it is the REALM SEAM (aura's `dyn_to_json`,
    /// the `Fn(&Ctx, Value)` bridge), and (c) − (d) is exactly what that seam costs.
    ///
    ///     cargo test -p gravity-booth --bin gravity-turn the_four_payload_sizes -- --nocapture
    #[test]
    fn the_four_payload_sizes() {
        let frames = sample_frames();
        let set = Value::Array(frames.clone());

        let pretty = serde_json::to_string_pretty(&set).expect("pretty").len();
        let compact = serde_json::to_vec(&set).expect("compact").len();
        // The content as the booth's declared codec encodes it: this is the document the guest
        // hands over, at every hop below. (Not "the payload" — the CARRIER of it differs.)
        let content = wire::Codec::Cbor.encode(&set).expect("cbor");

        // THE FRAME THE GUEST WRITES: `emit_bytes`' own shape — the JSON mapping, which
        // `CborWriter` resolves into a byte string in the document it puts on stdout.
        let crossing = json!({
            "host": { "type": "emit", "args": {
                "event": "ui:turn.done",
                "data": induction_guest::cbor_payload(&content),
            }},
        });
        // (d) ON THE PIPE: the same frame as one CBOR document, the payload a byte string.
        let doc = induction_protocol::cbor_frame(&crossing).expect("frame encodes");
        let mut pipe = Vec::new();
        ciborium::into_writer(&doc, &mut pipe).expect("write");
        let raw_cbor = pipe.len();

        // (c) AT THE SEAM: the carrier's two steps, run for real — lift the byte string out of
        // the tree (`split_bytes`, the read loop) and put the JSON mapping back where it sat
        // (`FrameBytes::embed`, the `Value`-typed host's fallback in `emit_bytes`).
        let (mut at_seam, lifted) = induction_protocol::split_bytes(doc.clone()).expect("splits");
        let lifted = lifted.expect("the frame carried bytes");
        assert!(lifted.embed(&mut at_seam), "the payload goes back where it sat");
        let on_wire = serde_json::to_vec(&at_seam).expect("seam frame").len();
        let b64 = at_seam["host"]["args"]["data"][induction_guest::CBOR_PAYLOAD_KEY]
            .as_str()
            .expect("the wrapper holds base64 text")
            .len();

        println!(
            "accrete frame set: {} frames · content {} B · \
             (a) pretty JSON {pretty} B · \
             (b) compact JSON {compact} B · \
             (c) frame at the realm seam ({{\"$cbor\": base64}}) {on_wire} B (base64 alone {b64} B) · \
             (d) frame on the pipe, raw CBOR with a byte string {raw_cbor} B",
            frames.len(),
            content.len(),
        );

        // The relationships that make the numbers a DECISION rather than trivia: CBOR is
        // smaller than compact JSON, the pipe carries it as bytes, and the seam's base64 is
        // where the 4/3 comes back — a property of the SEAM, not of the codec.
        assert!(raw_cbor < compact, "raw cbor {raw_cbor} < compact json {compact}");
        assert!(
            on_wire > raw_cbor,
            "base64 costs: seam {on_wire} > pipe {raw_cbor}"
        );
        // The payload there is a real byte string, not the mapping, and what crossed is the
        // content BYTE FOR BYTE: the lift above found bytes (not text), and they are the same
        // document the codec produced. That is the difference the byte path bought.
        assert_eq!(lifted.bytes, content, "the pipe carried the content itself");
    }

    /// The model hears the outcome, not the ledger: an answer that names every ready
    /// step and every ran one makes the conversation grow with the engine's bookkeeping.
    #[test]
    fn the_model_hears_the_outcome_and_not_the_ledger() {
        // A waiting run: the status, the run, and the field names a person will fill.
        let waiting = conclusion(&json!({
            "ok": true, "run_id": 9, "written": true,
            "ready": [], "requests": [{ "step": "ask", "pass": 0, "attempt": 0,
                "form": { "fields": [{ "name": "branch", "type": "choice",
                                       "options": ["left", "right"] }] } }],
            "ran": [{ "step": "fetch", "output": { "body": "x" } }],
            "complete": false, "stalled": false,
        }));
        assert_eq!(waiting["status"], "waiting", "{waiting}");
        assert_eq!(waiting["asks"][0]["fields"], json!(["branch"]), "{waiting}");
        assert!(waiting.get("ran").is_none(), "the walk is not the model's: {waiting}");

        // A finished run: its result, which is the whole point.
        let complete = conclusion(&json!({
            "ok": true, "run_id": 9, "complete": true, "result": { "total": 9 },
        }));
        assert_eq!(complete["status"], "complete");
        assert_eq!(complete["result"]["total"], 9);

        // A failed step is said plainly, because it is why the ask went unanswered.
        let failed = conclusion(&json!({
            "ok": true, "run_id": 9, "complete": false,
            "ran": [{ "step": "fetch", "failed": true, "reason": "the node did not answer" }],
            "requests": [],
        }));
        assert_eq!(failed["status"], "failed");
        assert!(failed["failed"]["reason"]
            .as_str()
            .unwrap()
            .contains("did not answer"));

        // Anything that is not a run answer is passed through untouched.
        let other = conclusion(&json!({ "outcome": "committed", "id": 3 }));
        assert_eq!(other, json!({ "outcome": "committed", "id": 3 }));
    }

    /// A `send` to the page becomes a row removal in the stream the forms went into,
    /// addressed by the answer's own id — so a second device watching the conversation
    /// sees the form go when the first one answers.
    #[test]
    fn a_clear_send_becomes_a_removal_in_the_stream() {
        let ran = vec![json!({
            "step": "close", "subject": "ui",
            "clear": [{ "step": "ask", "pass": 0, "attempt": 0 }],
        })];
        assert_eq!(
            ui_frames(9, &ran),
            vec![json!({ "action": "remove", "slot": MESSAGES_SLOT, "id": "9/ask/0/0" })]
        );
        // `start` answers the run id; `advance` was handed it. Both compose the same ids.
        assert_eq!(
            page_updates(
                &json!({ "run_id": 4, "ran": [{ "subject": "ui",
                        "clear": [{ "step": "ask", "pass": 1, "attempt": 0 }] }] }),
                &json!({})
            ),
            vec![json!({ "action": "remove", "slot": MESSAGES_SLOT, "id": "4/ask/1/0" })]
        );
        assert_eq!(
            page_updates(
                &json!({ "ran": [{ "subject": "ui",
                        "clear": [{ "step": "ask", "pass": 0, "attempt": 0 }] }] }),
                &json!({ "run_id": 6 })
            ),
            vec![json!({ "action": "remove", "slot": MESSAGES_SLOT, "id": "6/ask/0/0" })]
        );
        // A send to a store subject says nothing to the page.
        assert!(page_updates(
            &json!({ "run_id": 4, "ran": [{ "subject": "memory/fact", "output": [] }] }),
            &json!({})
        )
        .is_empty());
    }

    /// A run that waits on a person is the one answer a page must act on, and it
    /// carries the address the answer comes back through.
    #[test]
    fn a_waiting_run_becomes_a_page_question() {
        let answer = json!({
            "ok": true, "run_id": 17, "complete": false,
            "requests": [{ "step": "ask", "pass": 0, "attempt": 0,
                           "form": { "fields": [{ "name": "branch", "type": "choice" }] } }],
        });
        let said = awaiting_of(&answer, &json!({ "skill_id": 9 })).expect("a question");
        assert_eq!(said["run_id"], 17);
        assert_eq!(said["skill_id"], 9);
        assert_eq!(said["requests"][0]["step"], "ask");

        // Nothing waiting: nothing to ask.
        assert!(awaiting_of(&json!({ "requests": [] }), &json!({})).is_none());
        assert!(awaiting_of(&json!({ "ok": true }), &json!({})).is_none());
    }

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
