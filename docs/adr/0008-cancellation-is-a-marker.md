# 0008 — Cancellation is a marker the turn checks, not a kill

> **Languages:** [English](0008-cancellation-is-a-marker.md) (primary) · [中文](0008-cancellation-is-a-marker.zh-CN.md)

**Status:** Accepted (2026-10-30). Revised 2026-11-09: cancellation is an INTERRUPT in two stages, and neither stage is storage — stage one is gravity's own boundary logic over a control bit in the realm's memory, stage two is the realm lifting an operation through the execution layer. What this record first called a marker survives as the boundary rule and the terminal error value; its storage carrier does not.
**Date:** 2026-10-30

## Context

A turn is not one atomic act. It opens a provider stream, reads tokens, runs tool calls between inference rounds, and only at the end appends the exchange. A user who changes their mind mid-answer has nothing to address: today prism's `turn_cancel` disposes an iterate stream, and with the pull face retired (PLAN Phase 4.1, `NO PULL FACE`) there is no stream to dispose.

Two obvious answers are wrong here.

**Killing the booth.** It is the hard inverse of the problem being solved: the realm holds the store pointers, the guest holds the tail branches and the reply parts, and a process that disappears mid-turn leaves k10r with an open compression node and no append. "Stop" would mean "lose the trace of what happened".

**Sending a cancel event as the whole mechanism.** The premise this paragraph carried on 2026-10-30 — one event at a time per instance, so a `ui:turn.cancel` queues behind the turn it means to cut short — no longer holds: the turn type registers `.unbounded()` (ADR-0046, `crates/booth/src/lib.rs:57-82`), so a cancel is served as a second activation while the first is mid-stream. The conclusion survives in a sharper form: an activation cannot reach into another activation's stack, so the event can only WRITE something the turn will read. A cancel event is the writer of the marker, never the cancellation itself.

## Decision

**RULING — cancellation is STATE the turn observes, and the observation points are the turn's own boundaries.**

1. **A marker, not a message.** Cancelling writes a flag; the turn reads it. Nothing is delivered, nothing is interrupted from outside.

2. **The check lives at boundaries the turn already has**, and the granularity is a POLICY, not a constant: between streamed chunks (every token, every N tokens, every round), and between tool rounds. A marker checked once per chunk is a turn that stops within a token's latency; checked once per round it stops at the next inference round and costs one round's tokens.

3. **A function call is atomic.** A tool call in flight completes and its result is discarded; nothing inside a mount, a script, or a request is interrupted. The boundary rule is what makes this honest — the marker is never read in the middle of something that has no safe point.

4. **A cancelled turn closes no turn and says why.** The oldest rule holds in effect: nothing of a cancelled turn reaches the next inference — its rows stay OPEN and the projection skips them (krystallizer ADR-0024). The caller learns the turn ended on the same wire it was drawing on, with the cancellation as the reason — no new event vocabulary, no `cancelled: true` field for a consumer to forget.

5. **The marker is per turn and cleared with it.** A stale marker that survives its turn cancels the next one; a marker that a caller forgets to clear is a turn that never answers.

## Where the stop lives (decided 2026-11-09)

**RULING — cancellation is an interrupt in two stages, and neither stage is storage.**

1. **Stage one is gravity's own logic, and it needs a bit rather than a carrier.** At a boundary the turn already has — between chunks, between calls — it reads a control bit and stops issuing work. That is the streaming loop of `stream_answer`, which is why this half is cheap: the boundary existed before the feature.
2. **The bit lives in the REALM, in memory, keyed by instance and call** — beside the watchdog, which is the same family: cancellation is resource control, not application state. A dead process owes nothing, and the trace that matters is the store's (the rows the turn wrote, left OPEN).
3. **The door is a framework primitive with two actions: SET (stage one) and LIFT (stage two — abandon the operation in flight).** It is not an application event, because an event addressed to a busy instance queues behind the very turn it means to stop: measured 2026-11-09, a device-keyed cancel took effect only after the turn ended, while the same entry CALLED on that instance overlapped with it.
4. **Stage two exists for the one case stage one cannot reach**: the turn is inside an operation — an LLM stream, a mount call, a script — with no boundary at which to see the bit. Then the realm lifts the operation through the execution layer, and the call returns an INTERRUPTED error value (ADR-0012). That value is TERMINAL: the turn must not retry it and must not route around it, or a forced stop is quietly eaten by a retry loop.
5. **The execution layer needs ONE primitive, at the finest granularity its carrier allows** — kill the script child, drop the in-flight request, or, where an embedded language cannot be reached at all, terminate the node process (the watchdog's coarse family). It does not need two stages: soft is gravity's, hard is this one.
6. **What the caller sees does not change.** The turn ends on the wire it was drawing on with a reason, closes no turn, and its rows stay OPEN for the projection to skip (krystallizer ADR-0024).

**LANDED (2026-11-09, across the three repositories below this one).** Stage one is real: `Realm::stop_set` sets the bit (aura ADR-0047), a guest reads it through `ctx.stop.poll` (induction's arm, bridged by effector to the host-fn table), and this booth's stream loop reads it at its chunk boundary and clears it when the turn ends, finished or not. The `cancellations` table and the key-less `ui:turn.cancel` route that carried the interim shape were REMOVED in the same wave — nothing about a stop is a row now. The venue sets the bit on the instance it is addressing, because it holds authority over the connection; the application still decides whose work that is, which is now the only place the ownership question exists. Stage two (LIFT) remains absent: it needs the execution layer's one primitive, and until it lands a turn parked inside a single long pull can only be stopped after that pull returns.

## Considered and rejected

- **Aborting the activation at the realm** (a job-cancel face in aura). It is the only true preemption, and it is also the only one that can leave the store with a half-turn and no record of why. If it ever lands it belongs to aura's scheduler for resource reasons, never as gravity's cancellation.
- **A client-side stop** (the page stops drawing and forgets). It leaves the model spending tokens and k10r appending an exchange nobody read, which is the state the marker exists to prevent.
- **A per-turn timer as the cancellation mechanism.** Time is not intent: a long answer that the user is still watching must not be cut by its own clock.

## Consequences

- The cancel path becomes a readable fact: an operator can see that a turn was cancelled, and when, because the marker is data rather than a signal that vanished.
- A policy knob enters the deployment (`cancel_check_every` or equivalent). Default: per chunk, which is what a streaming UI expects.
- The turn has one more boundary obligation: every loop that streams or dispatches must pass through the check, and a loop added later that skips it is a bug of this record's kind. The unit to test is "a marker set mid-stream stops the turn and it closes no turn" — asserting on what the projection returns, never on the log's row count, because the turn's rows are written as it ran and survive as OPEN.
- Cancellation is not retry: nothing re-runs. The user's next message is the retry.

## Related records

ADR-0007 (the turn's configuration and its own storage — the same field store would carry the marker), gravity PLAN Phase 4.1 `NO PULL FACE` (the pull face whose disposal used to BE the cancellation), ADR-0004 (a failure is a value the model reads; a cancelled turn is a value the caller reads), prism ADR-0017 (the wire a cancel action travels as an ordinary `view:` event).
