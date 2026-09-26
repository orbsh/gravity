# Gravity

Turn executor of the stateless agent architecture. Design: [stateless-agent-architecture.md](../../.hermes/wiki/stateless-agent-architecture.md) (wiki), Krystallizer (memory), Probe (skill runtime).

## Model

One turn = one execution:

```
fetch_session(session_id) -> SessionView   // full mode, from Krystallizer
run LLM + tool-call loop                   // skill execution via Probe
append(session_id, delta)                  // only the new messages
```

Stateless between turns: no resident loop, no session state in memory. Compression is invisible here — when Krystallizer's fetched view carries a trailing compression prompt, Gravity just runs it; memory tool calls flow back and Krystallizer handles them on append. Active compression (upstream slowdown, user idle) calls `summarize` and ends the turn without a normal reply.

## Layout

- `crates/core` — turn executor: LLM loop, tool-call dispatch, SSE streaming
- `crates/cli` — local driver: for-loop over turns (local/single-machine mode)
- `crates/booth` — Aura Booth binding: one turn event per execution (distributed mode)

Same execution function, three drivers: CLI (local), Aura Booth (distributed), FaaS (serverless).
