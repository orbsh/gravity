# 0003 — Skill execution: code composes, progress is data

> **Languages:** [English](0003-skill-execution-shape.md) (primary) · [中文](0003-skill-execution-shape.zh-CN.md)

**Status:** Accepted (2026-10-14). Design ruling: the pieces it names are partly
landed (the ctx bridge out of a resident guest, the turn-start projection) and
partly owed (the task rows, the pre-dispatch gate).
**Date:** 2026-10-14

## Context

A skill that comes back from the graph is not one script. Its steps branch — a
「点餐」 branch carries steps 2, 3 and 4 with the rule "if 2 holds, call 3;
otherwise call 4" — a step can be a whole sub-procedure, and a task spanning
several tool calls has PROGRESS that outlives the turn that started it.

Three shapes were on the table for the execution half:

- a **glue script**: the branch is encoded in a script that calls the other
  scripts (the delivered script is data, so this needs no new mechanism — except
  a channel to call siblings through);
- a **flow engine**: control flow carried as data, interpreted by a runner;
- a **memo/todo pattern**: one task row per step, bound to the call that
  finishes it, advanced by code rather than by the model.

They are not three answers to one question. The first two are about control flow
INSIDE one invocation; the third is about progress ACROSS turns, and it exists
because advancing a multi-step task by LLM turns is slow. What they need is
mostly already there, verified: the ctx bridge out of a resident guest (a
delivered script calls a sibling mount and gets its value back — ADR-0001's
status), one line per relation in okm (`link` / `unlink` write and delete every
face in one batch; a row deletes via `Collection::delete`; nothing cascades —
the imperative pairing IS the discipline, okm ADR-0015 §B), and the projection
fetch gravity already performs at turn start.

## Decision

**RULING — code is the composition primitive, progress is data, and execution
history never edits the graph's topology.**

1. **Branch and step logic is a glue script, not a flow engine.** A branch that
   the caller's code can express does not need a second interpreter: the script
   is delivered as an argument (ADR-0001), it calls the sibling steps through
   ctx, and the composition is visible in one place. A flow engine's ceiling is
   its own vocabulary, while code is Turing-complete on an executor that already
   exists — and the model writes code better than it writes a flow DSL.
2. **Glue rides a mount whose carrier is `induction`.** The ctx bridge is a
   channel, and the one-shot `exec` carrier has none by contract (one JSON
   document in, one out; no frames, no residency, no ctx bridge). The manifest's
   `carrier` axis (ADR-0001 §7) therefore STATES which mounts can host a
   composed operation — "can this mount run glue" is a declaration, not a guess.
3. **Progress is data: one task row per step, bound to the call that completes
   it.** The binding is an edge in the same graph as the facts (the task-graph
   ruling: task vertices + their link edges in the graph; their completion state
   is structured application state, not a memory-pipeline fact — approximate
   retrieval cannot answer an enumeration question). Completion is a write at
   the call site, never a model judgement.
4. **The pre-dispatch gate is code, and inside a turn it reads memory.** Gravity
   pulls the session's open todos WITH the projection it already fetches at turn
   start; every in-turn check before dispatching a call is then a memory read —
   zero per-call round trips, no LLM on the path. The truth stays in k10r and
   the snapshot refreshes once per turn: progress written in this turn is
   visible to this turn's later checks, and a change from elsewhere arrives with
   the next projection.
5. **Execution history writes numbers, never topology.** Counters and weights
   are additive and reversible data (the `tool_invoke_count` write-back is the
   counter's home, Phase 5); a glue script is a session-scoped artifact (a data
   row, not a new parent). Moving a node under another parent is a DEFINITION
   change, so a recurring composition is PROPOSED for promotion and applied by
   an audited act — never self-applied. The graph's boundaries emerge from usage
   statistics (clustering over what was called and how it went); rewriting edges
   at runtime is not one of those statistics, and a per-session artifact
   promoted to structure would accrete as structure once per session that ran
   it. **Landed 2026-10-26 as k10r ADR-0016**: the proposal is a row gravity files
   at turn end, the count is application state on that row, and promotion is a
   human's declaration that copies the run's steps — no execution path writes
   topology. The principle is the one Aura already applies to types: emergent data
   never mints structure, because the curated graph is not the runtime's to
   edit.

