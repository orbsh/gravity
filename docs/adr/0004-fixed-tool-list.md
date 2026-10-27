# 0004 — The tool list is fixed, hardcoded, and grouped

> **Languages:** [English](0004-fixed-tool-list.md) (primary) · [中文](0004-fixed-tool-list.zh-CN.md)

**Status:** Accepted (2026-10-26). Landed the same day: the turn sends the fixed
`tools` array on every inference round and ALL FOUR names are wired end to end —
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

   The task group (`task_open` / `task_done` / `task_list`) is declared but NOT
   listed: its write face has no route yet and the read side rides the session
   projection (SEAM.md §2 "Task progress"; ADR-0003 §3, PLAN Phase 2.6). A tool
   the substrate cannot serve is a lie to the model, and a model that has called
   one learns to distrust the whole list.
3. **The flow is the one the face describes: the model resolves, then invokes.**
   When the request cannot be answered from the prompt and the head, the model
   calls `resolve_skills`; the candidates come back as tool results; the model
   picks one and calls `invoke_skill`, which fetches the code row by id and
   hands the script to the mount as an argument. No skill ever becomes a tool
   definition of its own, and no step of that flow is a retrieval the model has
   to guess at.
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
   5): `recall_memory` and `resolve_skills` are `fast`; `commit_memory` and
   `invoke_skill` are `human`.
7. **The turn-start projection stays the primary memory read.** The head arrives
   already composed and rendered; these tools answer what the head cannot — a
   specific lookup, an enumeration the approximate face cannot do. They are not
   a replacement for the projection and must not become a per-turn poll: a turn
   that calls `recall_memory` before every step has turned a composed view into
   N round trips.
8. **The list grows by DOOR, never by enthusiasm.** An entry ships when its
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
- **Open:** the `invoke_skill` argument shape for a script that expects its own
  parameters — today `arguments` is one free-form object the model fills, and the
  code row's `parameters` is advisory text it can read but nothing checks; a
  schema check at the mount seam is unbuilt. And whether the graph's other
  entrances (`top`, `timeline`, `chain`, `anchored` — all landed as doors) should
  ever become tools or stay what they are: doors for a caller that already knows
  what it is looking for. The skill node's schema and namespace are CLOSED (k10r
  ADR-0015: kind `skill` in ns 36, the scope the facts live in).

## Related records

ADR-0001 (the execution surface: mounts, `carrier`, the script as an argument),
ADR-0002 (the client form), ADR-0003 (skill execution: code composes, progress
is data, code identity by id), `docs/PLAN.md` Phases 2.5 / 2.6 / 4.6 / 5, and
k10r `docs/SEAM.md` §2/§3 (the tool face, the names, the doors) plus
`docs/MODELING.md` (namespaces).