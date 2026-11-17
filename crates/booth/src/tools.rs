//! The fixed tool list (ADR-0004): hardcoded, grouped, and carrying the names
//! k10r's tool face already has (`docs/SEAM.md` §2).
//!
//! Ten entries, because ten doors answer: k10r's HTTP surface (data access — the
//! memory, skill and task verbs) and the node mounts (code execution, ADR-0001 —
//! `start` runs a skill as a run, `run` runs a row no skill names). The list
//! is fixed
//! because gravity is the top of the stack and a pure glue: its memory and
//! skills ARE k10r's and its execution surface IS the node's, so there is
//! nothing per-deployment to discover here. Names are k10r's; the ADR fixed
//! them so nothing gets renamed later, and the list grows by door, never by
//! enthusiasm. Listing a tool the substrate cannot serve would teach the model
//! that failing tools are normal.
//!
//! The list is static on purpose: the `tools` array is part of the request's
//! stable prefix, so the same bytes ride every turn.
//!
//! Two kinds of door, one dispatch: [`Doors`] is the seam. The turn owns the
//! implementation (HTTP for k10r, `ctx.invoke` for a mount); this module owns
//! the list, the argument reading and the error vocabulary, which is what the
//! tests pin.

use serde_json::{json, Value};

/// How a tool's execution behaves (ADR-0004 §6) — the field Phase 2.5's cold
/// path reads and the one Phase 5's `tool_invoke_count` hangs on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nature {
    /// In-process, idempotent: repeated for free, no outside effects.
    Fast,
    /// Touches the outside world or changes state.
    Human,
}

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// The JSON-schema builder for the arguments. A function rather than a
    /// literal because `serde_json::Value` cannot be built in a `static`.
    pub parameters: fn() -> Value,
    pub nature: Nature,
}

/// The list. Order is the order the model sees.
static SPECS: &[ToolSpec] = &[
    ToolSpec {
        name: "recall_memory",
        description: "Recall what is known over the long term. k10r lands the query in the \
                      fact graph and answers with the facts around it, rendered as text. Use it \
                      when the answer depends on something you were not told in this conversation; \
                      the turn-start projection already carries the composed head.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What to recall, in natural language."
                    }
                },
                "required": ["query"]
            })
        },
        nature: Nature::Fast,
    },
    ToolSpec {
        name: "commit_memory",
        description: "Write facts into the long-term graph. Each fact is an 8-slot tuple: \
                      [head value, head type, head props, relation name, relation props, \
                      tail value, tail type, tail props]. Props are JSON objects (`{}` when \
                      empty). Asserting the same head, relation and tail again does not add a \
                      second fact: k10r adjudicates functional relations at the same source and \
                      treats a repeated assertion as the same edge, so the answer says which of \
                      linked / rewritten / dropped / bumped happened — `bumped` means the fact \
                      already stood and nothing was written.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "facts": {
                        "type": "array",
                        "description": "The 8-slot fact tuples to ingest.",
                        "items": {
                            "type": "array",
                            "minItems": 8,
                            "maxItems": 8
                        }
                    }
                },
                "required": ["facts"]
            })
        },
        nature: Nature::Human,
    },
    ToolSpec {
        name: "resolve_skills",
        description: "List the skills this deployment can run. Use it when the answer needs a \
                      capability the conversation did not hand you: k10r answers candidates by \
                      intent — each with a name, what it does, its steps and a score. A \
                      candidate is not a tool by itself; the call that runs one is \
                      `start`, which takes the id you pick here.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "What you are trying to do, in natural language."
                    },
                    "limit": {
                        "type": "integer",
                        "description": "How many candidates at most (default 5, at most 20)."
                    }
                },
                "required": ["query"]
            })
        },
        nature: Nature::Fast,
    },
    ToolSpec {
        name: "start",
        description: "Start a skill as a RUN. A skill whose `steps` is a plan (a graph of steps \
                      with control edges, gravity ADR-0011) is validated first, and what you get \
                      back is a `run_id` plus the steps that may run now. Call `advance` with the \
                      run's id and one step's result; it answers the next ready set. A plan that \
                      cannot run is refused with its problems — nothing is left half-started.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "skill_id": {
                        "type": "integer",
                        "description": "The `node_id` that `resolve_skills` answered."
                    },
                    "params": {
                        "type": "object",
                        "description": "Values the plan's steps read by name (the run's parameters)."
                    }
                },
                "required": ["skill_id"]
            })
        },
        nature: Nature::Fast,
    },
    ToolSpec {
        name: "advance",
        description: "Record one step's result in a run and get the next ready set. Pass the \
                      `step` you ran, its `output`, and (for a loop) the `pass` and `attempt` it \
                      was. Redelivering the same (step, pass, attempt) writes nothing twice — \
                      at-least-once delivery is expected. The answer carries the same shape as \
                      `start`: ready / complete / stalled.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "integer", "description": "The id `start` answered." },
                    "step": { "type": "string", "description": "The step id that finished." },
                    "output": { "description": "What the step produced; the run keeps it whole." },
                    "pass": { "type": "integer", "description": "Loop pass (default 0 — the first)." },
                    "attempt": { "type": "integer", "description": "Attempt within the pass (default 0)." }
                },
                "required": ["run_id", "step"]
            })
        },
        nature: Nature::Fast,
    },
    ToolSpec {
        name: "run",
        description: "Run a script by its id, with no skill in the way. This is the door for a \
                      code row YOU wrote with `write_code`, or one somebody handed you: exactly \
                      what `start` does with a step, minus the run. The row states its \
                      own language, and the mount that serves that language runs it; what you \
                      pass in `arguments` is what the script receives. Use `read_code` first if \
                      you need to see what it takes.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "code_id": {
                        "type": "integer",
                        "description": "The code row to run."
                    },
                    "arguments": {
                        "type": "object",
                        "description": "What the script receives as its arguments."
                    }
                },
                "required": ["code_id"]
            })
        },
        // It runs code: outside the process, on a node, with whatever effects
        // that code has (ADR-0004 §6).
        nature: Nature::Human,
    },
    ToolSpec {
        name: "create_skill",
        description: "Write down a procedure worth repeating, as a skill this deployment can later \
                      resolve and run. This is how a skill COMES INTO BEING: nothing else creates \
                      one, and a skill that is never written down is a procedure the next \
                      conversation has to reinvent. Use it when you worked something out that has \
                      steps and will be needed again — not for a one-off answer, and not for a \
                      fact (that is `commit_memory`). Declaring a name that already exists MERGES \
                      into it: send the parts again to extend one.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Short stable identifier, snake_case or kebab-case. It IS the \
                                       identity: declaring it twice declares it once."
                    },
                    "title": {
                        "type": "string",
                        "description": "One line a reader sees in the candidate list."
                    },
                    "content": {
                        "type": "string",
                        "description": "What the skill does and when to use it, in prose."
                    },
                    "preconditions": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "What must hold before the first step."
                    },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "code_id": { "type": "integer" },
                                "note": { "type": "string" }
                            }
                        },
                        "description": "The steps in execution order; each names a code row this \
                                        deployment stores and what that step is for."
                    },
                    "verification": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "What to check after the walk."
                    }
                },
                "required": ["name", "content"]
            })
        },
        nature: Nature::Human,
    },
    ToolSpec {
        name: "read_code",
        description: "Read a script back: its language, description, parameters and source, plus \
                      which version answered. Without `rev` it is the CURRENT version — the one \
                      `start` would run. With `rev` it is that exact version, which is how \
                      you see what an older run was given before you change it. Read before \
                      revising: `write_code` with a `code_id` replaces the script's meaning, and a \
                      blind edit is how a working step stops working.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "code_id": {
                        "type": "integer",
                        "description": "The script to read (the number a skill's step names)."
                    },
                    "rev": {
                        "type": "integer",
                        "description": "Which version. Omit for the current one."
                    }
                },
                "required": ["code_id"]
            })
        },
        nature: Nature::Fast,
    },
    ToolSpec {
        name: "write_code",
        description: "Deliver a script as a piece of code this deployment stores and can run, and \
                      get back the `code_id` a skill's step names. Give `code_id` to REVISE a script \
                      you delivered before: a correction appends a new version, so what ran \
                      yesterday stays readable and the address does not move (read it first with \
                      `read_code` when it is not yours from this turn). This is the writing half of \
                      execution — a procedure with real steps needs the scripts its steps run, or \
                      `start` has nothing to run.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "language": {
                        "type": "string",
                        "description": "The language the script is written in. The call is routed \
                                       to the mount serving it, and a language no mount serves \
                                       cannot be invoked — but it CAN be stored here."
                    },
                    "source": {
                        "type": "string",
                        "description": "The script body, verbatim. What the mount runs."
                    },
                    "description": {
                        "type": "string",
                        "description": "What it does, in one line — what you and others read when \
                                       deciding whether to reuse this script instead of writing \
                                       another one."
                    },
                    "parameters": {
                        "type": "object",
                        "description": "What the script expects as arguments (a JSON document; stored \
                                       as text and not parsed by the store)."
                    },
                    "code_id": {
                        "type": "integer",
                        "description": "The script to revise. Omit to deliver a NEW script. The \
                                       revision keeps the same `code_id`, so skills, tasks bound to \
                                       it and its counters all keep pointing at it."
                    }
                },
                "required": ["language", "source"]
            })
        },
        nature: Nature::Human,
    },
    ToolSpec {
        name: "open_task",
        description: "Open a task: work somebody has taken on and has not finished. A task is a \
                      VERTEX in the same graph as memory, not a separate list — so a dependency, a \
                      parent, a tag or a reference is an ordinary fact (`commit_memory` with type \
                      `task` and the task's name as the value), and a fact about a task is an \
                      ordinary fact. Opening a name that already exists only RESTATES it — its state, \
                      next action and clock are untouched, so calling this twice is \
                      safe — and the answer says whether this call created it. Use it for work that outlives this turn.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Stable identifier, snake_case or kebab-case. It IS the \
                                       identity: facts reach the task by this name, and opening it \
                                       twice opens it once."
                    },
                    "next": {
                        "type": "string",
                        "description": "The next action, in your own words. Optional; it is what \
                                       the next turn starts from."
                    },
                    "state": {
                        "type": "string",
                        "enum": ["open", "doing", "blocked", "done"],
                        "description": "Defaults to `open`."
                    },
                    "finishes_on": {
                        "type": "array",
                        "items": { "type": "integer" },
                        "description": "The code ids (`code_id`, the number a skill step names) that \
                                       FINISH this task. When one of them runs successfully the task is \
                                       closed at the call site — you do not move it afterwards. State it \
                                       when the work IS a step you are about to run."
                    }
                },
                "required": ["name"]
            })
        },
        nature: Nature::Human,
    },
    ToolSpec {
        name: "move_task",
        description: "Move a task: a new state, a new next action, or both. Absent fields are \
                      LEFT ALONE, so send only what changed — an empty string clears the next \
                      action, silence never does. A state outside the four is refused rather than \
                      stored, because a task in a state no listing looks for is a task nobody finds.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "integer",
                        "description": "The `node_id` a task was opened with or listed with."
                    },
                    "state": {
                        "type": "string",
                        "enum": ["open", "doing", "blocked", "done"]
                    },
                    "next": {
                        "type": "string",
                        "description": "The next action; `\"\"` clears it."
                    }
                },
                "required": ["task_id"]
            })
        },
        nature: Nature::Human,
    },
    ToolSpec {
        name: "list_tasks",
        description: "The tasks of this memory scope, oldest id first. This is where a turn \
                      starts: what is open is what the work is. `state` narrows the answer to one \
                      state; without it, every task the scope holds comes back.",
        parameters: || {
            json!({
                "type": "object",
                "properties": {
                    "state": {
                        "type": "string",
                        "enum": ["open", "doing", "blocked", "done"],
                        "description": "Only tasks in this state."
                    }
                },
                "required": []
            })
        },
        nature: Nature::Human,
    },
];

