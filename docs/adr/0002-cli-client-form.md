# 0002 — The CLI is a client, not a REPL

> **Languages:** [English](0002-cli-client-form.md) (primary) · [中文](0002-cli-client-form.zh-CN.md)

**Status:** Accepted (2026-10-14). Sequencing only; no implementation yet.
**Date:** 2026-10-14

## Context

Gravity's planned surfaces were three drivers of one function: a CLI for-loop
(local), the aura booth (distributed), and a FaaS driver (deferred). The CLI's
original framing was a local chat loop — a REPL over the turn function.

Two things changed the answer.

First, ADR-0001 moved execution off the local process entirely: gravity holds no
execution surface, so a CLI that wants anything done submits to the venue like
any other client. What remains "local" is where the memory service and the node
happen to run — a deployment fact, not a second driver.

Second, the product surface. The web front end is the primary interface for a
human in a conversation; a terminal REPL would be a second implementation of the
same interactive session (prompt state, streaming render, cancel semantics) with
a worse rendering surface. What a terminal is actually better at is the opposite
of conversation: composing, batching, and scripting — which is exactly what a
REPL is worst at (interactive state per invocation, human-paced, output shaped
for eyes rather than pipes).

## Decision

**RULING — the CLI is a client of the venue, shaped for automation.**

1. **One protocol face.** The CLI is a WS client of prism and speaks the
   existing turn verbs (`turn.submit` / `turn.cancel`, the delta stream to
   `turn.done` / `turn.error`). No second RPC face, no CLI-specific endpoint —
   "the interface is WS; the CLI wraps WS" is the standing ruling and it is what
   this follows.
2. **kubectl-shaped, not REPL-shaped.** One command = one call; stdin/stdout;
   meaningful exit codes; machine-readable output as the default for scripts and
   a readable rendering for terminals; batch input is first-class (a list of
   files, a glob, a pipeline) because "read these twenty files and tell me what
   they say" is the work the CLI exists for.
3. **No interactive session state.** No REPL, no local conversation loop, no
   prompt-and-wait. Session continuity is the channel's (k10r), not the
   terminal's — nothing needs to live in the client between invocations.
4. **Priority below the web front end.** The web is the human interface; the CLI
   is the automation interface. Development order follows that, and the CLI does
   not gate the phases that serve the web.

## Considered and rejected

- **A REPL.** It duplicates the web client's statefulness in a second place,
  with per-invocation interactive state that automation cannot use, and it
  re-creates the "second driver" fork ADR-0001 removed. A REPL is a UI choice
  that a client-form command can grow later if it ever earns its keep; the
  reverse (starting from a REPL) would have shaped the protocol surface around
  it.
- **A local CLI driver (gravity in-process, direct to the memory service).** It
  is the shape ADR-0001 forecloses for anything that executes, and for the rest
  it is a second path to the same three calls (fetch, infer, append) that the
  venue already owns.
- **A CLI-specific protocol (a leaner line protocol, an HTTP endpoint).** A
  second face means two auth paths, two streaming shapes, two failure shapes.
  The existing verbs already cover submit, stream, and cancel.

## Consequences

- **`docs/PLAN.md` Phase 6** is restated to this ruling: "CLI over Prism WS"
  stays the content; the "local CLI (direct Krystallizer)" reading is retired —
  a local memory store is a deployment fact (`store_url` rides the call), not a
  second driver.
- **The three-driver framing collapses** to one execution shape (a booth
  activation) plus clients. The FaaS driver in the deferred gates is unaffected
  in principle — a turn is still a function; only its tool execution needs a
  venue, which a function can reach over the network — and it is revisited when
  taken up.
- **No protocol work.** `turn.submit` / `turn.delta` / `turn.done` /
  `turn.error` / `turn.cancel` exist and are tested (prism `turns.rs` +
  `turn_e2e.rs`); the CLI adds a client, not a surface.
- **Open:** the command surface (verb names and their arguments) and the output
  conventions (JSON shape, streaming rendering, exit-code mapping) — client-side
  design, no venue impact.

## Related records

ADR-0001 (the execution surface this follows from), aura ADR-0017 (the
connection plane; prism as the WS gateway), `docs/PLAN.md` (Phase 6, and the
deferred FaaS gate).