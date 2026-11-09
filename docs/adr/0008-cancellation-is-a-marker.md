# 0008 — Cancellation is a marker the turn checks, not a kill

> **Languages:** [English](0008-cancellation-is-a-marker.md) (primary) · [中文](0008-cancellation-is-a-marker.zh-CN.md)

**Status:** Accepted (2026-10-30). The principle and the boundary rule are decided; where the marker lives so a BUSY turn can still see it was owed then and is DECIDED (2026-11-09): a row in the turn type's own storage, keyed by the turn id.
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

## Where the marker lives (decided 2026-11-09)

**RULING — a row in the turn type's own storage, keyed by the turn id.**

1. **The table is the type's own**, a `cancellations` collection in the ns every instance of that type already shares (ADR-0026 §1: one ns per type, all instances in it). One row per cancelled turn: key = the turn id, payload `{at, by}`, and the row's EXISTENCE is the cancellation.
2. **The writer is the type's SINGLETON instance — same type, a different instance, never busy.** A delivery addressed to an instance that is running a turn waits behind that turn: measured 2026-11-09, an event keyed by `device` took effect only after the turn ended, while the same entry CALLED on that instance overlapped with it (calls overlap, deliveries queue; ADR-0046 is what makes the call half true). The cancel is therefore declared key-less — `ui:turn.cancel` with an empty key field — which lands it on the type's singleton: that instance is never mid-turn, so it writes the row and returns while the turn streams. No second booth, no venue write, and no new ns: every instance of a type already shares one.
3. **The route is not gated; the two checks move into code.** A gate binds the ADDRESSED instance, and the singleton has no device and no user to bind. So the handler refuses a sender with no bound account, and the TURN honours the marker only when the row names its own member: a stranger's row can be written and still stops nothing. That split is what keeps the cancel both writable during a turn and unusable against someone else's.
4. **The key is the turn id, because the caller always has it.** A turn lands its submission and answers with a view carrying `turn_id`, and the cancel control on that view is bound to it. The id is also the only nameable thing: rounds overlap per device (ADR-0046), so "the current turn of this device" names several.
5. **No clearing duty.** Keying by turn means a marker can never cancel a later turn — the hazard the "marker is per turn and cleared with it" rule existed to prevent. The row may be deleted when the turn ends or kept as the record that the turn was cancelled. The address is `(kind, channel, turn)` and the channel is part of it: the store mints a turn number per channel, so turn 7 exists once in every one.
6. **Reads stay at the boundaries** of point 2 of the Decision: per chunk (the streaming default) or per round, and never inside a call.

**Rejected: `ctx.queue_depth` on the turn's own queue.** It needs no write path and no shared table, but it makes cancel a queued message again — read as a level instead of delivered as an edge: depth is a count with no identity (two cancels are indistinguishable, and any other event on the same name inflates it), a queue that lives in memory cannot carry a cancel across a cold re-entry, and it binds cancellation to the event plane's naming.

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