/// The entries, in the order the model sees them.
pub fn specs() -> &'static [ToolSpec] {
    SPECS
}

/// The nature declared for a tool, by name. `None` for a name the list does not
/// declare (a model can invent one; the caller-refusal path handles it).
///
/// This is the reader Phase 2.5's cold half was missing: `nature` was written on
/// every entry and read nowhere, which made the declaration a comment. The
/// writing side lists ten tools and no runtime registry — the list IS the
/// registry, so the lookup is here and not in a table of its own.
pub fn nature_of(name: &str) -> Option<Nature> {
    SPECS.iter().find(|s| s.name == name).map(|s| s.nature)
}

/// The names this deployment serves — the ones an unknown call is answered
/// with.
pub fn names() -> Vec<&'static str> {
    SPECS.iter().map(|spec| spec.name).collect()
}

/// The `tools` array for the provider request (OpenAI's function shape).
pub fn tools_array() -> Value {
    Value::Array(
        SPECS
            .iter()
            .map(|spec| {
                json!({
                    "type": "function",
                    "function": {
                        "name": spec.name,
                        "description": spec.description,
                        "parameters": (spec.parameters)(),
                    }
                })
            })
            .collect(),
    )
}

/// The two kinds of door a call reaches. k10r's HTTP surface is data access
/// (ADR-0007 as revised: HTTP is the single cross-process form); a mount is
/// code execution on a node (ADR-0001) and only the turn can reach it, because
/// only the turn holds the ctx.
pub trait Doors {
    fn post(&mut self, url: &str, body: &Value) -> Result<Value, String>;
    /// A REPLACEMENT: the body is the whole value at that address from now on.
    /// Separate from `post` because the store's run door means exactly that —
    /// a snapshot replaces the previous one, it does not append to it.
    fn put(&mut self, url: &str, body: &Value) -> Result<Value, String>;
    fn get(&mut self, url: &str) -> Result<Value, String>;
    /// `ctx.invoke(type_name, key, "execute", args)` — `execute` is the entry
    /// CONVENTION every runner serves (ADR-0001 §1), never a lookup.
    fn mount(&mut self, type_name: &str, key: &str, args: Value) -> Result<Value, String>;
}

/// The deployment facts a call reads. All of them come from the turn args and
/// none has a default: a namespace, a mount table and a residency key are the
/// deployment's declarations, and a guess would silently point at someone
/// else's graph or instance (ADR-0004 §5).
/// THE WORLD an execution is given: everything a step needs that the plan does not say.
///
/// Not to be confused with a memory-graph *fact* — the name this carried before said
/// "facts", which is what k10r stores, but nothing here is one: this is where results go
/// (`store_url`, `memory_scope`), what can run them (`mounts`), under which residency
/// (`session`), and which open work they close (`ends_on`). A plan names none of it; the
/// host hands it over, per call.
pub struct World<'a> {
    pub store_url: &'a str,
    /// k10r's namespace this deployment writes its runs, skills and memories in.
    pub memory_scope: Option<u64>,
    /// OPERATION language → the mount type that serves it (prism's manifest).
    pub mounts: &'a [(String, String)],
    /// The residency key a mount call runs under (ADR-0001 §6: the caller
    /// chooses it; gravity uses the conversation, so a script's state survives
    /// the turns of one conversation and nothing else).
    pub session: &'a str,
    /// Which open tasks each code row finishes (`task --finishes_on--> code`),
    /// built once from the turn's task listing. The call site reads it to close
    /// work (ADR-0003 §3); an empty table means nothing is bound, which is the
    /// ordinary case.
    pub ends_on: &'a std::collections::BTreeMap<u64, Vec<u64>>,
}

impl World<'_> {
    fn scope(&self, tool: &str) -> Result<u64, String> {
        self.memory_scope.ok_or_else(|| {
            format!("{tool}: `memory_scope` is not set on this deployment (the turn args carry it)")
        })
    }

    fn store(&self) -> &str {
        self.store_url.trim_end_matches('/')
    }

    fn mount_for(&self, language: &str) -> Option<&str> {
        self.mounts
            .iter()
            .find(|(served, _)| served == language)
            .map(|(_, type_name)| type_name.as_str())
    }
}

/// Execute one call against its door.
///
/// Every failure is a VALUE the model reads (`{"error": …}`), never a turn
/// failure: a wrong argument or a missing deployment fact must not cost the
/// user their answer.
pub fn call(
    doors: &mut impl Doors,
    name: &str,
    args: &Value,
    world: &World<'_>,
    walked: &mut Vec<Walked>,
) -> Result<Value, String> {
    let store_url = world.store();
    match name {
        "recall_memory" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("recall_memory: `query` is required")?;
            let scope = world.scope("recall_memory")?;
            doors.post(
                &format!("{store_url}/memories/{scope}/recall"),
                &json!([query]),
            )
        }
        "commit_memory" => {
            let facts_arg = args
                .get("facts")
                .ok_or("commit_memory: `facts` is required")?;
            if !facts_arg.is_array() {
                return Err("commit_memory: `facts` must be an array of 8-slot tuples".into());
            }
            let scope = world.scope("commit_memory")?;
            doors.post(&format!("{store_url}/memories/{scope}"), facts_arg)
        }
        "resolve_skills" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or("resolve_skills: `query` is required")?;
            // A skill is a node in the same graph as the facts (k10r ADR-0015),
            // so the scope that addresses the graph addresses its skills too.
            let scope = world.scope("resolve_skills")?;
            let mut body = json!({ "query": query });
            if let Some(limit) = args.get("limit") {
                body["limit"] = limit.clone();
            }
            doors.post(&format!("{store_url}/skills/{scope}/resolve"), &body)
        }
        "open_task" => {
            let name = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or("open_task: `name` is required")?;
            let scope = world.scope("open_task")?;
            let mut body = json!({ "name": name });
            if let Some(next) = args.get("next") {
                body["next"] = next.clone();
            }
            if let Some(state) = args.get("state") {
                body["state"] = state.clone();
            }
            doors.post(&format!("{store_url}/tasks/{scope}"), &body)
        }
        "move_task" => {
            let task_id = args
                .get("task_id")
                .and_then(Value::as_u64)
                .ok_or("move_task: `task_id` is required")?;
            let mut body = json!({});
            if let Some(state) = args.get("state") {
                body["state"] = state.clone();
            }
            if let Some(next) = args.get("next") {
                body["next"] = next.clone();
            }
            if body.as_object().is_some_and(|map| map.is_empty()) {
                return Err("move_task: send `state`, `next`, or both".into());
            }
            let scope = world.scope("move_task")?;
            doors.post(
                &format!("{store_url}/tasks/{scope}/{task_id}/move"),
                &body,
            )
        }
        "list_tasks" => {
            let scope = world.scope("list_tasks")?;
            let url = match args.get("state").and_then(Value::as_str) {
                Some(state) => format!("{store_url}/tasks/{scope}?state={state}"),
                None => format!("{store_url}/tasks/{scope}"),
            };
            doors.get(&url)
        }
        "create_skill" => create_skill(doors, args, world, store_url),
        "read_code" => read_code(doors, args, world, store_url),
        "write_code" => write_code(doors, args, world, store_url),
        "start" => start(doors, args, world, store_url, walked),
        "advance" => advance_run(doors, args, world, store_url, walked),
        "run" => run_code(doors, args, world, store_url, walked),
        other => Err(format!(
            "unknown tool `{other}`; this deployment serves: {}",
            names().join(", ")
        )),
    }
}

/// `create_skill`: the procedure the model decided is worth keeping (k10r
/// ADR-0015's declaration door, ADR-0004's fifth tool).
///
/// The step-shaped parts arrive as ARRAYS because that is what a model writes
/// naturally; k10r takes them as JSON TEXT and refuses to parse them, so the
/// conversion happens here — at the boundary that owns the wire, not by teaching
/// the store the model's habits (ADR-0012 §2: the producer owns the shape).
fn create_skill(
    doors: &mut impl Doors,
    args: &Value,
    world: &World<'_>,
    store_url: &str,
) -> Result<Value, String> {
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or("create_skill: `name` is required (it is the identity)")?;
    let content = args
        .get("content")
        .and_then(Value::as_str)
        .ok_or("create_skill: `content` is required (what the skill does)")?;
    let scope = world.scope("create_skill")?;
    // Absent parts stay absent: the store merges, so sending "" would erase a
    // part a previous declaration filled.
    let mut body = json!({ "name": name, "content": content });
    if let Some(title) = args.get("title").and_then(Value::as_str) {
        body["title"] = json!(title);
    }
    for field in ["preconditions", "verification"] {
        if let Some(value) = args.get(field) {
            body[field] = json!(value.to_string());
        }
    }
    // The plan is checked HERE, at the declaration: a skill whose steps cannot
    // run is refused while the model still has the shape in hand, rather than
    // discovered at `start` by whoever tried to use it later. The old ordered
    // list is refused by name — an array says "next", a plan says "when the
    // inputs resolve", and only the second can be a run.
    if let Some(value) = args.get("steps") {
        let text = match value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        };
        let plan = schedule_plan(&text).map_err(|e| e.replace("start:", "create_skill:"))?;
        let problems = schematic::validate(&plan);
        if !problems.is_empty() {
            return Ok(json!({
                "ok": false,
                "problems": problems.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
            }));
        }
        body["steps"] = json!(text);
    }
    doors.post(&format!("{store_url}/skills/{scope}"), &body)
}

/// `read_code`: the script a step names, as it stands — the current version by
/// default, one exact version when `rev` says so.
///
/// Both addresses are k10r's (ADR-0023): `/code/{scope}/{code_id}` answers the
/// current version, `/code/{scope}/{code_id}/{rev}` one exact version. The turn
/// reads this way before revising, and a person reads it way to see what an older
/// run was actually given.
fn read_code(
    doors: &mut impl Doors,
    args: &Value,
    world: &World<'_>,
    store_url: &str,
) -> Result<Value, String> {
    let code_id = args
        .get("code_id")
        .and_then(Value::as_u64)
        .ok_or("read_code: `code_id` is required (the number a skill's step names)")?;
    let scope = world.scope("read_code")?;
    match args.get("rev").and_then(Value::as_u64) {
        Some(rev) => doors.get(&format!("{store_url}/code/{scope}/{code_id}/{rev}")),
        None => doors.get(&format!("{store_url}/code/{scope}/{code_id}")),
    }
}

/// `write_code`: a script becomes a code row, and a correction becomes the next
/// version of the SAME row (k10r ADR-0023's chain).
///
/// This is the writing half of execution. Without it `invoke_skill` can only run
/// scripts somebody delivered out of band, so a skill's steps are prose the
/// deployment cannot carry out — the model can declare a procedure and never run
/// it. Two manners are the store's and are not restated here: a `code_id` it
/// never wrote is refused by id, and a revision never overwrites the version it
/// supersedes.
///
/// `parameters` arrives as an OBJECT when a model writes it naturally; the door
/// carries a JSON document in TEXT (the store parses none of it), so the
/// conversion happens at this boundary — the producer owns the shape (ADR-0012
/// §2), exactly as `create_skill`'s step-shaped parts do.
fn write_code(
    doors: &mut impl Doors,
    args: &Value,
    world: &World<'_>,
    store_url: &str,
) -> Result<Value, String> {
    let language = args
        .get("language")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .ok_or("write_code: `language` is required (the mount that runs it is chosen by it)")?;
    let source = args
        .get("source")
        .and_then(Value::as_str)
        .ok_or("write_code: `source` is required (the script body)")?;
    let scope = world.scope("write_code")?;
    let mut body = json!({ "language": language, "source": source });
    if let Some(description) = args.get("description").and_then(Value::as_str) {
        body["description"] = json!(description);
    }
    match args.get("parameters") {
        Some(Value::String(text)) => body["parameters"] = json!(text),
        Some(doc @ (Value::Object(_) | Value::Array(_))) => body["parameters"] = json!(doc.to_string()),
        _ => {}
    }
    if let Some(code_id) = args.get("code_id").and_then(Value::as_u64) {
        body["code_id"] = json!(code_id);
    }
    doors.post(&format!("{store_url}/code/{scope}"), &body)
}

