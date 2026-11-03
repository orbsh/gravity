# 0010 — The venue sees the conversation, not the machine

**Status:** Accepted (2026-10-31)
**Supersedes nothing.** Extends ADR-0009 (one turn, two renderings) to the question it did
not answer: which facts of a turn are spoken to the venue at all.

## Context

A turn writes everything it learns to its log: the question, every tool call with its
arguments and result, every completion branch, the compaction summaries, the review facts,
the answer, the closing record. Keys carry `channel:turn`, so all of it is addressable and
replayable, and a client that fetches the session sees the curation the log has
already applied (ADR-0024 §5).

Speaking is a different act. Until now the booth emitted the answer's chunks, the end, and
nothing else — the receipt that names the turn was missing, so a client could not key the
sentence it had just sent to the answer arriving beside it, and every other fact had no
rule to say whether it should be spoken at all. Adding events one at a time would let the
answer stream fill up with whatever was easy to emit.

## Decision

**An event carries what a person watching the turn must READ while it happens. Anything a
machine needs to continue, audit or replay stays in the log.**

Two consequences make the rule checkable:

1. **The log is the record; speech is a projection of it.** No event may carry a fact the
   log does not hold — otherwise the conversation a person saw cannot be reconstructed from
   the store, and the two drift without anything failing.
2. **A fact whose only home is an event does not exist.** If it is worth showing, it is
   worth writing; if it is not written, it is not a fact of the turn.

| Fact of the turn | Venue | Why |
| --- | --- | --- |
| the question | **log** (its write is what mints the turn id) | spoken by the client that sent it, not by the booth |
| `ui:turn.opened {turn}` | **emit**, immediately after the mint | the sender needs the id before the answer to key its own sentence; without it two turns of one conversation cannot be untangled where they start |
| answer chunks | **emit** | the answer IS the speech (ADR-0009: frames for a renderer, `{delta}` for a program) |
| the model's own reasoning | **neither** | it is not the answer and not a record of the conversation. A "show the work" view is its own event name under its own name, opted into by a client — never the answer's delta stream, never a silent second stream |
| tool calls (`invoke_skill`) | **log** in full (arguments, result); **emit** at most a title and a status | what a person needs is what is being done and whether it worked; the payload is machine truth, and putting it on the answer stream duplicates the log onto the wire |
| memory retrieval (`recall_memory`) | **log only** | memory contents are not this conversation, and a stream that carries them is a leak surface no client asked for |
| compaction, review branches | **log only** | maintenance of the record, not something that happened in the conversation |
| `ui:turn.done` / `ui:turn.error` | **emit** | the end is SAID, not returned (ADR-0009) |
| configuration doors (model save, screen, layout) | **emit**, no turn id | they are not part of a turn |

Every emitted frame that belongs to a turn carries `turn` = `<channel>:<turn>` — the log
key's first two segments — on the envelope, beside `name` and `data`.

## The pattern, drawn

The order is: the client sends an event → the store writes the row (and mints the turn id) →
the application speaks the receipt and one batch of operations (the person's row, the region,
the bubble that reads it) → only then does the work run, patching what the open created →
the end is said. Nothing is drawn by the client, and nothing is re-drawn by a chunk.

The drawing side of this — the five steps, where container shape lives, and what is
deliberately absent from a frame — belongs to the renderer's own documentation:
**`fluxen/docs/interaction-patterns.md`**. This ADR keeps only what the application owes:
write first, speak before working, key every frame with the turn.

## Consequences

- A client that misses `opened` can still recover the id from the first chunk's `turn`; a
  client that misses both learns nothing about ordering and must refetch the session.
- Adding a step event later changes no existing stream: it is a new name with the same
  envelope, so a renderer that does not know it ignores it.
- Reasoning tokens stay unreachable from the venue. That is deliberate: a model's
  scratchpad shown as a conversation record is a claim about the answer that the answer
  does not support.
- The log carries strictly more than the venue, so "what did the person see" is a function
  of the log plus this table — a review can be answered without a second record.