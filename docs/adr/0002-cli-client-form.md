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

5. **The caller is a tag, not a person.** `--tag <name>` is the whole identity:
   the member id is derived from the name, so the same tag is the same agent —
   and the same line — on every run, and nothing is stored, so a fresh checkout
   or a new CI worker cannot come up as a different speaker. There is no
   `--member` and no `member_id` file: this client is not the user, and a
   person's voice comes from the page they are looking at. A caller that does
   not name itself is a usage failure, because an unnamed caller has no history
   to continue and a client that mints one is inventing an identity.

6. **The environment is the venue's.** Tools run on the node that lives inside
   the venue the turn was submitted to (ADR-0001), so "run this where the
   incident is" means "submit to the venue on that machine" — not "link an
   executor into this process". Likewise the lifetime: one invocation is one
   turn, and anything that has to stay resident belongs to the venue, not to
   the client. A caller that needs facts about its own machine passes them in
   the text.

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
- **Settled:** the command surface is `--tag` (who calls), `--session` (pin a
  channel), `--venue`, `--cancel-after-ms`, `--json`, one positional turn per
  argument and `-` for stdin as ONE turn; exit codes are 0/2/3/4/5.
- **The unattended credential is a TOKEN** (`token_env`), and it is the whole
  way in: the client presents it on the dial and sends no login frame, so a CI
  step holds one secret and never an account's password. The venue mints it
  (`POST /admin/tokens`, which is login over HTTP — the password is what proves
  the request), and a config gives `token_env` OR `username` + `password_env`;
  neither is refused at first sight rather than at the first turn. A token the
  venue does not know leaves the connection anonymous, which the client reports
  as an auth failure (exit 3), not as a turn that ran as nobody.

## Related records

ADR-0001 (the execution surface this follows from), aura ADR-0017 (the
connection plane; prism as the WS gateway), `docs/PLAN.md` (Phase 6, and the
deferred FaaS gate).