/// Count one record of an invocation, on the row that owns that count. Silent on
/// failure: the store answers a receipt, and the receipt is not what the turn is
/// for (ADR-0018).
///
/// `area` is the door's row kind — `skills` for the procedure the model invoked,
/// `code` for the script that ran. BOTH are counted, because neither count
/// derives the other: a `generate-pdf` script can be a step in several skills and
/// reachable from more than one tool, so its count belongs to the script, while
/// "this procedure was tried N times" belongs to the skill.
fn touch(
    doors: &mut impl Doors,
    store_url: &str,
    area: &str,
    scope: u64,
    id: u64,
    event: &str,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let _ = doors.post(
        &format!("{store_url}/{area}/{scope}/{id}/touch"),
        &json!({ "event": event, "ts_ms": now }),
    );
}

/// `invoke_skill`: one step, resolved to a script, run on the mount that
/// serves its language.
///
/// The walk is deliberately ONE step (ADR-0004 §3, Phase 4.6): ordering,
/// retry and feeding one step's result into the next are Phase 5's, and a
/// second meaning of "invoke" inside the same tool would make the model's
/// control over the sequence invisible. A multi-step skill is a loop the MODEL
/// drives — it saw the steps in the resolve answer.
/// The subjects this deployment can DELIVER, and which k10r door each belongs to.
///
/// A `send` step names a subject; a deployment that cannot deliver it must leave
/// the step ready rather than drop the message — a send that silently vanished is
/// the one failure mode a message cannot report. `memory/fact` is the ingest door
/// (k10r's `POST /memories/{scope}`), which is the only subject the memory core
/// itself can receive.
const SUBJECTS: &[(&str, &str)] = &[("memory/fact", "memories")];

/// The last run id this process issued, so two `start` calls in the same
/// millisecond cannot land on one address (the second would REPLACE the first:
/// k10r's run door takes the id it is handed, and the revision would show a single
/// run where two exist).
static LAST_RUN_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A run id: the clock in milliseconds, pushed forward if the clock has not moved
/// since the last one this process issued.
///
/// A clock alone is not enough — `SystemTime` has millisecond resolution and a plan
/// that validates in microseconds starts faster than that — and a bare counter is
/// not enough either, because it reads as a number with no meaning. This keeps the
/// clock's meaning and adds the one guarantee that matters: strictly increasing.
fn next_run_id() -> u64 {
    use std::sync::atomic::Ordering;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut last = LAST_RUN_ID.load(Ordering::Relaxed);
    loop {
        let next = now.max(last.saturating_add(1));
        match LAST_RUN_ID.compare_exchange_weak(last, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => last = current,
        }
    }
}

/// How many drive rounds one `start` may take. A conforming plan converges in a
/// handful; the bound exists so a defect spins in the caller's hands, not here.
const MAX_ROUNDS: usize = 32;

/// Drive one run: execute every activation the ENGINE can execute, until there
/// are none left.
fn drive(
    doors: &mut impl Doors,
    world: &World<'_>,
    store_url: &str,
    plan: &schematic::Plan,
    run: &mut schematic::Run,
    walked: &mut Vec<Walked>,
) -> Result<Vec<Value>, String> {
    let mut trace: Vec<Value> = Vec::new();
    for _round in 0..MAX_ROUNDS {
        let readiness = schematic::ready(plan, run);
        let mut progress = false;
        for activation in readiness.ready {
            let Some(step) = plan.step(&activation.step) else {
                continue;
            };
            let output = match &step.kind {
                // A call is the engine's: the row runs on the mount that serves
                // its language, with the step's inputs RESOLVED (the script sees
                // values, not the plan's paths).
                schematic::Kind::Call { code: Some(code_id), .. } => {
                    let inputs = json!(activation.inputs);
                    match run_one(doors, world, store_url, *code_id, &inputs, "start", walked) {
                        Ok((value, answer)) => {
                            trace.push(json!({ "step": activation.step, "pass": activation.pass,
                                               "attempt": activation.attempt, "output": answer }));
                            value
                        }
                        // A failed ATTEMPT is still an attempt. It goes into the log
                        // — so the plan's budget edges can count it, and a resumed
                        // run knows which step failed and why — and the pass carries
                        // on: a failure that aborted the drive would leave no run at
                        // all, which is the one outcome that loses information.
                        //
                        // The step therefore completes with `{failed: true, reason}`,
                        // and the fold's next move is the plan's own: a fallback edge
                        // fires, and a plan that wants to retry wires a budgeted back
                        // edge as its fallback.
                        Err(reason) => {
                            trace.push(json!({ "step": activation.step, "pass": activation.pass,
                                               "attempt": activation.attempt, "failed": true,
                                               "reason": reason }));
                            json!({ "failed": true, "reason": reason })
                        }
                    }
                }
                // An end dispatches nothing: its inputs ARE the run's result.
                schematic::Kind::End {} => {
                    let value = json!(activation.inputs);
                    trace.push(json!({ "step": activation.step, "pass": activation.pass,
                                       "attempt": activation.attempt, "output": value,
                                       "terminal": true }));
                    value
                }
                // A send leaves the run: the subject picks the door, and a
                // subject nobody here serves leaves the step READY — parked, not
                // dropped, because a message that vanished cannot report itself.
                schematic::Kind::Send { subject, payload } => {
                    let Some((_, area)) = SUBJECTS
                        .iter()
                        .find(|(served, _)| served == subject)
                    else {
                        continue;
                    };
                    let body = payload.clone().unwrap_or_else(|| json!(activation.inputs));
                    let scope = world.scope("start")?;
                    let answer = doors.post(
                        &format!("{store_url}/{area}/{scope}"),
                        &body,
                    )?;
                    trace.push(json!({ "step": activation.step, "pass": activation.pass,
                                       "attempt": activation.attempt, "subject": subject,
                                       "output": answer }));
                    answer
                }
                // A call by ADDRESS and a request are not the engine's: one is a
                // route this deployment may not mount, the other waits for a
                // person. Both stay ready and the caller decides.
                _ => continue,
            };
            schematic::advance(
                plan,
                run,
                schematic::Entry {
                    step: activation.step.clone(),
                    pass: activation.pass,
                    attempt: activation.attempt,
                    output,
                },
            )
            .map_err(|reject| format!("start: {reject}"))?;
            progress = true;
        }
        if !progress {
            break;
        }
    }
    Ok(trace)
}

/// What the run is waiting for a PERSON to answer: the ready steps whose kind is
/// `request`, in the shape a renderer needs — the form as data (it already is:
/// schematic's `Form` is fields and types, not a schema language), the inputs
/// already resolved, and the `(step, pass, attempt)` that `advance` takes to
/// record the answer when it comes.
///
/// This is the whole of the engine's opinion about interaction: it says what is
/// missing and where the answer goes. Drawing it is the host's, and so is
/// deciding which of several waiting requests a person sees first.
fn waiting(readiness: &schematic::Readiness, plan: &schematic::Plan) -> Vec<Value> {
    readiness
        .ready
        .iter()
        .filter_map(|activation| {
            let step = plan.step(&activation.step)?;
            match &step.kind {
                schematic::Kind::Request { form, .. } => Some(json!({
                    "step": activation.step,
                    "pass": activation.pass,
                    "attempt": activation.attempt,
                    "form": serde_json::to_value(form).unwrap_or(Value::Null),
                    "inputs": activation.inputs,
                })),
                _ => None,
            }
        })
        .collect()
}

/// The run's result: the inputs the first `end` it reached was given, or `null`
/// while no path has closed.
fn run_result(plan: &schematic::Plan, run: &schematic::Run) -> Value {
    for entry in &run.entries {
        if plan
            .step(&entry.step)
            .is_some_and(|step| matches!(step.kind, schematic::Kind::End {}))
        {
            return entry.output.clone();
        }
    }
    Value::Null
}

