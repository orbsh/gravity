# 0018 — The close is checked against the channel

Status: Accepted (2026-11-25)

## The problem

A turn ends by naming the rows that are the turn (`keep`), and the store accepts that claim on the
turn's word: it collapses everything else and keeps what was named. Nothing ever asked whether the
named rows exist. A row that the turn believes it wrote and the channel never got — a write lost on a
retry, a key remembered from an earlier turn, a race between two writers — is therefore invisible, and
the turn's record says the opposite.

This is the cheap half of the "a premise changed under a working agent" problem, and the half that
needs no notification at all: the turn already keeps an account of what it wrote (`keep`, and the
`(ts, sub)` key of every row it appended), so checking the account costs one read and no tokens.

## The decision

**The claim is verified before it is made final.** `TurnLog::verify` reads the channel
(`GET /channels/{id}/messages`, k10r's history — which is the channel, not the projection, so an OPEN
row is visible) and answers which of the `keep` keys are absent. The close then carries the verdict in
its own record: `content` is `""` in the ordinary case and one line
(`unverified: N row(s) in this turn's keep set are not in the channel (3:1, 5:0)`) when it is not.

**An unverified turn does not fail.** The answer already went out; failing after it would tell the
caller nothing true, and the caller cannot act on it. The distinction that matters is "it failed"
versus "it cannot be trusted", and the closing record is where the second one can be said. So the
success/failure semantics of a turn are unchanged, and the verdict is additive.

**A failed read is a turn failure**, on the other hand: if the channel cannot be read back, `verify`
returns an error and the close does not happen. Books that cannot be checked are not closed silently.

## Why not more

- **Not a check of the CONTENT.** Whether the row says the right thing is a judgment; it needs a model
  reading the transcript, which is exactly the cost this was built to avoid. A row that exists is a
  fact; a row that is correct is an opinion.
- **Not a check of the PREMISES.** The rows' *inputs* (a cluster, a fact, a task state) are a different
  read set, and deciding what to do when one moved is a separate decision — the one this one was
  carved out of.
- **No new store door.** The check uses the read k10r already serves. It is one full channel read per
  turn, which is fine for a conversation and would stop being fine for a very long channel; the day it
  hurts, k10r grows a range read and this is the caller that asks for it.
- **No retry, no repair.** A missing row is reported, not reconstructed. Rebuilding a row after the
  fact would hide the very thing worth seeing.

## What it caught, and what it costs

Cost on the ordinary path: one `GET` per turn, zero tokens, no extra rows written.

The harness that proved the shape is `missing_rows` (a pure function over k10r's history shape and the
keep set: five cases — all present, one absent, same timestamp with the wrong sub, nothing claimed,
and a body that is not a key). End to end the smoke closes its turns through this read, so a
regression in the read path shows up as a failed turn rather than a silent one.