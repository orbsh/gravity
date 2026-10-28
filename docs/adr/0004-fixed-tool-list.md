# 0004 — The tool list is fixed, hardcoded, and grouped

> **Languages:** [English](0004-fixed-tool-list.md) (primary) · [中文](0004-fixed-tool-list.zh-CN.md)

**Status:** Accepted (2026-10-26), amended (2026-10-27: the list is EIGHT — the
three task tools closed the gap this record's own grouping named). Landed the same
day: the turn sends the fixed `tools` array on every inference round and every name
in it is wired end to end —
`recall_memory`, `commit_memory`, `resolve_skills` (k10r ADR-0015's skill node
and its doors) and `invoke_skill` (the walk: a skill's step → its code row → the
mount that serves the step's language → the script runs on a node). Proven on
the real stack by prism's `tests/turn_invoke_skill.rs`.
**Date:** 2026-10-26

**Names withdrawn in this record:** `memory_search` / `memory_recall` /
`memory_top` / `memory_timeline` / `skill_search` / `skill_run` appeared in the
first draft of this record and are WITHDRAWN. The tool face already has names —
k10r's `docs/SEAM.md` §2 names it `recall_memory`, `commit_memory`,
`resolve_skills`, `invoke_skill` — and a second vocabulary for one operation is
exactly the drift an architecture record exists to prevent.

## Context

Phase 2 landed the tool-call LOOP — the guest parses `tool_calls`, dispatches
them, feeds the results back, and re-opens the provider — but nothing ever told
the model which tools exist. There is no `tools` array in the provider request,
and dispatch takes its target from the turn args (`tool_target`, default
`effector`) with the model-supplied name used directly as the entry point. So
the loop is live code on a surface that is never described: a real model cannot
call a tool it was not told about, and the only traffic that exercises the loop
is a test stub that emits `tool_calls` unprompted.

The tool face itself is not undesigned, though — k10r's `docs/SEAM.md` §2 lists
it operation by operation, with each operation's door and status. What was
missing is the decision about its SHAPE: how these tools reach the model.

Three shapes were on the table.

- **Dynamic: retrieve top-k skills per turn and inject each as a tool
  definition.** The model's affordances become a function of retrieval quality;
  a skill that misses the cut is invisible, and the tool block changes every
  turn, which also means the cached prefix changes every turn.
- **Fixed: a small, hardcoded list of a few tools that reach the substrate.**
  The model always sees the same affordances; skills are reached THROUGH those
  tools (`resolve_skills` then `invoke_skill`) rather than being flattened into
  the tool list.
- **Run modes: preset profiles, k10r being one of several substrates.** The same
  binary parameterized over different memory/skill backends.

## Decision

**RULING — Gravity is the topmost application and pure glue, so its tool list is
fixed and hardcoded, in groups, and it carries k10r's existing tool-face names.**

1. **Every group's substrate is an upstream.** Memory is k10r's (graph doors,
   memory doors, projection). Skills are k10r's rows plus the execution face
   (ADR-0001's mounts). Gravity adds composition and a prompt; it owns no
   substrate of its own. An affordance list that must be RETRIEVED is a
   consequence of a substrate Gravity does not have — the substrate already
   indexes itself, and reaching it is what `resolve_skills` / `recall_memory`
   are for.
2. **The list is hardcoded, in groups — memory, skill, task — under the names
   the tool face already has.** No new vocabulary: `docs/SEAM.md` §2 (k10r) is
   the register, and this record adopts it.

   | tool | arguments | what it does | today |
   |---|---|---|---|
   | `recall_memory` | `{query}` | `POST /memories/{memory_scope}/recall` — the landing plus its community, rendered by k10r, with k10r's read budget | LISTED (gravity `crates/booth/src/tools.rs`) |
   | `commit_memory` | `{facts:[…]}` | `POST /memories/{memory_scope}` — the graph's ingest | LISTED |
   | `resolve_skills` | `{query}`, `{limit}` | the SELECTABLE SKILL LIST — what the model consults when it cannot finish on its own; k10r answers candidates by intent | LISTED (2026-10-26): k10r ADR-0015 landed the skill node and `POST /skills/{memory_scope}/resolve` — a skill is a node in the same graph, so the graph's scope addresses it |
   | `invoke_skill` | `{skill_id, step?, arguments?}` | read the skill, pick the step, `GET /code/{id}` for its script, then the governed call `invoke(mount_type, session_key, "execute", {"code": …, "args": …})` on an effector node mount (SEAM.md §3) | LISTED (2026-10-26): ONE step per call — the sequence is the model's (§3) |
   | `create_skill` | `{name, title?, content, preconditions?, steps?, verification?}` | `POST /skills/{memory_scope}` — the declaration door k10r already serves (ADR-0015); the step-shaped parts travel as JSON TEXT, so the array→text conversion happens here, at the boundary that owns the wire | LISTED (2026-10-26, k10r ADR-0018): the model's act, symmetric with `commit_memory` — a skill that is never written down is a procedure the next conversation reinvents |   | `create_skill` | `{name, title?, content, preconditions?, steps?, verification?}` | `POST /skills/{memory_scope}` — the declaration door k10r already serves (ADR-0015); the step-shaped parts travel as JSON TEXT, so the array→text conversion happens here, at the boundary that owns the wire | LISTED (2026-10-26, k10r ADR-0018): the model's act, symmetric with `commit_memory` — a skill that is never written down is a procedure the next conversation reinvents |
   | `open_task` | `{name, next?, state?}` | `POST /tasks/{memory_scope}` — k10r's task door (k10r ADR-0021): a task is an ORDINARY vertex of the fact graph with its sheet in ns 44, so a dependency or a tag is a `commit_memory` fact and needs no tool | LISTED (2026-10-27): without it the third group of ADR-0016's list stayed a promise |
   | `move_task` | `{task_id, state?, next?}` | `POST /tasks/{memory_scope}/{task_id}/move` — absent fields are LEFT ALONE; the state is a closed set and a typo is refused at the door, because a task in a state no listing looks for is a task nobody finds | LISTED (2026-10-27) |
   | `list_tasks` | `{state?}` | `GET /tasks/{memory_scope}?state=` — the sheet's `by_state` face with a state, the kind scan without one | LISTED (2026-10-27); the turn also calls it WITHOUT a state and puts every task but `done` in front of the model — the state is on each line, because `blocked` and `open` mean different things to a reader |

   The task group (`task_open` / `task_done` / `task_list`) is declared but NOT
   listed: its write face has no route yet and the read side rides the session
   projection (SEAM.md §2 "Task progress"; ADR-0003 §3, PLAN Phase 2.6). A tool
   the substrate cannot serve is a lie to the model, and a model that has called
   one learns to distrust the whole list.
3. **The flow is the one the face describes: the model resolves, invokes — and writes down what it had to work out.**
   When the request cannot be answered from the prompt and the head, the model
   calls `resolve_skills`; the candidates come back as tool results, steps
   included (k10r ADR-0015 §4 — deciding needs to see what a skill would make it
   do, and no fifth tool exists to ask); the model picks one and calls
   `invoke_skill`, which fetches the code row by id and hands the script to the
   mount as an argument. No skill ever becomes a tool definition of its own, and
   no step of that flow is a retrieval the model has to guess at.

   **One step per call.** `invoke_skill` runs exactly one of a skill's steps, and
   the SEQUENCE is the model's: it saw the steps, it passes whatever the next one
   needs in `arguments`, and it decides whether to continue. Ordering, retry and
   feeding one step's result into the next stay Phase 5's — a second meaning of
   "invoke" inside the same tool would hide that control. The long-term form for a
   multi-step skill is not N calls but a single script that orchestrates its own
   steps; that is RULED in ADR-0005 (the body is code in an existing language, and
   the step edges are derived from it), and it changes what a skill's `steps`
   holds, not this flow.

   **Arguments are a JSON map, never argv.** A mount call carries ONE JSON document
   and every layer keeps it that way: gravity sends `{"code": …, "args": {...}}`,
   the carrier frames it as `{"handler": "execute", "args": …}` on stdin, and the
   runner hands the script the same named map (both runners: `TOOLRUNNER_ARGS` is
   that map as a JSON string, and the script answers with one JSON document on
   stdout). There is no expansion into `--flag value`, and none should be added:
   the script is not a command LINE, it is a body delivered as data, so there is no
   shell grammar to expand into; argv is typeless, which would undo the type
   checking the arguments now get; and quoting rules would become a contract every
   runner reimplements. A script that wants CLI ergonomics can parse its own map.

   That checking has TWO declaration surfaces, and they are the same language on
   purpose: the tool face's `parameters` (this list, sent to the provider) is JSON
   Schema, and it constrains the MODEL's call; a code row's `parameters` is JSON
   Schema, and it constrains the SCRIPT's `args` map. One vocabulary, two subjects
   — the subset the walk enforces (`required`, `properties.*.type`,
   `additionalProperties: false`) is a subset of what both surfaces already write,
   so neither side learns a second dialect. Known bound: `args` reaches the script
   through an environment variable, so a payload past the kernel's per-variable
   limit is a runner concern, not a schema one.
4. **No run modes.** A mode system parameterizes the application over substrates
   it does not have; the substrates it does have are k10r and aura, named once.
   A different memory plane is a different application, not a flag on this one —
   glue that grows profiles has stopped being glue.
5. **Deployment facts ride the turn args, never the list.** `store_url`,
   `endpoint`, `model`, `key_env` already do; the scope joins them
   (`memory_scope`). k10r's namespace numbers are k10r's declarations — a
   constant compiled into Gravity would turn a k10r renumbering into silent
   reads of somebody else's graph, and the failure would look like an empty
   answer, not an error. Absent a scope, the affected tool fails with the
   missing fact named; it does not default.
6. **Each entry declares its execution nature — static, per tool.** `fast`
   (in-process, idempotent read) versus `human` (touches the outside world, costs
   a person's attention or changes state). This is the field Phase 2.5's cold
   path reads and the field the `tool_invoke_count` write-back hangs on (Phase
   5): `recall_memory` and `resolve_skills` are `fast`; `commit_memory`,
   `invoke_skill` and `create_skill` are `human`.
7. **A call is counted where it happens, not at the end of the turn.** `invoke_skill`
   posts an `attempt` record before the node is asked to run anything and a `success`
   record when the answer comes back (k10r ADR-0018: `POST /skills/{scope}/{id}/touch`
   with `{event, ts_ms}`). The attempt goes first because a call that never returns
   must still be visible — `calls - ok` is the failure count, and it can only exist
   if the attempt was recorded without knowing the outcome. The door's answer is a
   RECEIPT: a counter that cannot be written never fails the step the model asked
   for. Nothing about this waits for the turn to end.

8. **The turn-start projection stays the primary memory read.** The head arrives
   already composed and rendered; these tools answer what the head cannot — a
   specific lookup, an enumeration the approximate face cannot do. They are not
   a replacement for the projection and must not become a per-turn poll: a turn
   that calls `recall_memory` before every step has turned a composed view into
   N round trips.
9. **The list grows by DOOR, never by enthusiasm.** An entry ships when its
   substrate answers; the names above are fixed now so nothing is renamed later.
   Concretely: `recall_memory` and `commit_memory` went out as soon as they were
   written, and `resolve_skills` followed the same day its door landed (k10r
   ADR-0015 — the bring-up pool read k10r withdrew with zero clients now has one).
   `invoke_skill` is what remains: it needs the walk over a resolved skill —
   `steps[].code_id` → `GET /code/{id}` → the language's mount (Phase 4.6).

## Considered and rejected

- **Top-k skill injection.** It makes the model's world model a function of an
  approximate retriever: the same question can expose different affordances on
  two runs, a newly created skill is invisible until its embedding converges,
  and the tool block (hence the prompt prefix) changes every turn for no gain —
  the model still needs a second call to run anything.
- **One tool definition per skill, generated at boot.** The list would grow with
  the graph, every addition would need a redeploy or a per-turn rebuild, and the
  model's context would fill with skills this conversation will never use.
- **A second vocabulary for the tools** (`memory_search`, `skill_run`, …). It was
  this record's own first draft and it is withdrawn: the tool face's names live
  in k10r's SEAM register, and two names for one operation split every later
  reader's attention between them.
- **Run modes / substrate profiles.** Rejected above: it is a configuration
  surface standing in for a different product.
- **Letting the model pick the memory namespace.** It is a deployment fact with
  a write-side neighbour (`commit_memory` writes facts), and a model that can
  choose where to read can choose where to write wrong.
- **Declaring the task tools now and answering "not available yet".** A tool
  that always fails teaches the model that failure is normal, which is exactly
  the habit a tool loop cannot afford.

## Consequences

- **The affordance list and the prompt are one prefix.** A fixed `tools` array
  means the stable part of the request does not move between turns; k10r's head
  and summary sit behind it, and both are already turn-stable.
- **The skill family's k10r side is PAID** (2026-10-26): the door is back and the
  schema exists — a skill node of kind `skill` in the fact graph, its four parts
  split between props (preconditions / steps / verification, JSON text) and its
  own incident facts (experience), with `POST /skills/{scope}` /
  `GET /skills/{scope}/{id}` / `POST /skills/{scope}/resolve` in front of it
  (k10r ADR-0015). Gravity's side followed the same day: `invoke_skill` walks a
  resolved candidate's `steps[].code_id` to its code row, matches the row's
  `language` against the deployment's mount table, and calls that mount with the
  script as an argument — `ctx.invoke` from the turn guest, so the turn is the
  only place both doors exist (`tools::Doors`, implemented in `bin/turn.rs`).
  Proven end to end by prism's `tests/turn_invoke_skill.rs`: a real node, a real
  `runner_py`, a real k10r, and the model's own round-1 body carrying `{"sum":7}`
  that a python child computed.
- **A tool result is the MODEL's, not the consumer's.** The turn feeds each
  result back into the loop (the assistant's call + a `tool` message, OpenAI's
  wire shape) and keeps it in this turn's transcript; it does NOT surface on the
  stream. prism's turn face carries `item.delta` text and refuses anything else
  ("envelope item carries no delta string"), so a tool_result item kills the turn
  at the pump — found by the smoke run, not by a unit test. A tool PROGRESS frame
  for a UI ("running `commit_memory`…") is prism's protocol to add, not a frame
  gravity invents behind another repo's face.
- **PLAN Phase 5** implements the pull-through and weight write-back on
  `invoke_skill`: the skill row is the registration info the counter rides, the
  counter write is additive data (ADR-0003 §5), no topology moves.
- **PLAN Phase 2.5** gets its declaration site: the entry's `nature` is static
  metadata, so "declared cold at registration" needs no runtime registry.
- **PLAN Phase 4.6 is PAID** (2026-10-26): the `run` call it was waiting for IS
  `invoke_skill`, and the language → mount match it was waiting for is a
  DEPLOYMENT FACT rather than a ruling — prism reads its own manifest
  (`PRISM_MOUNTS`, which already names both axes) and passes `mounts` in the turn
  args, so gravity matches a skill's declared language against what this
  deployment actually serves and, when nothing matches, says so by name (ADR-0001
  §7: a language with no mount cannot be called). There is no language DEFAULT to
  rule on: the code row states the language, and a row that states none is an
  error the model reads.
- **RULED — a multi-step skill's body is CODE, not a DSL** (ADR-0005,
  2026-10-26): the composition is a script in an existing language (steel or
  python), the step edges are DERIVED from that body rather than hand-kept in
  JSON, and no grammar, step vocabulary or JSON composition format is authorized
  anywhere. A composed body is ONE step, so §3's one-step-per-call rule stops
  being a limitation instead of being worked around. The language question is
  per-skill (the code row's `language`), with one verification outstanding: steel
  must prove it can evaluate a delivered string before a steel body can ride a
  mount (ADR-0005 §4).
- **PAID (2026-10-26): the arguments are checked against the row's `parameters`
  before a node is asked to run anything.** The field is TEXT the store never
  parses (k10r ADR-0012 §2 — the producer owns the shape), so gravity checks only
  what a producer DECLARED, in a JSON Schema subset (`required`, `properties.*.
  type`, `additionalProperties: false`, and `type` as a name or a list). A document
  that declares nothing — `{}`, an empty string, prose written for a human —
  constrains nothing, and passing it means exactly that: nothing was checked
  because nothing was claimed. Inventing a rejection there would break rows whose
  author chose description over schema, and the model could not fix it anyway. A
  failure never reaches the node, names the offending key or type, and hands the
  contract text back — so the model repairs its call in one round instead of
  learning the shape from three layers down.
- **Open:** whether the graph's other entrances (`top`, `timeline`, `chain`,
  `anchored` — all landed as doors) should ever become tools or stay what they
  are: doors for a caller that already knows what it is looking for. The skill
  node's schema and namespace are CLOSED (k10r ADR-0015: kind `skill` in ns 36,
  the scope the facts live in).

## Related records

ADR-0001 (the execution surface: mounts, `carrier`, the script as an argument),
ADR-0002 (the client form), ADR-0003 (skill execution: code composes, progress
is data, code identity by id), `docs/PLAN.md` Phases 2.5 / 2.6 / 4.6 / 5, and
k10r `docs/SEAM.md` §2/§3 (the tool face, the names, the doors) plus
`docs/MODELING.md` (namespaces).