/// The plan hash a run is stamped with: FNV-1a over the plan's JSON.
///
/// An IDENTITY, not a checksum to defend anything with — the run records which
/// plan it started from, so a skill edited mid-run is visible rather than
/// silently re-interpreted. Deliberately not a cryptographic hash: nothing bets
/// security on it, and a dependency for a label would be the wrong trade.
fn plan_hash(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// `start`: a skill's plan becomes a run (gravity ADR-0011 §7).
///
/// The plan is VALIDATED before a run exists, and a plan with problems is refused
/// whole — `ok: false` plus the problem list, nothing written. A run that started
/// from a plan the engine already knows is broken would be a half-task the model
/// then has to reason its way out of.
///
/// The snapshot goes to k10r's run table, which stores the engine's document and
/// never reads it: the run's shape belongs to this side.
fn start(
    doors: &mut impl Doors,
    args: &Value,
    world: &World<'_>,
    store_url: &str,
    walked: &mut Vec<Walked>,
) -> Result<Value, String> {
    let skill_id = args
        .get("skill_id")
        .and_then(Value::as_u64)
        .ok_or("start: `skill_id` is required (the `node_id` resolve_skills answered)")?;
    let scope = world.scope("start")?;
    let skill = doors.get(&format!("{store_url}/skills/{scope}/{skill_id}"))?;
    let text = skill
        .get("steps")
        .and_then(Value::as_str)
        .ok_or("start: the skill answered no `steps`")?;
    let plan = schedule_plan(text)?;

    let problems = schematic::validate(&plan);
    if !problems.is_empty() {
        return Ok(json!({
            "ok": false,
            "problems": problems.iter().map(|p| format!("{p:?}")).collect::<Vec<_>>(),
        }));
    }

    let hash = plan_hash(text);
    // The run remembers WHERE it came from, not just which plan it is: a run can
    // outlive the turn that started it by days, and whoever picks it up again is
    // not holding the skill id the model had in hand. `plan_hash` says "same
    // definition", the origin says "which one".
    let mut run = schematic::Run::new(hash.clone()).with_origin(format!("skill:{scope}/{skill_id}"));
    if let Some(params) = args.get("params").and_then(Value::as_object) {
        for (name, value) in params {
            run.params.insert(name.clone(), value.clone());
        }
    }
    // The run drives itself as far as the plan allows: every ready `call` runs on
    // its mount, every `end` closes its path, and the loop stops when nothing left
    // is the engine's to dispatch. A `request` waits for a person and a `send` for
    // a subject this deployment may not serve — both stay in the ready set, which
    // is the answer the caller gets.
    // A run IS the skill being used (k10r ADR-0018's counters are about the
    // PROCEDURE, not about which door reached it): the attempt is counted before
    // anything runs, and the success only if the drive came back with no failed
    // entry — `calls > ok` is what a reader looks for.
    touch(doors, store_url, "skills", scope, skill_id, "attempt");
    let trace = drive(doors, world, store_url, &plan, &mut run, walked)?;
    if !trace.iter().any(|entry| entry.get("failed").is_some()) {
        touch(doors, store_url, "skills", scope, skill_id, "success");
    }

    // The engine issues the run's id: k10r stores what it is handed and never
    // allocates one, because a run is the engine's object, not the corpus's.
    let run_id = next_run_id();
    let revision = doors.put(
        &format!("{store_url}/runs/{scope}/{run_id}"),
        &json!({ "doc": schematic::run_to_json(&run), "ts_ms": run_id }),
    )?;
    let readiness = schematic::ready(&plan, &run);
    Ok(json!({
        "ok": true,
        "run_id": run_id,
        "plan_hash": hash,
        "revision": revision.get("revision").cloned().unwrap_or(Value::Null),
        "ran": trace,
        "result": run_result(&plan, &run),
        "ready": readiness.ready.iter().map(|a| json!({
            "step": a.step,
            "pass": a.pass,
            "attempt": a.attempt,
            "inputs": a.inputs,
        })).collect::<Vec<_>>(),
        "blocked": readiness.blocked.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>(),
        "requests": waiting(&readiness, &plan),
        "complete": readiness.complete,
        "stalled": readiness.stalled,
    }))
}

/// The `(scope, skill)` a run's origin spells — `skill:{scope}/{id}`, the one form
/// this booth writes, and the ONLY place `advance` learns which skill a run belongs to.
fn origin_of(run: &schematic::Run) -> Option<(u64, u64)> {
    let rest = run.origin.as_deref()?.strip_prefix("skill:")?;
    let (scope, skill) = rest.split_once('/')?;
    Some((scope.parse().ok()?, skill.parse().ok()?))
}

/// `advance`: one completion into a run, then the next ready set (ADR-0011 §7).
///
/// The PLAN is re-read rather than trusted from the run: `plan_hash` is the
/// identity, and a skill edited mid-run is refused instead of being re-interpreted
/// under the old run's entries — the definition changed, so the run is over as
/// an instance of it.
fn advance_run(
    doors: &mut impl Doors,
    args: &Value,
    world: &World<'_>,
    store_url: &str,
    walked: &mut Vec<Walked>,
) -> Result<Value, String> {
    let run_id = args
        .get("run_id")
        .and_then(Value::as_u64)
        .ok_or("advance: `run_id` is required (the id `start` answered)")?;
    let step = args
        .get("step")
        .and_then(Value::as_str)
        .ok_or("advance: `step` is required (the step id that finished)")?
        .to_string();
    // Passes and attempts are ZERO-based in the fold (`ready` starts a step at
    // pass 0, attempt 0), so the default is the first pass's first attempt.
    let pass = args.get("pass").and_then(Value::as_u64).unwrap_or(0) as u32;
    let attempt = args.get("attempt").and_then(Value::as_u64).unwrap_or(0) as u32;
    let output = args.get("output").cloned().unwrap_or(Value::Null);

    // WHICH skill: the RUN says, and only the run. It carries the address it started
    // from, so an advance is possible from anywhere — another device, another day, a
    // page that never saw the starting call. An argument naming the skill would be a
    // second copy of the same fact, and two copies disagree eventually.
    let (scope, skill_id) = {
        let read_scope = world.scope("advance")?;
        let stored = doors.get(&format!("{store_url}/runs/{read_scope}/{run_id}"))?;
        let doc = stored
            .get("doc")
            .and_then(Value::as_str)
            .ok_or("advance: the run answer carried no document")?;
        let run = schematic::run_from_json(doc)
            .map_err(|e| format!("advance: the stored run is not a run: {e}"))?;
        origin_of(&run).ok_or(
            "advance: this run names no origin — a run started before origins existed cannot \
             be advanced, because nothing says which skill it belongs to",
        )?
    };

    let skill = doors.get(&format!("{store_url}/skills/{scope}/{skill_id}"))?;
    let text = skill
        .get("steps")
        .and_then(Value::as_str)
        .ok_or("advance: the skill answered no `steps`")?;
    let plan = schedule_plan(text)?;
    let hash = plan_hash(text);

    let url = format!("{store_url}/runs/{scope}/{run_id}");
    // READ, APPEND, WRITE BACK — conditional on the revision that was read. A run
    // is advanced from more than one place (two devices of one account, a turn and
    // a page), and the log is the whole record: two writers each reading revision
    // N would erase one completion apiece. The store refuses the stale write; the
    // entry is idempotent, so re-reading and applying it again is correct — it
    // either lands once or is already there.
    let mut settled: Option<(schematic::Run, bool, Vec<Value>)> = None;
    let mut last_error = String::new();
    for _attempt in 0..3 {
        let stored = doors.get(&url)?;
        let doc = stored
            .get("doc")
            .and_then(Value::as_str)
            .ok_or("advance: the run answer carried no document")?;
        let mut run = schematic::run_from_json(doc)
            .map_err(|e| format!("advance: the stored run is not a run: {e}"))?;
        if run.plan_hash != hash {
            return Ok(json!({
                "ok": false,
                "stale": true,
                "why": format!(
                    "the run started from plan {} and the skill is now {hash}: the definition \
                     moved, so this run cannot be advanced under it",
                    run.plan_hash
                ),
            }));
        }
        let written = match schematic::advance(
            &plan,
            &mut run,
            schematic::Entry {
                step: step.clone(),
                pass,
                attempt,
                output: output.clone(),
            },
        ) {
            Ok(schematic::Advanced::Appended) => true,
            Ok(schematic::Advanced::Duplicate) => false,
            Err(reject) => return Err(format!("advance: {reject}")),
        };
        let rev = stored.get("revision").and_then(Value::as_u64).unwrap_or(0) as u32;
        // WHAT CAN RUN, RUNS — the completion that just landed may have unblocked
        // steps the engine can do itself, and a caller should not have to loop over
        // `advance` to make that happen. Both go in ONE write: the answer and its
        // consequences are one revision of the log, so a reader never sees an answer
        // that has not been acted on.
        let trace;
        trace = drive(doors, world, store_url, &plan, &mut run, walked)?;
        let body = json!({ "doc": schematic::run_to_json(&run), "rev": rev });
        match doors.put(&url, &body) {
            Ok(_) => {
                settled = Some((run, written, trace));
                break;
            }
            // Someone wrote between this read and this write. One more round: the
            // re-read resumes from the log, so a step already recorded is not run
            // twice, and the entry that did not land is applied again.
            Err(error) => last_error = error,
        }
    }
    let Some((run, written, trace)) = settled else {
        return Err(format!(
            "advance: the run stayed busy across three reads — somebody else is driving it ({last_error})"
        ));
    };
    let readiness = schematic::ready(&plan, &run);
    Ok(json!({
        "ok": true,
        "written": written,
        // What this call's aftermath ran (a completion can unblock steps the engine
        // does itself), each with its own step address and answer.
        "ran": trace,
        "ready": readiness.ready.iter().map(|a| json!({
            "step": a.step,
            "pass": a.pass,
            "attempt": a.attempt,
            "inputs": a.inputs,
        })).collect::<Vec<_>>(),
        "blocked": readiness.blocked.iter().map(|b| format!("{b:?}")).collect::<Vec<_>>(),
        "requests": waiting(&readiness, &plan),
        "complete": readiness.complete,
        "stalled": readiness.stalled,
    }))
}

/// A skill's `steps` as a plan. A skill written before plans existed carries an
/// ORDERED ARRAY of steps, and saying so is better than guessing at one: the two
/// shapes mean different things (an array says "next", a plan says "when the
/// inputs resolve"), so the refusal names the migration instead of performing it.
fn schedule_plan(text: &str) -> Result<schematic::Plan, String> {
    if text.trim_start().starts_with('[') {
        return Err(
            "start: this skill's `steps` is the old ordered list, not a plan. Re-declare it as \
             a graph (schematic's plan form: steps + control edges) — an array says \"next\" and \
             a plan says \"when its inputs resolve\", and only the second can be a run."
                .to_string(),
        );
    }
    schematic::plan_from_json(text).map_err(|e| format!("start: the skill's `steps` is not a plan: {e}"))
}

/// What a code row needs before a node is asked to run it: the mount that serves
/// its language, its source, and its arguments checked against the row's own
/// declaration.
///
/// Split out because BOTH doors that run code need exactly this — `invoke_skill`
/// for a skill's step, `run` for a row no skill names — and two doors that
/// disagree about what "ready to run" means is how a script runs with arguments
/// nobody checked, or a language no node serves.
fn prepare(
    world: &World<'_>,
    code: &Value,
    arguments: &Value,
    who: &str,
    code_id: u64,
) -> Result<(String, String), String> {
    let language = code
        .get("language")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{who}: code row {code_id} answered no `language`"))?;
    let source = code
        .get("source")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{who}: code row {code_id} answered no `source`"))?;

    // Language → mount is a MATCH against the deployment's declaration, not a
    // default: a language no mount serves cannot be called (ADR-0001 §7), and
    // the error names what this deployment does declare.
    let mount = world.mount_for(language).ok_or_else(|| {
        let served: Vec<&str> = world.mounts.iter().map(|(l, _)| l.as_str()).collect();
        format!(
            "{who}: no mount serves `{language}` here (this deployment declares: {})",
            if served.is_empty() {
                "no mounts".to_string()
            } else {
                served.join(", ")
            }
        )
    })?;

    // The code row states what the script takes. Checking here — before a node
    // is asked to run anything — is the difference between a model that learns
    // the contract from one error and a node that fails on a missing key three
    // layers down.
    let parameters = code
        .get("parameters")
        .and_then(Value::as_str)
        .unwrap_or_default();
    check_arguments(parameters, arguments).map_err(|e| format!("{who}: {e}"))?;

    Ok((mount.to_string(), source.to_string()))
}

/// `run`: a code row, on this deployment's node, with no skill in the way.
///
/// A row runs by its id: the row states its language, the language picks the
/// mount, the script rides the call as an argument, and the row's own
/// `parameters` are enforced before a node is asked to run anything.
///
/// The counters differ on purpose: there is no skill row to count, so only the
/// code row records the attempt. Closing bound tasks and the walk stay the same —
/// neither is about which door was used.
/// Run ONE code row on the mount that serves its language — the shared half of
/// `run` and of every `call` step a run drives.
///
/// Returns the script's own answer (a plan step's output) and the answer the
/// MODEL reads (the same, wrapped with what the call closed). Counting, walking
/// and closing are the same work whichever door reached the row.
fn run_one(
    doors: &mut impl Doors,
    world: &World<'_>,
    store_url: &str,
    code_id: u64,
    arguments: &Value,
    who: &str,
    walked: &mut Vec<Walked>,
) -> Result<(Value, Value), String> {
    // The scope is the deployment's, not the caller's: `Facts::scope` answers the
    // same number whoever asks, and only names the tool in its refusal.
    let scope = world.scope(who)?;
    let code = doors.get(&format!("{store_url}/code/{scope}/{code_id}"))?;
    let (mount, source) = prepare(world, &code, arguments, who, code_id)?;

    // A call that never comes back must still be visible (k10r ADR-0018): the
    // attempt is counted before the node is asked to run anything, and the
    // success is the second record — so `calls > ok` is what a reader looks for.
    touch(doors, store_url, "code", scope, code_id, "attempt");
    let value = doors.mount(
        &mount,
        world.session,
        json!({ "code": source, "args": arguments }),
    );
    if value.is_ok() {
        touch(doors, store_url, "code", scope, code_id, "success");
    }
    let value = value?;

    // Recorded like any other step: "this turn ran this row" is the same fact
    // whichever door reached it, and the composition rule filters singletons.
    walked.push(Walked {
        code_id,
        description: code
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    });
    let answer = close_bound(doors, world, store_url, scope, code_id, value.clone());
    Ok((value, answer))
}

fn run_code(
    doors: &mut impl Doors,
    args: &Value,
    world: &World<'_>,
    store_url: &str,
    walked: &mut Vec<Walked>,
) -> Result<Value, String> {
    let code_id = args
        .get("code_id")
        .and_then(Value::as_u64)
        .ok_or("run: `code_id` is required (the row to run — `read_code` names it)")?;
    let arguments = args.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let (_, answer) = run_one(doors, world, store_url, code_id, &arguments, "run", walked)?;
    Ok(answer)
}

/// Close every open task bound to `code_id`, and say so in the value the model
/// reads. Best effort in the same sense as a counter: the step already ran, and
/// a store that cannot take the move must not turn a success into a failure —
/// it must be VISIBLE instead, so the answer carries the error rather than
/// swallowing it.
fn close_bound(
    doors: &mut impl Doors,
    world: &World<'_>,
    store_url: &str,
    scope: u64,
    code_id: u64,
    value: Value,
) -> Value {
    let Some(task_ids) = world.ends_on.get(&code_id) else {
        return value;
    };
    if task_ids.is_empty() {
        return value;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default();
    let mut closed: Vec<u64> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    for task_id in task_ids {
        let moved = doors.post(
            &format!("{store_url}/tasks/{scope}/{task_id}/move"),
            &json!({ "state": "done", "ts_ms": now }),
        );
        match moved {
            Ok(_) => closed.push(*task_id),
            Err(error) => errors.push(format!("task {task_id}: {error}")),
        }
    }
    // The step's own document is NEVER edited: it is wrapped, so a caller (or a
    // model reading the transcript) finds the script's answer whole under
    // `result`, and what the call closed beside it. Merging keys into the script's
    // object would be this booth rewriting somebody else's answer.
    let mut out = json!({ "result": value });
    if let Value::Object(map) = &mut out {
        if !closed.is_empty() {
            map.insert("finished_tasks".into(), json!(closed));
        }
        if !errors.is_empty() {
            map.insert("finish_errors".into(), json!(errors));
        }
    }
    out
}

/// One code row this turn walked, in order. The description rides along because
/// the proposal a human reviews shows what each step does, and the walk is the
/// only moment all of them are in hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Walked {
    pub code_id: u64,
    pub description: String,
}

/// The composition as the store spells it (ADR-0016: the walker names it, since
/// Whether this call should FILE the walk it just extended (k10r ADR-0016 as
/// re-cut): the walk must have GROWN — a retried step leaves the recorder as it
/// was, and filing it again would count one walk twice — and what it grew to
/// must be a composition, two different code rows.
pub fn should_file(before: usize, after: usize, walked: &[Walked]) -> bool {
    after > before && is_composition(walked)
}

/// k10r does not parse `steps`).
pub fn composition_key(walked: &[Walked]) -> String {
    walked
        .iter()
        .map(|step| step.code_id.to_string())
        .collect::<Vec<_>>()
        .join("+")
}

/// The steps document a proposal carries — the same shape a skill's `steps`
/// takes, so promotion copies it into a skill unchanged (k10r ADR-0016).
pub fn steps_json(walked: &[Walked]) -> String {
    Value::Array(
        walked
            .iter()
            .map(|step| json!({ "code_id": step.code_id, "note": step.description }))
            .collect(),
    )
    .to_string()
}

/// Is this walk worth reporting? Two DIFFERENT rows ran in one turn — a single
/// row twice is a retry, not a composition.
pub fn is_composition(walked: &[Walked]) -> bool {
    let mut ids: Vec<u64> = walked.iter().map(|step| step.code_id).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.len() >= 2
}

/// Does `arguments` honour the contract a code row declares in `parameters`?
///
/// The field is TEXT the store never parses (k10r ADR-0012 §2: the producer owns
/// the shape), so the only shape this can check is one the producer DECLARED. A
/// document that declares nothing — `{}`, an empty string, or prose somebody wrote
/// for a human — constrains nothing, and passing it means exactly that: nothing
/// was checked, because nothing was claimed. Inventing a rejection for it would
/// break rows whose author chose description over schema, and the model could do
/// nothing about that either.
///
/// The declared form is a JSON Schema SUBSET, because that is what people already
/// write and a custom format would be one more thing to learn:
///   { "type": "object",
///     "required": ["repo"],
///     "properties": { "repo": {"type": "string"}, "n": {"type": "integer"} },
///     "additionalProperties": false }
fn check_arguments(declared: &str, given: &Value) -> Result<(), String> {
    let text = declared.trim();
    if text.is_empty() {
        return Ok(());
    }
    let Ok(schema) = serde_json::from_str::<Value>(text) else {
        // Prose, not a contract. See the doc comment: silence is the honest
        // answer, and the review belongs on the producer's side.
        return Ok(());
    };
    let Some(object) = schema.as_object() else {
        return Ok(());
    };
    let required = object.get("required").and_then(Value::as_array);
    let properties = object.get("properties").and_then(Value::as_object);
    if required.is_none() && properties.is_none() {
        return Ok(());
    }

    let Some(given) = given.as_object() else {
        return Err(format!(
            "the code row declares parameters, so `arguments` must be an object; the contract is: {text}"
        ));
    };

    if let Some(required) = required {
        let missing: Vec<&str> = required
            .iter()
            .filter_map(Value::as_str)
            .filter(|name| !given.contains_key(*name))
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "`arguments` is missing required {}: {}; the contract is: {text}",
                if missing.len() == 1 { "key" } else { "keys" },
                missing.join(", ")
            ));
        }
    }

    if let Some(properties) = properties {
        for (name, rule) in properties {
            let Some(value) = given.get(name) else {
                continue;
            };
            let wanted = rule.get("type").and_then(type_names).unwrap_or_default();
            if wanted.is_empty() || wanted.iter().any(|kind| is_kind(value, kind)) {
                continue;
            }
            return Err(format!(
                "`arguments.{name}` is {}, and the code row wants {}; the contract is: {text}",
                kind_of(value),
                wanted.join(" or ")
            ));
        }
    }

    if object.get("additionalProperties") == Some(&Value::Bool(false)) {
        let unknown: Vec<&str> = given
            .keys()
            .filter(|name| {
                properties
                    .map(|declared| !declared.contains_key(*name))
                    .unwrap_or(true)
            })
            .map(String::as_str)
            .collect();
        if !unknown.is_empty() {
            return Err(format!(
                "the code row takes no `{}`; the contract is: {text}",
                unknown.join("`, no `")
            ));
        }
    }

    Ok(())
}

