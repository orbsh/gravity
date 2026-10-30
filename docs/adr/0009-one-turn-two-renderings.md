# 0009 — One turn, two renderings: frames for a surface, raw for a program

> **Languages:** [English](0009-one-turn-two-renderings.md) (primary) · [中文](0009-one-turn-two-renderings.zh-CN.md)

**Status:** Accepted (2026-10-30). It fixes how the turn's output is addressed to its two kinds of consumer; it adds no second turn.
**Date:** 2026-10-30

## Context

A turn produces text one chunk at a time, and the consumers of that text are not alike. A web surface draws: it wants Accrete operations against slots it declared, because everything else on its page is such an operation and it has no business parsing a model's output. A program (the CLI) reads: it wants the deltas themselves, in order, and whatever it does with them is its own.

The tempting shapes are both wrong. Two turn implementations — one "for the UI", one "for the CLI" — is two places for the append rule, the tool loop and the tail branches to drift apart. A UI-shaped output with a parser on the program's side pushes the drawing vocabulary into a consumer that does not draw; the program would need to understand `patch /bind/value/default` to read a sentence.

The turn's behaviour does not change with its audience: it fetches the projection, runs the model and its tools, appends the exchange. Only the shape of what leaves changes.

## Decision

**RULING — the turn has ONE implementation and TWO renderings, chosen by a call argument (`protocol`).**

1. **`fluxen` draws, raw speaks.** `protocol: "fluxen"` makes every chunk a batch of operations against the slots the caller declared; the default (`data`) makes every chunk a raw delta at the event the caller named. Same loop, same appends, same failure rule.

2. **The names are the CALLER's, in both modes.** Slot names (`slots.rack`, `slots.text`) come from the caller — not because gravity happens to own no layout, but because the layout is the APPLICATION's, and only the party holding it can tell whether the frames it sends are right: a slot name is the sentence "my surface reads here", and that sentence is checkable only against a layout. A slot travels with the layout, never with the executor; gravity draws where it was told. A data-mode event name (`event`) comes from the caller for the same reason gravity mints no venue vocabulary. A view caller without slots, or a data caller without an event, is refused by name — gravity will not invent either.

3. **The renderings are not isomorphic, and that is stated.** View mode carries the DRAWING of the answer: the question bubble, the text region, the bubble that reads it, then one patch per chunk. Data mode carries the TEXT: one delta per chunk, nothing about the stage. A consumer that wants text from a view turn reads the patches; nothing emits both shapes at once.

4. **The end is said on the same wire the chunks were.** A drawing turn closes with `view:turn.done` / `view:turn.error` (a page's submit was an emit, so it has no answer it can read); a data caller reads the handler's terminal value, which is `{reply}`. Neither mode invents a second terminator.

5. **Both clients are clients.** The web surface (a renderer leg) uses frames; the CLI uses raw deltas. Neither is privileged: the CLI is not a second executor and the page is not a second model driver — a turn is the same booth call, reached through the same venue. The venue strips the `view:` marker on the way down (prism ADR-0017), so a program sees the plain names it already speaks while the plane stays marked.

6. **A cancelled or failed turn renders the same way.** Nothing about the output mode changes the append rule (ADR-0012: a failed turn appends nothing) or the cancellation boundary (ADR-0008).

## Considered and rejected

- **Two turn implementations (a UI one and a CLI one).** Two homes for the same rules; the second one is the one that rots, and the divergence shows up as "the CLI's answers differ from the web's".
- **Emitting both shapes in one envelope.** A consumer must then know which half to ignore, and the envelope grows a field no one asked for.
- **Making the CLI read frames and extract text.** It would have to learn the drawing vocabulary to read a sentence — the renderer's knowledge inside an automation client.
- **Making the page parse deltas.** The same error mirrored: the renderer would learn the model's transport format and would have to re-implement the streaming UI it already has.
- **A `--json` flag that reshapes frames.** Reshaping is the consumer's business; the booth states what it drew and what it said, and the client chooses.

## Consequences

- **The two modes are testable from one fixture**, which is what the booth suite does: the data mode collects `{"delta": …}` payloads and assembles the reply, the view mode collects operation batches and asserts the stage before the patch. A new consumer mode is a test of this record, not a new turn.
- **A caller can no longer be surprised by a different shape mid-turn**: the mode is fixed by the args, and an unknown `protocol` value is an error value naming the argument rather than a silent fallback.
- **The CLI's exit-code contract (ADR-0002) is untouched** — raw mode is what it always wanted; it simply stops being produced by someone else.
- **Open:** whether a turn should ever carry a MIXED rendering (text plus a progress frame for a tool round). Today it does not: tool results are the model's, not the consumer's.

## Related records

ADR-0001 (the execution surface), ADR-0002 (the CLI is a client of the venue, with exit codes as a contract), ADR-0007 (the configuration a turn reads), ADR-0008 (a cancelled turn renders like any other end), ADR-0012 (failure is a value). prism ADR-0017 (the marker, added and stripped by the venue) and prism `docs/MODELING.md` (the model this instance follows: an application is a view, and a program is a client of the same wire).
