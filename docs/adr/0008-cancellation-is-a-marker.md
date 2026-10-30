# 0008 — Cancellation is a marker the turn checks, not a kill

> **Languages:** [English](0008-cancellation-is-a-marker.md) (primary) · [中文](0008-cancellation-is-a-marker.zh-CN.md)

**Status:** Accepted (2026-10-30). The principle and the boundary rule are decided; where the marker lives so a BUSY turn can still see it is named as owed.
**Date:** 2026-10-30

## Context

A turn is not one atomic act. It opens a provider stream, reads tokens, runs tool calls between inference rounds, and only at the end appends the exchange. A user who changes their mind mid-answer has nothing to address: today prism's `turn_cancel` disposes an iterate stream, and with the pull face retired (PLAN Phase 4.1, `NO PULL FACE`) there is no stream to dispose.

Two obvious answers are wrong here.

**Killing the booth.** It is the hard inverse of the problem being solved: the realm holds the store pointers, the guest holds the tail branches and the reply parts, and a process that disappears mid-turn leaves k10r with an open compression node and no append. "Stop" would mean "lose the trace of what happened".

**Sending a cancel event.** The event plane delivers to an instance one event at a time, and the instance running a turn is exactly that instance: a `view:turn.cancel` emitted while a turn runs is queued behind it and is handled after the end it was meant to cut short. A message cannot cancel work the same mailbox is doing.

## Decision

**RULING — cancellation is STATE the turn observes, and the observation points are the turn's own boundaries.**

1. **A marker, not a message.** Cancelling writes a flag; the turn reads it. Nothing is delivered, nothing is interrupted from outside.

2. **The check lives at boundaries the turn already has**, and the granularity is a POLICY, not a constant: between streamed chunks (every token, every N tokens, every round), and between tool rounds. A marker checked once per chunk is a turn that stops within a token's latency; checked once per round it stops at the next inference round and costs one round's tokens.

3. **A function call is atomic.** A tool call in flight completes and its result is discarded; nothing inside a mount, a script, or a request is interrupted. The boundary rule is what makes this honest — the marker is never read in the middle of something that has no safe point.

4. **A cancelled turn appends NOTHING and says why.** The oldest rule holds: only a successful stream appends. The caller learns the turn ended on the same wire it was drawing on, with the cancellation as the reason — no new event vocabulary, no `cancelled: true` field for a consumer to forget.

5. **The marker is per turn and cleared with it.** A stale marker that survives its turn cancels the next one; a marker that a caller forgets to clear is a turn that never answers.

## The owed part

**Where the marker lives so a turn that is BUSY can still see it** — and who may write it. Two carriers are compatible with the ruling:

- **A row in the field store** (the application's own ns, beside the settings of ADR-0007). The turn reads it at each boundary. The writer cannot be the turn's own type — its instance is busy — so the write must come from a component that can reach that namespace while the instance works (the venue executing a store operation against the type's plan, or a second booth type that shares it). That is a capability question, not a vocabulary one: whoever writes must be authorized, and the authorization must not be the turn's own queue.
- **A pending-event count on the turn's own queue** (`ctx.queue_depth("view:…")`): the marker IS an event that waits, and the turn reads how many are waiting without consuming them. No write path, no shared namespace — but it makes "cancel" a queued message again, just one that is read as a level rather than delivered as an edge, and it ties cancellation to the event plane's naming.

Both are legitimate; the choice is the implementation's, and this record keeps the requirement that whichever is chosen is readable from inside a running turn and writable from outside it.

## Considered and rejected

- **Aborting the activation at the realm** (a job-cancel face in aura). It is the only true preemption, and it is also the only one that can leave the store with a half-turn and no record of why. If it ever lands it belongs to aura's scheduler for resource reasons, never as gravity's cancellation.
- **A client-side stop** (the page stops drawing and forgets). It leaves the model spending tokens and k10r appending an exchange nobody read, which is the state the marker exists to prevent.
- **A per-turn timer as the cancellation mechanism.** Time is not intent: a long answer that the user is still watching must not be cut by its own clock.

## Consequences

- The cancel path becomes a readable fact: an operator can see that a turn was cancelled, and when, because the marker is data rather than a signal that vanished.
- A policy knob enters the deployment (`cancel_check_every` or equivalent). Default: per chunk, which is what a streaming UI expects.
- The turn has one more boundary obligation: every loop that streams or dispatches must pass through the check, and a loop added later that skips it is a bug of this record's kind. The unit to test is "a marker set mid-stream stops the turn and appends nothing".
- Cancellation is not retry: nothing re-runs. The user's next message is the retry.

## Related records

ADR-0007 (the turn's configuration and its own storage — the same field store would carry the marker), gravity PLAN Phase 4.1 `NO PULL FACE` (the pull face whose disposal used to BE the cancellation), ADR-0012 (failure is a value; a cancelled turn is a value the caller reads), prism ADR-0017 (the wire a cancel action travels as an ordinary `view:` event).
