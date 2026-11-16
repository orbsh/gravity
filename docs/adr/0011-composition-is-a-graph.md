# 0011 — Composition is a graph: control edges, data edges, one run document

> **Languages:** [English](0011-composition-is-a-graph.md) (primary) · [中文](0011-composition-is-a-graph.zh-CN.md)

**Status:** Accepted (2026-11-16). **Supersedes ADR-0005** (a skill's body is code). **Amends ADR-0003**: §1's criterion and §2's glue-carrier rule are replaced; §3 (progress is data) and §5 (execution history never edits topology) stand, and matter more in this shape.
**Date:** 2026-11-16

## Context

ADR-0005 ruled that the composition of a multi-step skill is a script in an existing language, on two grounds: an invented step language ends as a hand-written evaluator, and JSON can express only a fixed sequence. ADR-0003 §1 ruled the same way from the other side: code is Turing-complete while a flow engine's ceiling is its own vocabulary, and the model writes code better than it writes a flow DSL.

Two requirements have since appeared that neither record had in view.

The first is a step that waits for a human. A script parked on a form has to hold its own continuation, and the resident guest's session is keyed by the conversation (ADR-0004 §3) — so the wait costs a resident interpreter per conversation and dies with the process. Persisting it instead means serialising an interpreter's locals.

The second is a run that must be forked, resumed, replayed and audited. An interpreter's locals have no address: "re-run from step 3 with one parameter changed" has no mechanism, and "what did step 3 actually receive" has no answer that is not a log scrape.

Once those are the requirements, ADR-0003 §1's argument inverts. Unbounded expressiveness is not a virtue in an executor that must be auditable, cost-bounded, renderable and statically checkable — a form whose programs can do anything cannot be examined at all. What ADR-0005 called a limitation is the contract.

## Decision

**RULING — a skill's composition is a graph held in its node, code is the leaf, and every run is a document.**

### 1. The test is protocol vs language, not expressiveness

A form in which a program can express unbounded computation is a language. A form in which it cannot is a protocol. A protocol's ceiling is a premise, not a defect: without the ceiling there is no enumeration, and without enumeration there is no audit, no static check and no cost bound. ADR-0003 §1's criterion (Turing completeness as an advantage) is replaced by this one.

### 2. Steps and control edges live in the skill node; there is no glue-script tier

`steps` becomes the definition: each step declares an `id`, a `kind`, and its own parts. Composition has one expression. A composition that needs real logic puts that logic in a `code` step — a step is a reusable, counted, reviewable row, which an inline script calling its siblings is not.

### 3. Three primitives, separated by when a step completes

`call` (an executor — a code row on a mount, or an addressable target — answers, and the answer completes the step), `request` (a declarative ask goes out and the step does **not** complete until data comes back: the only source of a run that stays pending for days), `send` (a message goes out one way; the step completes on delivery and nothing reads its output). Interaction is a node, not a script API: the node carries a *form* — a closed data definition, `title?` / `description?` / `fields[]` of `bool|string|integer|number|choice` — and requests the UI protocol itself; the dispatcher emits the request and the answer lands as that step's output. Approval is not a kind: it is a request whose form is one `bool` field. A request also carries an optional opaque `resource` (who answers: a URL, a device, a queue, a person), which is the reserved oracle slot — a host that resolves it skips the prompt.

A step's script receives no ctx and no handle on the run document. It is a pure function from its input document to its output document. That purity is what makes caching, sandboxing, forking and replay possible; a script that can reach for anything at any time has none of the four.

### 4. Data edges are derived; control edges are declared

Data edges come from the `inputs` references (§8) and carry no identity: no label, no budget, no name. They serve rendering as a second layer, fork impact analysis, and the reachability check behind §6. Control edges are declared in `out_edges`, and they carry the only three things an edge can say: `when` (§7), `budget` (required on a back edge), and at most one fallback for its selector.

The two meet at readiness: a control edge decides which steps are ACTIVATED, a data edge decides whether a step CAN RUN.

### 5. Every satisfied out edge is activated

A step's completion activates every out edge whose condition holds — not the first one, not one of them. Switch and parallel are therefore a shape rather than a mode: one edge per selector value is a switch, several is a fan-out, and there is no node-level `mode` flag that could disagree with the graph.

The fallback (`*`) fires only when no labelled edge on that selector matched. That is the one place where an edge's behaviour depends on its siblings, and it is stated here as a rule rather than left to the reader.

### 6. Readiness is a data question, not an edge count

A step is ready when every required reference in its `inputs` resolves; optional references may be absent and are handed to the code as absent, to be handled there.

- Compile time: a required reference must have at least one unconditional producer (a step reachable without crossing a conditional edge), or the plan is refused.
- Run time: an empty ready set with steps still pending ends the run as `stalled`, naming the references nothing fed.

This is why the token model's join problem has no counterpart here. A token-based model counts what arrived and must know how many were supposed to; a data-based one asks only whether its inputs exist, so a join waits for values, never for edges — and there is no "supposed to" to get wrong.

### 7. Conditions are selector equalities

A decision step emits ONE key whose value is a string from its own vocabulary; an edge tests `selector == value`, or is the fallback `*`. No negation, no conjunction, no comparison, no arithmetic, no expression grammar: a decision that depends on several facts computes its selector value in the code that emits it.

Two properties come free: mutual exclusion is structural (one key holds one value, so two branches cannot both fire from one decision), and multi-way branching needs no cascade of decision nodes.

### 8. `inputs` is the read declaration

A step's `inputs` maps the code row's own parameter names to references (`$.step_2.paths`) or literals. Those references ARE the reads — there is no separate `reads` field to drift from them, and the derived data edges are a projection of the same list.

The return value is the output, stored under the step's own id: single assignment, one key per step, several values as one object (`$.step_3.paths`).

### 9. The run document is an append log

Each traversal appends `{step, pass, output}`; a read resolves to the latest pass. A back edge never overwrites a pass, so what pass 3 received survives pass 4, and a replay compares inputs instead of reconstructing them.

### 10. A fork is a row, and reused work is marked

A fork is `{parent_run, parameter deltas, reused}`. The re-run set is the transitive closure of the data dependencies on the changed parameters; anything reused carries its parent run's id in the record. Mixing an old upstream with a new downstream is permitted and always visible — never silent.

### 11. Approvals bind to what was approved, and a rejection is an edge

An approval verdict is approve / reject(reason) / modify(parameters), not a boolean, and it is bound to a digest of the item it approves: a plan revised after the digest changed voids that approval and nothing else. A rejection is an out edge to a revise step whose input is the rejected items, their reasons and the plan version — so the approved subset and its approvals survive a revision instead of being re-litigated.

### 12. Execution history never writes topology

ADR-0003 §5 stands unchanged and matters more in this shape: the graph is the definition, a run is a row, and no execution path edits a step or an edge. A recurring composition is proposed for promotion (k10r ADR-0016), never self-applied.

### 13. One leaf crate: the scheduler and the validator

`ready(plan, run)`, `advance(plan, run, outcome)`, `validate(plan)`. No I/O, no graph types, no tokio, no aura: the plan IR is a plain owned struct, compiled from the skill node and cached under the content hash of the subgraph it was compiled from. It is a pure fold, and a table test is its whole test suite.

Everything else belongs elsewhere — timers, retries, dispatch and waiting to the dispatcher (gravity); persistence and the run document to the store (k10r); delivery to the event plane (aura). A scheduler that owns any of those is a service that can no longer be resumed from a document.

### 14. Aura is the delivery plane, not a rival shape

The graph says which steps are eligible; aura says which instance receives the work. Orthogonal, and deliberately so: the engine is never a blocking interpreter. Each advance is one invocation — compute, dispatch what can run, append, return. A run waiting on a human is simply pending; the process may die, and the next advance resumes from the document.

## Considered and rejected

- **Two node modes, `parallel` and `branch`.** A mode is definition that can disagree with the graph; the number of edges carrying one selector value cannot.
- **N booleans with ordered edges and first-match-wins.** The order becomes semantics living in a set rather than in the definition, so two readers of one plan can disagree, and exclusivity becomes a discipline the author must keep. A single selector makes exclusivity structural and needs no order.
- **Boolean conditions with negation.** Negation is the entry point of an expression grammar — `!a` is followed by `a or b`, then by a comparison. One selector value covers what negation was for.
- **Token-based joins (the AND/OR gateway shape).** A join that counts arrivals must also know how many were supposed to arrive, which no static check can settle and no runtime can infer without reachability games. Readiness over data deletes the question.
- **Keeping the ctx bridge inside scripts.** It is what makes a script impure: it can read anything at any time, so it cannot be cached, forked, replayed or bounded — the four properties this shape exists to buy.
- **Keeping a glue-script tier beside the graph for "simple" skills.** Two composers, one of which is the audit hole, and the choice between them would be made per author rather than per requirement.
- **A separate execution service owning timers and retries.** It duplicates aura's delivery and gives up resumability: a service's in-memory timers do not survive a restart, while a document does.

## Consequences

- **`invoke_skill` changes shape.** The model no longer names the next step: it starts a run (or advances one), and the scheduler decides what executes. One-step-per-call stops being the walk.
- **The `induction`-carrier requirement for composition disappears.** ADR-0003 §2 required a composed operation to ride a mount with a ctx bridge. A step is one call — arguments in, return value out — so it can ride the one-shot `exec` carrier, and nothing on the composition path needs a channel.
- **ADR-0005 is superseded, and this record authorizes no DSL either.** What it authorizes is a closed vocabulary of node kinds and three edge attributes — a protocol, testable by the §1 criterion.
- **k10r ADR-0015 §3's `steps` stops being interim**: it is the definition, still TEXT JSON the store does not parse.
- **Iteration and loops are bounded.** There is no map/iterate node: a fan-out width comes from data and is capped by a declared limit, so a run's worst-case step count is computable. A loop is a back edge with a budget, and a back edge without one is refused.
- **A plan is renderable, and the diagram is the definition.** A flowchart drawn from a plan is a projection of the stored edges, not a guess at an execution trace.
- **Two things must be designed outside this repo**: the UI protocol an `ask` / `approve` node requests (prism / fluxen side; not verified from here), and the satisfaction path that makes such a node executable headlessly.

## Open

- **Where the run document lives.** k10r ADR-0021 ruled that tasks are vertices with application state; a run document is the same kind of thing, so the likely home is that namespace rather than a new row kind. Not decided here.
- **Where the compiled plan is cached** — k10r (the graph's owner) or gravity (the executor). A content hash makes it a cache rather than a second truth in either case.
- **The validator's rule for a reference fed only by a loop body.** A step inside a loop reads an output its body produced in an earlier pass; "unconditional producer" must be stated so that a loop guaranteed one pass is legal and one that may never be entered is not.
- **The UI protocol's shape** for `ask` and `approve`, and what an unattended run does when nobody answers: fail the step with `needs_input`, or fail the run.

## Related records

ADR-0003 (amended here: §1's criterion and §2's glue-carrier rule; §3 and §5 stand), ADR-0005 (superseded), ADR-0001 (the execution surface and the runner mount), ADR-0004 (the fixed tool list, one-step-per-call), ADR-0010 (the venue sees the conversation), aura ADR-0043 (the ctx surface a mount call rides), and k10r ADR-0015 (a skill is a node, what `steps` holds), ADR-0012 (the code store), ADR-0016 (a proposal is a row; promotion is a declaration), ADR-0021 (tasks are vertices with a sheet).