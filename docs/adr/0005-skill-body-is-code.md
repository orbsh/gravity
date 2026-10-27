# 0005 — A skill's body is code, not a DSL

> **Languages:** [English](0005-skill-body-is-code.md) (primary) · [中文](0005-skill-body-is-code.zh-CN.md)

**Status:** Accepted (2026-10-26). Ruling: composition inside a skill is written
in an EXISTING language — steel or python — and never in a format this project
invents. One part of the execution path is unverified (§4: steel's ability to
evaluate a delivered string); that is a check, not an objection to the ruling, and
it is DEFERRED: python alone is enough to implement against, so nobody is to spend
time on the steel path until the priority rises on its own.
**Date:** 2026-10-26

## Context

ADR-0004 §3 landed the walk as ONE STEP PER CALL: `invoke_skill` runs step *n*,
the model passes what step *n+1* needs, and the sequence belongs to the model.
That is honest — nothing in gravity invents an order — and it is inefficient by
construction: each step is a round trip plus a full context re-read.

The shape that removes the round trips is a skill whose body orchestrates its own
steps. The question this record answers is what that body is WRITTEN IN.

Three candidates were on the table:

1. **An invented DSL.** A grammar for steps, guarded calls and results.
2. **JSON.** The composition as data — a list of steps with their arguments.
3. **Code.** A script in a language the node already carries.

JSON is what ADR-0015 §3 currently writes into `steps` (`[{code_id, note}]`), so
it is the de facto third candidate and deserves to be rejected explicitly rather
than by inertia.

## Decision

**Composition is code composing code. The body is a script in an existing
language; no DSL is designed here.**

1. **No invented DSL, and no JSON as the composition form.** A step language this
   project invents ends where every invented step language ends — a poor lisp
   dialect with a hand-written evaluator, a second set of control-flow rules, and
   no tooling. JSON is the other failure: it can express a fixed sequence with
   fixed arguments and nothing else — no condition, no reuse, no error path — and
   it is verbose at the size a real body reaches. Both are formats nobody can
   debug with the tools they already have.

2. **Code composition is still a graph, and the graph is derived.** A body calls
   other code nodes; those calls ARE the step edges. k10r's graph already holds
   the code nodes (ns 35) and the skill node (ns 36). The edge set of a skill is
   therefore READ FROM THE BODY — parsed, not hand-maintained — instead of kept
   as a parallel JSON list that drifts from the code it claims to describe. The
   graph representation of a composed skill is the same graph it was before; what
   changes is where the truth lives: in the body, with the edges derived.

3. **Both languages are admissible, and neither is the house language.** The
   choice is per skill and already expressible — the code row's `language` is
   what gravity routes on (ADR-0004 §3, ADR-0001 §7). What differs is what each
   language costs:

   - **steel** is homoiconic: a body can be read AS DATA without being run, which
     makes derived edges, rewriting and consolidation analysis cheap. It runs on
     the node's EMBEDDED steel runtime (`effector`'s `steel` carrier is
     resident-only, no process), so ADR-0001 §2's rejection of embedded carriers
     for generated code applies to it verbatim: the body would share the node
     process's environment, credentials and mounted filesystem, and bwrap covers
     process carriers only.
   - **python** is readable, it is the language models get right on the first
     attempt (which matters when the body is written by the emergence loop), and
     it is the only language with BOTH a one-shot process runner and a resident
     guest. Reading a python body as data costs a parser — CPython's `ast` in one
     subprocess, or a Rust parser — a real cost, but a bounded one, and it buys
     the same derived edges steel gets for free.

   The asymmetry is smaller than it looks from the outside: what steel has for
   free (the body IS the tree) python can have with one `ast` pass, and what
   python has for free (a process boundary) steel cannot have at all.

4. **What execution actually requires — and the one unknown.** The runner-mount
   pattern (ADR-0001) has a FIXED source per mount, so a skill's body must arrive
   as a call ARGUMENT and be evaluated by the mount's runner. That requires the
   language to evaluate a delivered string:
   - python: yes (`exec`), proven by `deploy/runners/runner_py.py`.
   - nu: no — measured (ADR-0001 §8: `source` demands a parse-time constant, nu
     0.116 has no `eval`).
   - **steel: NOT VERIFIED, AND DEFERRED.** Its carrier loads a source once at cold start, and
     nothing in this tree evaluates a runtime string; whether `steel-core` exposes
     an eval is an open check. If it does not, a steel body cannot ride a mount
     argument and would need either a per-skill type (rejected, ADR-0001 §2) or an
     in-process venue path (rejected, ADR-0001 §3) — which would leave steel as a
     WRITING form for bodies that execute as something else, or as a language the
     node carries for its own booths but not for skills.

   This does not block anything that matters: python is a complete implementation
   of the ruling (§3's second bullet: a process boundary, a one-shot runner, a
   resident guest, and the language models write correctly first), so building
   bodies in python is the whole of the near-term work. Steel's half becomes a
   check worth running when a steel-shaped requirement shows up — most likely from
   the static-analysis side (§3's first bullet), not from execution.

## Consequences

- **`invoke_skill` does not change.** A composed body is ONE step, so
  one-step-per-call stops being a limitation: there is nothing left to sequence
  from outside. The tool face, the walk and the error vocabulary stay as landed.
- **k10r's `steps` JSON list is the interim** (ADR-0015 §3), not the destination.
  The destination is a single code row holding the body, with the step structure
  derived from it. Until a body exists, the list stays what `resolve` answers and
  `invoke_skill` walks.
- **No DSL work is authorized anywhere in this project**: no grammar, no step
  vocabulary, no JSON composition format, no evaluator.
- **The node's language surface gains steel as a composition host only after §4's
  check.** A language that cannot evaluate a delivered script is a language whose
  skills cannot ride a mount; saying otherwise would be the lie ADR-0004 §8
  forbids.
- **The sandbox question is now a real decision, not a footnote.** If steel bodies
  are to run in-process on a node, that is an explicit acceptance that generated
  code shares a trusted process — the one thing ADR-0001 §2 rejected for exactly
  this code. Recording the acid test here keeps it from being discovered by
  accident.

## Open

- Does `steel-core` evaluate a runtime string (an eval, or an equivalent)? If not,
  the steel half of this ruling is a writing form only.
- Are derived edges computed at declaration time in k10r (Intr inspection on
  write) or at resolve time? Declaration time is cheaper to read, resolve time is
  cheaper to write — the same trade the code store already made for `parameters`
  (ADR-0012 §2: the producer owns the shape).
- Who owns the python AST pass — k10r at declaration, or a node-side introspect on
  the delivered body? k10r is the graph's owner, which argues for k10r.

## Related records

ADR-0004 (the fixed tool list; §3's one-step-per-call rule), ADR-0001 (the
execution surface: runner mounts, §2's embedded-carrier rejection, §8's measured
nu limitation), ADR-0003 (skill execution: code composes, progress is data), and
k10r's ADR-0015 (the skill node and what `steps` holds), ADR-0012 (the code store).