## Considered and rejected

- **A flow engine.** It buys a visible, pausable, deterministic flow diagram —
  and pays by re-expressing branch, loop and composition in a vocabulary that
  caps at whatever the engine implements, on top of an executor that runs code.
  The one property it genuinely adds (the flow is inspectable) is deliverable by
  the task rows of decision 3, which are checkable data rather than a second
  interpreter.
- **Auto re-parenting on repeat ("the glue ran three times, hang these steps
  under it").** This is execution history rewriting shared topology: the graph's
  structure is curated and emergent-by-statistics, a glue node is per-session,
  and the first session that invented a composition would silently re-define
  every later reader's graph. Same rejection as "emergent data never mints a
  type", applied to edges.
- **The gate as an LLM step ("review which tasks are done").** It puts a
  probabilistic call on a deterministic fact: a full inference round for a
  comparison of rows, and it fails OPEN when the model forgets — the gate is
  exactly the kind of check the model must not own.
- **Gate state kept only inside the turn.** A turn-local todo list loses
  cross-turn and cross-session truth, and reconstructing it from the memory face
  cannot work: "all bound tasks are complete" is an enumeration, which the
  approximate retrieval face does not answer (application state never rides the
  memory pipeline).
- **A node-side dispatcher that interprets the steps (rejected earlier,
  re-confirmed here).** It re-implements the carrier runtime's sessions, pooling
  and sandbox inside a deployment artifact, and it moves routing out of the
  caller where the skill's language and carrier are already known.
- **Giving a mount a route/subscription declaration** (so a delivered script
  could `ctx.emit` into the realm's MQ plane and something would consume it).
  A mount is an executor, not a publisher: it has no consumer set, and inventing
  one makes a deployment artifact a graph participant — the thing ADR-0001 §5
  forecloses — while opening a second, unasked-for progress channel. The
  behavior WITHOUT it is stated and pinned: an emit out of a delivered script is
  accepted, reaches no consumer, and lands in the dead ring with `NoRoute`
  (ADR-0038 §4 — observable, never silently dropped;
  `prism/crates/prism/tests/runner_mount.rs`). Progress belongs in the call's
  return value or in the stream envelope (ADR-0034's iterate), where the caller
  is already reading.

## Consequences

- **PLAN Phase 2.6** is this ruling's implementation site: the task rows, their
  call bindings, and the pre-dispatch gate.
- **PLAN Phase 4.6** gains the carrier constraint: a composed operation needs an
  `induction` mount, and the manifest says which those are.
- **PLAN Phase 5**'s weight write-back is a counter write, not a topology edit.
- **okm side:** a re-parent is an explicit `unlink` + `link` pair — nothing
  sweeps edges when a node row is deleted, by design (okm's PLAN records the
  rejection of implicit one-side cleanup, including the stale-edge-resurrection
  failure it protects against). Any code that "moves" a node owns both halves of
  the pair.
- **Code identity:** a code node is a row addressed by its id — no name, no
  hash, and that is sufficient BECAUSE the script travels as a call argument and
  is re-read on every call (zero stale window by construction). The opposite
  case is a mount's own runner source, which a residency loads ONCE at cold
  start: that one stays content-addressed (ADR-0027's blob + reference), because
  an id-addressed residency source would silently keep running the old code.
- **Open:** the task row's schema (fields, the binding edge's kind, what
  "session" scopes the gate) and whether the completion write is gravity's or the
  runner's. The promotion path's form is no longer open: k10r ADR-0016 landed it —
  a proposal is a row (kind `skill_proposal`, counted, invisible to the resolver),
  gravity files it at turn end when a turn walked two different code rows, and the
  act that turns one into a skill is a HUMAN's declaration that copies the steps.

## Related records

ADR-0001 (the execution surface, the carrier axis, the ctx bridge),
ADR-0002 (the client form), `docs/PLAN.md` Phases 2.5 / 2.6 / 4.6 / 5, okm
ADR-0015 §B and `okm/docs/PLAN.md` (relations: `link`/`unlink`, no implicit
cleanup), aura ADR-0043 (the ctx surface a glue script calls through), and the
memory records on emergent skill boundaries (usage statistics, not edge
rewrites).