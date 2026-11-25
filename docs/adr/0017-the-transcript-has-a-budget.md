# 0017 — The transcript has a budget, and the turn reads it

Status: Accepted (2026-11-25)

## The problem

A turn sends whatever messages it has assembled and learns the cost afterwards — from the endpoint's
refusal. Two things follow. The transcript grows silently: tool results enter it raw and nothing
measures what the next request will cost. And a tiered endpoint (the one this was built against
charges by the band a request's prompt tokens fall into — `512k` inside a `1M` window starts the
higher band) charges the higher band on the **whole** request the moment it is crossed, once, with no
warning and no second chance in that round.

The one compaction that existed is a semantic trigger, not a measurement: finished tool rounds
collapse when the round about to be dispatched asks for a `human`-nature tool (a call that may not
answer for minutes, so the payloads it already read stop being worth their bytes). A turn made of
fast calls can therefore grow without limit until the endpoint says no.

## The decision

**The model's window is a fact about the model, so it lives on the model's row.** `models` gains one
cold field, `context_window`, a decimal string. Absent (or unparsable-proof absent) means the
deployment declares no window, and the turn senses nothing — exactly the behaviour before this ADR.
It is not on `settings`: one deployment may serve two models with different windows, and a window is
not a policy the deployment chooses.

**The measurement is the endpoint's, not ours.** When a window is declared, the request carries
`stream_options: {"include_usage": true}` and the turn reads `prompt_tokens` off the usage the stream
reports. A local tokenizer was refused: it is a second opinion about the same number, its estimate is
wrong in exactly the regime that matters (tool results, JSON, code), and the bill is computed from the
endpoint's count. A deployment without a window is not asked for usage at all — no field, no cost, and
a server that predates the field is never troubled with it.

**The number rides the decoder, not an event.** A chunk may carry content *and* `usage`; an event
variant would have to queue behind a delta already ready, and the queue loses the number the moment a
caller stops pulling. The SSE decoder keeps the last `usage` object it saw and hands it over when the
stream ends; the done envelope carries it (`{"done": true, "usage": …}`) because that is the round
boundary the turn already watches.

**Three lines, and each one has a reason beyond "it is high".**

| Line | Action | Why this number |
|---|---|---|
| `0.5` of the window | collapse finished tool rounds (`compact_finished_rounds`) | it is the tier line of a `512k`-in-`1M` endpoint: crossing it costs money on the whole request, so the collapse happens at the line, not near the top |
| `0.8` | also trim the oldest history rows to the last `HISTORY_KEEP` (8), leaving one line that says how many were dropped | the tail is what the next inference is about; the summary block above it stands for everything older, and those rows are the STORE's — the next projection hands them back |
| `0.95` | do not send the round: file a row that says so, close the turn with it visible, fail with that text | a round the endpoint would reject is not a round; and an empty reply would read as the provider's fault |

The decision itself is a pure function of two numbers (`budget_of(prompt_tokens, window)`), so the
boundaries are citable in a test without a provider.

**It is one round late, and that is stated rather than hidden.** `usage` describes the request that
just ended. The turn therefore decides the shape of the next request from where the last one stood,
plus nothing else — no estimate of what the next round will add. The error is one round's worth of
tool results, which is why the lines are placed with room above them (0.5 is a price line, not a
limit).

## What this does NOT do

- No tokenizer, no `tiktoken`, no character heuristics. If the endpoint reports nothing, nothing is
  sensed.
- No persistence of the number. A `usage` row would need a machine kind in k10r
  (`mem_core::channel::kind` has TEXT/TOOL_CALL/TOOL_RESULT), and inventing a kind for a diagnostic is
  k10r's decision, not this booth's. Today the reading goes to stderr as one line per gate
  evaluation: `[gravity] context 512340/1000000 (51%) · Compact`.
- No change for deployments that declare no window: the wire is untouched, and every code path
  returns `Budget::Free`.