/// `"type": "string"` and `"type": ["string", "null"]` are both idiomatic.
fn type_names(value: &Value) -> Option<Vec<&str>> {
    if let Some(name) = value.as_str() {
        return Some(vec![name]);
    }
    value
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

fn is_kind(value: &Value, kind: &str) -> bool {
    match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        // JSON has no integer, so this is a number that survived the trip
        // whole; a float that arrived as 4.0 is still the integer 4.
        "integer" => value
            .as_f64()
            .is_some_and(|n| n.is_finite() && n.fract() == 0.0),
        "array" => value.is_array(),
        "object" => value.is_object(),
        // An unknown keyword is not a constraint we can honour; it must not
        // become one that rejects valid arguments.
        _ => true,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// Every door, recorded: what the call asked of each, and canned answers
    /// keyed by a substring of the URL. Enough to pin WHERE a call goes and
    /// WHAT it carries — the doors' own behaviour is k10r's and the realm's.
    #[derive(Default)]
    struct Recorder {
        posts: Vec<(String, Value)>,
        puts: Vec<(String, Value)>,
        gets: Vec<String>,
        mounts: Vec<(String, String, Value)>,
        answers: Vec<(String, Value)>,
        mount_error: Option<String>,
        /// How many writes to refuse before one lands — the race a run's
        /// conditional write is built to survive.
        put_conflicts: usize,
    }

    impl Recorder {
        fn answering(mut self, needle: &str, answer: Value) -> Self {
            self.answers.push((needle.to_string(), answer));
            self
        }

        /// A node that refuses the run — the failure class a task binding must
        /// not paper over.
        fn losing_writes(mut self, conflicts: usize) -> Self {
            self.put_conflicts = conflicts;
            self
        }

        fn refusing_the_run(mut self, reason: &str) -> Self {
            self.mount_error = Some(reason.to_string());
            self
        }
    }

    impl Doors for Recorder {
        fn post(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            self.posts.push((url.to_string(), body.clone()));
            Ok(json!({ "posted": true }))
        }

        fn put(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            self.puts.push((url.to_string(), body.clone()));
            if self.put_conflicts > 0 {
                self.put_conflicts -= 1;
                return Err(format!("{url}: HTTP 409: revision conflict"));
            }
            Ok(json!({ "revision": 1 }))
        }

        fn get(&mut self, url: &str) -> Result<Value, String> {
            self.gets.push(url.to_string());
            self.answers
                .iter()
                .find(|(needle, _)| url.contains(needle.as_str()))
                .map(|(_, answer)| answer.clone())
                .ok_or_else(|| format!("{url}: no canned answer"))
        }

        fn mount(&mut self, type_name: &str, key: &str, args: Value) -> Result<Value, String> {
            self.mounts
                .push((type_name.to_string(), key.to_string(), args));
            match &self.mount_error {
                Some(reason) => Err(reason.clone()),
                None => Ok(json!({ "ran": true })),
            }
        }
    }

    /// The ordinary case: nothing bound, so nothing closes.
    static NOTHING_BOUND: std::collections::BTreeMap<u64, Vec<u64>> =
        std::collections::BTreeMap::new();

    /// A node that does not answer: the mount fails, every other door passes
    /// through to the recorder underneath.
    struct Broken(Recorder);

    impl Doors for Broken {
        fn post(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            self.0.post(url, body)
        }
        fn put(&mut self, url: &str, body: &Value) -> Result<Value, String> {
            self.0.put(url, body)
        }
        fn get(&mut self, url: &str) -> Result<Value, String> {
            self.0.get(url)
        }
        fn mount(&mut self, _type_name: &str, _key: &str, _args: Value) -> Result<Value, String> {
            Err("the node did not answer".into())
        }
    }

    fn world<'a>(mounts: &'a [(String, String)], session: &'a str) -> World<'a> {
        World {
            store_url: "http://mem.test/",
            memory_scope: Some(37),
            mounts,
            session,
            ends_on: &NOTHING_BOUND,
        }
    }

    fn world_bound<'a>(
        mounts: &'a [(String, String)],
        session: &'a str,
        ends_on: &'a std::collections::BTreeMap<u64, Vec<u64>>,
    ) -> World<'a> {
        World {
            store_url: "http://mem.test/",
            memory_scope: Some(37),
            mounts,
            session,
            ends_on,
        }
    }

    fn python_mount() -> Vec<(String, String)> {
        vec![("python".to_string(), "runner_py".to_string())]
    }

    #[test]
    fn every_entry_is_an_openai_function_shape() {
        let array = tools_array();
        let entries = array.as_array().expect("an array");
        assert_eq!(entries.len(), specs().len());
        for (entry, spec) in entries.iter().zip(specs()) {
            assert_eq!(entry["type"], "function");
            assert_eq!(entry["function"]["name"], spec.name);
            assert!(
                entry["function"]["description"].as_str().unwrap_or("").len() > 40,
                "a tool without a usable description is a tool the model cannot choose"
            );
            assert_eq!(entry["function"]["parameters"]["type"], "object");
            assert!(entry["function"]["parameters"]["required"].is_array());
        }
    }

    #[test]
    fn an_unlisted_tool_is_refused_by_name() {
        let mounts = python_mount();
        let mut doors = Recorder::default();
        let Err(error) = call(
            &mut doors,
            "task_open",
            &json!({}),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        ) else {
            panic!("an unlisted tool must be refused");
        };
        assert!(error.contains("unknown tool `task_open`"), "{error}");
        assert!(error.contains("recall_memory"), "{error}");
    }


    /// The plan k10r hands back as the skill's `steps`: two steps, one control
    /// edge, an exit.
    const PLAN: &str = r#"{
      "schema_version": 1,
      "steps": [
        { "id": "fetch", "kind": "call", "code": 7,
          "inputs": { "url": "https://example.invalid/a" },
          "out_edges": [{ "to": "done" }] },
        { "id": "done", "kind": "end", "inputs": { "body": "$.fetch.body" } }
      ]
    }"#;

    /// `start` writes ONE snapshot and answers the ready set: the plan's entry
    /// step may run, the exit may not.
    #[test]
    fn start_writes_the_snapshot_and_answers_the_ready_set() {
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": PLAN }))
            .answering(
                "/code/37/7",
                json!({ "language": "python", "source": "fetch()", "description": "fetch it" }),
            );
        let value = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9, "params": { "who": "me" } }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");

        assert_eq!(value["ok"], true, "{value}");
        // The engine drove the entry step: the row ran on the mount that serves
        // its language, and the completion is in the run's log.
        let ran = value["ran"].as_array().expect("a trace");
        assert_eq!(ran.len(), 1, "one step was the engine's to run: {value}");
        assert_eq!(ran[0]["step"], "fetch");
        // The exit reads `$.fetch.body` and the script answered `{"ran": true}`:
        // the step is BLOCKED rather than skipped, which is the whole point of
        // resolving inputs before dispatching anything.
        assert_eq!(value["complete"], false);
        assert!(
            !value["blocked"].as_array().unwrap().is_empty(),
            "an input that does not resolve is reported: {value}"
        );
        assert!(value["ready"].as_array().unwrap().is_empty());

        // One PUT, to the run's own address, carrying a run document whose
        // plan identity is the one that was validated.
        let (url, body) = doors
            .puts
            .first()
            .expect("the snapshot was written rather than posted");
        assert!(url.contains("/runs/37/"), "{url}");
        assert_eq!(body["doc"].as_str().unwrap().contains("\"plan_hash\""), true);
        assert_eq!(
            body["doc"].as_str().unwrap().contains(&value["plan_hash"].as_str().unwrap().to_string()),
            true,
            "the run records the plan it started from"
        );
        let doc = body["doc"].as_str().unwrap();
        assert!(
            doc.contains("\"skill:37/9\""),
            "the run is rooted where it started: {doc}"
        );
    }

    /// A plan that cannot run is refused WITH its problems, and nothing is
    /// written: a half-started run is a task the model would then have to reason
    /// its way out of.
    #[test]
    fn start_refuses_a_plan_that_cannot_run_and_writes_nothing() {
        let mounts = python_mount();
        // Two exits, one of them with an out-edge — `TerminalWithEdges`.
        let broken = r#"{
          "schema_version": 1,
          "steps": [
            { "id": "a", "kind": "call", "code": 7, "out_edges": [{ "to": "done" }] },
            { "id": "done", "kind": "end", "out_edges": [{ "to": "a" }] }
          ]
        }"#;
        let mut doors = Recorder::default().answering("/skills/37/9", json!({ "steps": broken }));
        let value = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("a refusal is an answer, not a turn failure");

        assert_eq!(value["ok"], false, "{value}");
        assert!(
            !value["problems"].as_array().unwrap().is_empty(),
            "the refusal names what is wrong"
        );
        assert!(doors.puts.is_empty(), "nothing was written");
    }


    /// `advance` appends one completion, writes the run back, and answers the
    /// next ready set — and a redelivered completion writes nothing twice.
    #[test]
    fn advance_appends_once_and_answers_the_next_ready_set() {
        let mounts = python_mount();
        let run_doc = schematic::run_to_json(&schematic::Run::new(plan_hash(PLAN)).with_origin("skill:37/9"));
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": PLAN }))
            .answering("/runs/37/", json!({ "doc": run_doc, "revision": 1 }));
        let value = call(
            &mut doors,
            "advance",
            &json!({ "run_id": 5, "step": "fetch", "output": { "body": "hi" } }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");

        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["written"], true);
        // The completion unblocked the exit, and the exit RAN: what the engine can do
        // itself happens inside the same call, so a caller does not loop over `advance`
        // until the run stops moving.
        assert_eq!(value["complete"], true, "{value}");
        let ready = value["ready"].as_array().expect("a ready set");
        assert!(ready.is_empty(), "nothing is left to run: {value}");
        assert_eq!(value["ran"][0]["step"], "done", "the exit ran: {value}");
        let (url, body) = doors.puts.first().expect("the run was written back");
        assert!(url.contains("/runs/37/5"), "{url}");
        assert_eq!(body["rev"], 1, "the write names the revision it read");
        let stored_doc = body["doc"].as_str().unwrap();
        assert!(
            stored_doc.contains("\"fetch\""),
            "the completed step is in the stored document: {stored_doc}"
        );
    }

    /// A run is an instance of ONE plan: a skill edited after the run started
    /// refuses the advance instead of re-interpreting the old entries under a
    /// new definition.
    #[test]
    fn advance_refuses_a_run_whose_plan_moved() {
        let mounts = python_mount();
        let run_doc = schematic::run_to_json(&schematic::Run::new("0000000000000000").with_origin("skill:37/9"));
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": PLAN }))
            .answering("/runs/37/", json!({ "doc": run_doc, "revision": 1 }));
        let value = call(
            &mut doors,
            "advance",
            &json!({ "run_id": 5, "step": "fetch" }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("a stale run is an answer");
        assert_eq!(value["ok"], false, "{value}");
        assert_eq!(value["stale"], true);
        assert!(doors.puts.is_empty(), "nothing was written");
    }

    /// A skill written before plans existed says so instead of being guessed at:
    /// an ordered array means "next", a plan means "when the inputs resolve".
    #[test]
    fn start_refuses_the_old_ordered_list_with_the_migration() {
        let mounts = python_mount();
        let mut doors = Recorder::default().answering(
            "/skills/37/9",
            json!({ "steps": r#"[{"code_id": 4}]"# }),
        );
        let error = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect_err("an ordered list is not a plan");
        assert!(error.contains("old ordered list"), "{error}");
        assert!(doors.puts.is_empty());
    }

    #[test]
    fn the_list_carries_the_tool_faces_names() {
        assert_eq!(
            names(),
            vec![
                "recall_memory",
                "commit_memory",
                "resolve_skills",
                "start",
                "advance",
                "run",
                "create_skill",
                "read_code",
                "write_code",
                "open_task",
                "move_task",
                "list_tasks",
            ]
        );
    }

    /// `run` (and, through `run_one`, every `call` step a run drives): the
    /// attempt is counted BEFORE the node is asked, the success after, and a
    /// failure leaves the count alone — `calls > ok` is the visible difference.
    #[test]
    fn a_row_counts_its_attempt_before_it_runs_and_its_success_after() {
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering("/code/37/4", json!({ "language": "python", "source": "print(1)" }));
        call(
            &mut doors,
            "run",
            &json!({ "code_id": 4, "arguments": {} }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the node answered");
        let events: Vec<&Value> = doors
            .posts
            .iter()
            .filter(|(url, _)| url.contains("/code/37/4/touch"))
            .map(|(_, body)| &body["event"])
            .collect();
        assert_eq!(events, vec!["attempt", "success"], "{:?}", doors.posts);

        // The same row, with a node that does not answer: the attempt is still
        // counted, and there is no success beside it.
        let mut broken = Broken(Recorder::default().answering(
            "/code/37/4",
            json!({ "language": "python", "source": "print(1)" }),
        ));
        let error = call(
            &mut broken,
            "run",
            &json!({ "code_id": 4 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect_err("the node did not answer");
        assert!(error.contains("did not answer"), "{error}");
    }

    /// A language no mount serves is refused with what this deployment declares,
    /// and the row's own `parameters` are enforced before a node is asked.
    #[test]
    fn a_row_is_checked_against_the_deployment_and_its_own_parameters() {
        let mounts = vec![("python".to_string(), "runner_py".to_string())];
        let mut doors = Recorder::default().answering(
            "/code/37/5",
            json!({ "language": "ruby", "source": "puts 1" }),
        );
        let error = call(
            &mut doors,
            "run",
            &json!({ "code_id": 5 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect_err("no mount serves ruby");
        assert!(error.contains("no mount serves `ruby`"), "{error}");
        assert!(doors.mounts.is_empty(), "nothing ran");

        let mut doors = Recorder::default().answering(
            "/code/37/6",
            json!({
                "language": "python",
                "source": "print(1)",
                "parameters": r#"{"type":"object","required":["url"]}"#
            }),
        );
        let error = call(
            &mut doors,
            "run",
            &json!({ "code_id": 6, "arguments": {} }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect_err("`url` is required by the row");
        assert!(error.contains("url"), "{error}");
        assert!(doors.mounts.is_empty(), "the node was never asked");
    }

    /// A skill is declared with a PLAN, and the declaration is where a plan is
    /// checked: a broken graph is refused while the model still has it in hand,
    /// and the old ordered list is refused by name.
    #[test]
    fn create_skill_holds_the_steps_to_the_plan_form() {
        let mounts = python_mount();
        let mut doors = Recorder::default();
        call(
            &mut doors,
            "create_skill",
            &json!({ "name": "fetch_page", "content": "get a page", "steps": PLAN }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");
        let (url, body) = &doors.posts[0];
        assert_eq!(url, "http://mem.test/skills/37");
        let stored = body["steps"].as_str().expect("steps travel as text");
        assert!(stored.contains("\"out_edges\""), "the plan was stored: {stored}");

        // An ordered list is not a plan.
        let error = call(
            &mut doors,
            "create_skill",
            &json!({ "name": "legacy", "content": "old", "steps": [{"code_id": 4}] }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect_err("an array is not a plan");
        assert!(error.contains("old ordered list"), "{error}");

        // A graph that cannot run is refused with its problems and nothing is sent.
        let before = doors.posts.len();
        let value = call(
            &mut doors,
            "create_skill",
            &json!({ "name": "broken", "content": "x", "steps": {
                "schema_version": 1,
                "steps": [
                    { "id": "a", "kind": "call", "code": 7, "out_edges": [{ "to": "z" }] }
                ]
            }}),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("a refusal is an answer");
        assert_eq!(value["ok"], false, "{value}");
        assert_eq!(doors.posts.len(), before, "nothing reached the store");
    }

    /// A `send` leaves the run through the door its subject names, and a subject
    /// no deployment here serves leaves the step READY — a message that vanished
    /// cannot report that it vanished.
    #[test]
    fn a_send_is_delivered_by_subject_or_left_ready() {
        let mounts = python_mount();
        let delivered = r#"{
          "schema_version": 1,
          "steps": [
            { "id": "tell", "kind": "send", "subject": "memory/fact",
              "payload": [["alice", "person", {}, "likes", {}, "tea", "drink", {}]],
              "out_edges": [{ "to": "done" }] },
            { "id": "done", "kind": "end" }
          ]
        }"#;
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": delivered }))
            .answering("/memories/37", json!({ "landed": 1 }));
        let value = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");

        assert_eq!(value["ok"], true, "{value}");
        let ingest = doors
            .posts
            .iter()
            .find(|(url, _)| url.ends_with("/memories/37"))
            .unwrap_or_else(|| panic!("the subject picked the ingest door: {:?}", doors.posts));
        assert_eq!(ingest.1[0][0], "alice", "the payload was the body");
        assert_eq!(value["complete"], true, "the path closed: {value}");

        // A subject with no deliverer: the step stays ready, nothing is posted.
        let undelivered = r#"{
          "schema_version": 1,
          "steps": [
            { "id": "tell", "kind": "send", "subject": "mail/out", "payload": {"x": 1},
              "out_edges": [{ "to": "done" }] },
            { "id": "done", "kind": "end" }
          ]
        }"#;
        let mut doors = Recorder::default().answering("/skills/37/9", json!({ "steps": undelivered }));
        let value = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");
        assert!(
            !doors.posts.iter().any(|(url, _)| url.contains("/memories/")),
            "nothing was delivered: {:?}",
            doors.posts
        );
        assert_eq!(value["ok"], true, "{value}");
        let ready = value["ready"].as_array().unwrap_or_else(|| panic!("{value}"));
        assert_eq!(ready.len(), 1, "the send is parked, not dropped: {value}");
        assert_eq!(ready[0]["step"], "tell");
    }

    /// A `request` is what a run waits on a person for: `start` answers it with the
    /// form as data, the inputs as values, and the address `advance` takes — the
    /// engine says what is missing and where the answer goes, and draws nothing.
    #[test]
    fn a_request_step_is_answered_as_a_form_and_an_address() {
        let mounts = python_mount();
        let plan = r#"{
          "schema_version": 1,
          "steps": [
            { "id": "ask", "kind": "request",
              "form": { "title": "Pick a branch",
                        "fields": [ { "name": "branch", "type": "choice",
                                      "options": ["retry", "fine"], "title": "Next" } ] },
              "out_edges": [{ "to": "done", "when": "retry" }, { "to": "done", "when": "*" }] },
            { "id": "done", "kind": "end" }
          ]
        }"#;
        let mut doors = Recorder::default().answering("/skills/37/9", json!({ "steps": plan }));
        let value = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");

        assert_eq!(value["ok"], true, "{value}");
        let waiting = value["requests"].as_array().expect("a waiting set");
        assert_eq!(waiting.len(), 1, "{value}");
        assert_eq!(waiting[0]["step"], "ask");
        assert_eq!(waiting[0]["pass"], 0);
        assert_eq!(waiting[0]["attempt"], 0);
        assert_eq!(waiting[0]["form"]["fields"][0]["name"], "branch");
        assert_eq!(waiting[0]["form"]["title"], "Pick a branch");
        assert_eq!(value["complete"], false, "nothing ran: a person has to answer");
    }

    /// A step that fails leaves a RECORD: the run is written with the failure in
    /// its log (so a budget edge can count it and a resumed run knows where it
    /// stopped), and the drive carries on to whatever the plan's fallback says.
    #[test]
    fn a_failed_step_is_recorded_in_the_run_instead_of_aborting_it() {
        let mounts = python_mount();
        let mut doors = Broken(Recorder::default().answering(
            "/skills/37/9",
            json!({ "steps": PLAN }),
        ).answering(
            "/code/37/7",
            json!({ "language": "python", "source": "boom()" }),
        ));
        let value = call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("a failed step is an answer, not a turn failure");

        assert_eq!(value["ok"], true, "{value}");
        let ran = value["ran"].as_array().expect("a trace");
        assert_eq!(ran.len(), 1, "{value}");
        assert_eq!(ran[0]["failed"], true, "the trace names the failure");
        assert!(
            ran[0]["reason"].as_str().unwrap().contains("did not answer"),
            "with the reason the node gave: {value}"
        );
        // Written anyway: the snapshot is the one thing a failure must not lose.
        let (url, body) = doors.0.puts.first().expect("the run was written");
        assert!(url.contains("/runs/37/"), "{url}");
        let doc = body["doc"].as_str().unwrap();
        assert!(doc.contains("\"failed\""), "the failure is in the log: {doc}");
        assert!(doc.contains("did not answer"), "and so is its reason: {doc}");
    }

    /// Two runs started in the same millisecond are two runs: the id is pushed
    /// forward rather than shared, because k10r's door takes the id it is handed
    /// and the second write would REPLACE the first.
    #[test]
    fn run_ids_are_strictly_increasing() {
        let first = next_run_id();
        let second = next_run_id();
        let third = next_run_id();
        assert!(first < second && second < third, "{first} {second} {third}");
    }

    /// Two writers, one row: a write refused because the revision moved is
    /// re-read and re-applied — the entry is idempotent, so the retry lands once
    /// and nothing is lost.
    #[test]
    fn advance_retries_a_write_the_store_refused() {
        let mounts = python_mount();
        let run_doc = schematic::run_to_json(&schematic::Run::new(plan_hash(PLAN)).with_origin("skill:37/9"));
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": PLAN }))
            .answering("/runs/37/", json!({ "doc": run_doc, "revision": 1 }))
            .losing_writes(1);
        let value = call(
            &mut doors,
            "advance",
            &json!({ "run_id": 5, "step": "fetch", "output": { "body": "hi" } }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");

        assert_eq!(value["ok"], true, "{value}");
        assert_eq!(value["written"], true, "the completion landed on the retry");
        assert_eq!(doors.puts.len(), 2, "one refused write, one taken");
    }

    /// A run says where it came from, so whoever picks it up later does not need
    /// the skill id the starting model had in hand — and a run that names no
    /// origin is refused rather than guessed at.
    #[test]
    fn advance_finds_its_skill_in_the_run_it_reads() {
        let mounts = python_mount();
        let rooted = schematic::Run::new(plan_hash(PLAN)).with_origin("skill:37/9");
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": PLAN }))
            .answering(
                "/runs/37/",
                json!({ "doc": schematic::run_to_json(&rooted), "revision": 1 }),
            );
        let value = call(
            &mut doors,
            "advance",
            &json!({ "run_id": 5, "step": "fetch", "output": { "body": "hi" } }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");
        assert_eq!(value["ok"], true, "{value}");
        assert!(
            doors.gets.iter().any(|url| url.ends_with("/skills/37/9")),
            "the origin named the skill: {:?}",
            doors.gets
        );

        // A run with no origin, and no skill in the args: an answer, not a guess.
        let bare = schematic::Run::new(plan_hash(PLAN));
        let mut doors = Recorder::default().answering(
            "/runs/37/",
            json!({ "doc": schematic::run_to_json(&bare), "revision": 1 }),
        );
        let error = call(
            &mut doors,
            "advance",
            &json!({ "run_id": 5, "step": "fetch" }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect_err("nothing names the skill");
        assert!(error.contains("names no origin"), "{error}");
    }

    /// Starting a run counts on the SKILL: the procedure was used, whichever door
    /// reached it, and a drive with a failed step is not a success.
    #[test]
    fn start_counts_the_skill_before_and_after() {
        let mounts = python_mount();
        let mut doors = Recorder::default()
            .answering("/skills/37/9", json!({ "steps": PLAN }))
            .answering(
                "/code/37/7",
                json!({ "language": "python", "source": "fetch()" }),
            );
        call(
            &mut doors,
            "start",
            &json!({ "skill_id": 9 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");
        let events: Vec<&Value> = doors
            .posts
            .iter()
            .filter(|(url, _)| url.contains("/skills/37/9/touch"))
            .map(|(_, body)| &body["event"])
            .collect();
        assert_eq!(events, vec!["attempt", "success"], "{:?}", doors.posts);
    }

    #[test]
    fn resolve_skills_posts_to_the_skill_door_with_the_scope_in_the_path() {
        let mounts = python_mount();
        let mut doors = Recorder::default();
        call(
            &mut doors,
            "resolve_skills",
            &json!({ "query": "list github issues", "limit": 3 }),
            &world(&mounts, "s-1"),
            &mut Vec::new(),
        )
        .expect("the door answered");

        let (url, body) = &doors.posts[0];
        assert_eq!(
            url, "http://mem.test/skills/37/resolve",
            "a skill is a node in the graph, so the graph's scope addresses it"
        );
        assert_eq!(body["query"], "list github issues");
        assert_eq!(body["limit"], 3);
    }

    #[test]
    fn a_missing_scope_names_the_fact_instead_of_guessing() {
        let mounts = python_mount();
        let mut doors = Recorder::default();
        let world = World {
            store_url: "http://mem.test",
            memory_scope: None,
            mounts: &mounts,
            session: "s-1",
            ends_on: &Default::default(),
        };
        let Err(error) = call(
            &mut doors,
            "recall_memory",
            &json!({ "query": "x" }),
            &world,
            &mut Vec::new(),
        ) else {
            panic!("a tool with no scope must be refused");
        };
        assert!(error.contains("memory_scope"), "{error}");
    }

    /// ADR-0003 §3: `task --finishes_on--> code`, and the CALL SITE closes it. The
    /// model dispatched the step; nothing about the close is its judgement, and
    /// nothing about it is a round trip the model could have skipped.
    #[test]
    fn a_composition_is_two_different_rows_and_the_wire_shape_is_the_walkers() {
        let one = Walked {
            code_id: 4,
            description: "fetch".to_string(),
        };
        let two = Walked {
            code_id: 7,
            description: "format".to_string(),
        };

        assert!(!is_composition(&[]), "nothing ran");
        assert!(
            !is_composition(&[one.clone(), one.clone()]),
            "one row twice is a retry, not a composition"
        );
        assert!(is_composition(&[one.clone(), two.clone()]), "two rows ran");

        assert_eq!(composition_key(&[one.clone(), two.clone()]), "4+7");
        assert_eq!(
            composition_key(&[two.clone(), one.clone()]),
            "7+4",
            "ORDER is the composition: the reverse is a different run"
        );
        assert_eq!(
            steps_json(&[one, two]),
            r#"[{"code_id":4,"note":"fetch"},{"code_id":7,"note":"format"}]"#,
            "the shape a skill's steps takes, so promotion copies it (k10r ADR-0016)"
        );
    }

    #[test]
    fn create_skill_posts_the_declaration_and_keeps_absent_parts_absent() {
        let mut doors = Recorder::default();
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        let value = call(
            &mut doors,
            "create_skill",
            &json!({
                "name": "list-github-issues",
                "title": "List issues",
                "content": "Lists the open issues of a repository.",
                "steps": {
                    "schema_version": 1,
                    "steps": [
                        { "id": "fetch", "kind": "call", "code": 4,
                          "out_edges": [{ "to": "done" }] },
                        { "id": "done", "kind": "end" }
                    ]
                },
                "preconditions": ["the repo is cloned"],
            }),
            &world,
            &mut Vec::new(),
        )
        .expect("the declaration lands");
        assert_eq!(value["posted"], true);

        let (url, body) = &doors.posts[0];
        assert_eq!(url, "http://mem.test/skills/37", "the scope is the deployment's");
        assert_eq!(body["name"], "list-github-issues");
        let stored = body["steps"].as_str().expect("the plan travels as text");
        assert!(
            stored.contains("\"out_edges\""),
            "the step graph travels as the JSON TEXT k10r stores (it parses nothing): {stored}"
        );
        assert_eq!(body["preconditions"], r#"["the repo is cloned"]"#);
        assert!(
            body.get("verification").is_none(),
            "a part the model did not state stays ABSENT — the store merges, \
             so an empty one would erase what a previous declaration filled"
        );
    }

    #[test]
    fn read_code_asks_the_current_version_unless_a_revision_is_named() {
        // The ordinary read is the version `invoke_skill` would run, so the URL
        // must not carry a revision. Naming one asks for that EXACT version —
        // what an older run was given (k10r ADR-0023).
        let mut doors = Recorder {
            answers: vec![(
                "/code/37/4".to_string(),
                json!({ "rev": 2, "language": "python", "source": "print(2)" }),
            )],
            ..Recorder::default()
        };
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        let value = call(&mut doors, "read_code", &json!({ "code_id": 4 }), &world, &mut Vec::new())
            .expect("the read lands");
        assert_eq!(value["source"], "print(2)");
        assert_eq!(
            doors.gets,
            vec!["http://mem.test/code/37/4"],
            "no revision in the path = the current version"
        );

        let mut doors = Recorder {
            answers: vec![(
                "/code/37/4/1".to_string(),
                json!({ "rev": 1, "language": "python", "source": "print(1)" }),
            )],
            ..Recorder::default()
        };
        let value = call(
            &mut doors,
            "read_code",
            &json!({ "code_id": 4, "rev": 1 }),
            &world,
            &mut Vec::new(),
        )
        .expect("the version reads");
        assert_eq!(value["rev"], 1);
        assert_eq!(value["source"], "print(1)");
        assert_eq!(doors.gets, vec!["http://mem.test/code/37/4/1"]);
    }

    #[test]
    fn read_code_refuses_a_call_without_a_script_to_read() {
        let mut doors = Recorder::default();
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");
        let err = call(&mut doors, "read_code", &json!({}), &world, &mut Vec::new())
            .expect_err("no id, nothing to read");
        assert!(err.contains("code_id"), "{err}");
        assert!(doors.gets.is_empty(), "nothing was fetched");
    }

    #[test]
    fn write_code_delivers_a_new_script_and_stringifies_its_parameters() {
        // The writing half of execution: a model states a script and its
        // arguments the way it writes naturally (parameters as an OBJECT); the
        // door carries TEXT, so the conversion is this boundary's.
        let mut doors = Recorder::default();
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        let value = call(
            &mut doors,
            "write_code",
            &json!({
                "language": "python",
                "source": "print(sum(args[\"n\"]))",
                "description": "sum the numbers",
                "parameters": { "n": "array of integers" },
            }),
            &world,
            &mut Vec::new(),
        )
        .expect("the script lands");
        assert_eq!(value["posted"], true);

        let (url, body) = &doors.posts[0];
        assert_eq!(url, "http://mem.test/code/37", "scoped like every door now");
        assert_eq!(body["language"], "python");
        assert_eq!(body["source"], "print(sum(args[\"n\"]))", "verbatim");
        assert_eq!(
            body["parameters"],
            r#"{"n":"array of integers"}"#,
            "the object travels as the JSON TEXT k10r stores (it parses nothing)"
        );
        assert!(
            body.get("code_id").is_none(),
            "a write with no `code_id` is a NEW script — the store assigns the id"
        );
    }

    #[test]
    fn write_code_revises_in_place_by_naming_the_id() {
        // A correction keeps the address: skills that name this `code_id`, tasks
        // bound to it and its counters all keep pointing at the same script,
        // while the version that ran stays readable (k10r ADR-0023).
        let mut doors = Recorder::default();
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        let value = call(
            &mut doors,
            "write_code",
            &json!({ "language": "python", "source": "print(2)", "code_id": 4 }),
            &world,
            &mut Vec::new(),
        )
        .expect("the revision lands");
        assert_eq!(value["posted"], true);

        let (url, body) = &doors.posts[0];
        assert_eq!(url, "http://mem.test/code/37");
        assert_eq!(body["code_id"], 4);
        assert_eq!(body["source"], "print(2)");
    }

    #[test]
    fn write_code_refuses_a_call_it_cannot_carry_out() {
        let mut doors = Recorder::default();
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");
        let err = call(
            &mut doors,
            "write_code",
            &json!({ "source": "print(1)" }),
            &world,
            &mut Vec::new(),
        )
        .expect_err("a script without a language cannot be routed");
        assert!(err.contains("language"), "{err}");

        let err = call(
            &mut doors,
            "write_code",
            &json!({ "language": "python" }),
            &world,
            &mut Vec::new(),
        )
        .expect_err("a language without a body is not code");
        assert!(err.contains("source"), "{err}");
        assert!(doors.posts.is_empty(), "nothing was posted");
    }

    #[test]
    fn create_skill_refuses_what_it_cannot_identify() {
        let mut doors = Recorder::default();
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        let unnamed = call(
            &mut doors,
            "create_skill",
            &json!({ "content": "does a thing" }),
            &world,
            &mut Vec::new(),
        );
        assert!(unnamed.unwrap_err().contains("`name` is required"));

        let empty = call(
            &mut doors,
            "create_skill",
            &json!({ "name": "  ", "content": "does a thing" }),
            &world,
            &mut Vec::new(),
        );
        assert!(empty.unwrap_err().contains("`name` is required"));

        let no_body = call(
            &mut doors,
            "create_skill",
            &json!({ "name": "x" }),
            &world,
            &mut Vec::new(),
        );
        assert!(no_body.unwrap_err().contains("`content` is required"));

        assert!(
            doors.posts.is_empty(),
            "nothing reaches the store when the declaration cannot be made"
        );
    }

    /// The door that was missing: a script no skill names. `write_code` could put a
    /// row in the store and nothing could run it, so a model could write a tool it
    /// could never use.
    #[test]
    fn run_reaches_the_mount_without_a_skill_in_the_way() {
        let mut doors = Recorder::default().answering(
            "/code/37/4",
            json!({
                "language": "python",
                "description": "adds",
                "parameters": r#"{"type":"object","properties":{"n":{"type":"array"}}}"#,
                "source": "print(sum(args['n']))",
            }),
        );
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        call(
            &mut doors,
            "run",
            &json!({ "code_id": 4, "arguments": { "n": [1, 2, 3] } }),
            &world,
            &mut Vec::new(),
        )
        .expect("the mount answered");

        assert_eq!(
            doors.gets,
            vec!["http://mem.test/code/37/4".to_string()],
            "the row is read directly — no skill, no step"
        );
        let (type_name, key, args) = &doors.mounts[0];
        assert_eq!(type_name, "runner_py", "the row's own language chose the mount");
        assert_eq!(key, "channel-7");
        assert_eq!(args["code"], "print(sum(args['n']))");
        assert_eq!(args["args"]["n"], json!([1, 2, 3]));
    }

    #[test]
    fn run_counts_the_row_once_and_no_skill() {
        let mut doors = Recorder::default().answering(
            "/code/37/4",
            json!({ "language": "python", "parameters": "{}", "source": "print(1)" }),
        );
        let mounts = python_mount();
        let world = world(&mounts, "channel-7");

        call(
            &mut doors,
            "run",
            &json!({ "code_id": 4 }),
            &world,
            &mut Vec::new(),
        )
        .expect("the row runs");

        let touches: Vec<&(String, Value)> = doors
            .posts
            .iter()
            .filter(|(url, _)| url.ends_with("/touch"))
            .collect();
        assert_eq!(touches.len(), 2, "one row, one attempt and one success");
        assert_eq!(touches[0].0, "http://mem.test/code/37/4/touch");
        assert_eq!(touches[0].1["event"], "attempt");
        assert_eq!(touches[1].1["event"], "success");
    }

    #[test]
    fn run_refuses_a_language_no_mount_serves() {
        let mut doors = Recorder::default().answering(
            "/code/37/4",
            json!({ "language": "steel", "parameters": "{}", "source": "(code)" }),
        );
        let mounts = python_mount();
        let err = call(
            &mut doors,
            "run",
            &json!({ "code_id": 4 }),
            &world(&mounts, "channel-7"),
            &mut Vec::new(),
        )
        .expect_err("nothing serves steel here");
        assert!(err.contains("no mount serves `steel`"), "{err}");
        assert!(err.contains("python"), "the error names what IS declared: {err}");
        assert!(doors.mounts.is_empty(), "no node was asked to run anything");
    }

    #[test]
    fn run_enforces_the_rows_own_parameters() {
        let schema = r#"{"type":"object","required":["n"],"properties":{"n":{"type":"array"}}}"#;
        let mut doors = Recorder::default().answering(
            "/code/37/4",
            json!({ "language": "python", "parameters": schema, "source": "print(sum(args['n']))" }),
        );
        let mounts = python_mount();
        let err = call(
            &mut doors,
            "run",
            &json!({ "code_id": 4, "arguments": {} }),
            &world(&mounts, "channel-7"),
            &mut Vec::new(),
        )
        .expect_err("the row says `n` is required");
        assert!(err.starts_with("run: "), "{err}");
        assert!(err.contains('n'), "{err}");
        assert!(doors.mounts.is_empty(), "the refusal happens before the node");
    }

    /// The filing rule (k10r ADR-0016 as re-cut): one report per step that
    /// EXTENDED the walk, and only once the walk is a composition.
    #[test]
    fn a_walk_files_when_it_grows_into_a_composition() {
        let one = vec![
            Walked { code_id: 4, description: "fetch".into() },
        ];
        let two = vec![
            Walked { code_id: 4, description: "fetch".into() },
            Walked { code_id: 7, description: "format".into() },
        ];
        assert!(!should_file(0, 1, &one), "one row is not a composition");
        assert!(should_file(1, 2, &two), "a second DIFFERENT row is");
        assert!(
            !should_file(1, 2, &[
                Walked { code_id: 4, description: "fetch".into() },
                Walked { code_id: 4, description: "fetch".into() },
            ]),
            "the same row twice is a retry, not a composition"
        );
        assert!(
            !should_file(2, 2, &two),
            "a step that did not grow the walk files nothing: the composition \
             was already recorded when it appeared"
        );
    }

    // ---- tasks (k10r ADR-0021) ------------------------------------------

    #[test]
    fn open_task_posts_the_name_and_only_what_was_stated() {
        let mut doors = Recorder::default();
        call(
            &mut doors,
            "open_task",
            &json!({ "name": "ship-the-face-change" }),
            &world(&[], "channel-7"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(doors.posts.len(), 1);
        assert_eq!(doors.posts[0].0, "http://mem.test/tasks/37");
        assert_eq!(doors.posts[0].1, json!({ "name": "ship-the-face-change" }));

        // Stated parts ride along; absent ones are absent, not null.
        let mut doors = Recorder::default();
        call(
            &mut doors,
            "open_task",
            &json!({ "name": "x", "state": "doing", "next": "read the diff" }),
            &world(&[], "channel-7"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            doors.posts[0].1,
            json!({ "name": "x", "state": "doing", "next": "read the diff" })
        );
    }

    #[test]
    fn move_task_sends_only_the_move() {
        let mut doors = Recorder::default();
        call(
            &mut doors,
            "move_task",
            &json!({ "task_id": 9, "state": "blocked" }),
            &world(&[], "channel-7"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(doors.posts[0].0, "http://mem.test/tasks/37/9/move");
        assert_eq!(doors.posts[0].1, json!({ "state": "blocked" }));
    }

    #[test]
    fn move_task_with_nothing_to_move_is_refused_before_the_door() {
        let mut doors = Recorder::default();
        let Err(error) = call(
            &mut doors,
            "move_task",
            &json!({ "task_id": 9 }),
            &world(&[], "channel-7"),
            &mut Vec::new(),
        ) else {
            panic!("a move that names nothing is a mistake, not a no-op");
        };
        assert!(error.contains("send `state`, `next`, or both"), "{error}");
        assert!(doors.posts.is_empty(), "nothing was sent");
    }

    #[test]
    fn list_tasks_asks_for_the_state_it_was_given() {
        let mut doors = Recorder::default().answering("/tasks/37?state=open", json!([]));
        call(
            &mut doors,
            "list_tasks",
            &json!({ "state": "open" }),
            &world(&[], "channel-7"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(doors.gets, vec!["http://mem.test/tasks/37?state=open"]);

        // No state: every task the scope holds.
        let mut doors = Recorder::default().answering("/tasks/37", json!([]));
        call(
            &mut doors,
            "list_tasks",
            &json!({}),
            &world(&[], "channel-7"),
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(doors.gets, vec!["http://mem.test/tasks/37"]);
    }

}