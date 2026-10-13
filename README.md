# Gravity

Turn executor of the stateless agent architecture. Design: [stateless-agent-architecture.md](../../.hermes/wiki/stateless-agent-architecture.md) (wiki), Krystallizer (memory), Probe (skill runtime).

Pure Rust. The booth face runs as induction guest processes (stdio pipe protocol, `~/world/induction`): the turn streams as envelope frames on stdout and closes with `done: true` — single-call and stream share the one envelope shape.

## Model

One turn = one execution:

```
fetch_session(session_id) -> SessionView   // full mode, from Krystallizer
run LLM + tool-call loop                   // skill execution via Probe
append(session_id, delta)                  // only the new messages
```

Stateless between turns: no resident loop, no session state in memory. Compression is invisible here — when Krystallizer's fetched view carries a trailing compression prompt, Gravity just runs it; memory tool calls flow back and Krystallizer handles them on append. Active compression (upstream slowdown, user idle) calls `summarize` and ends the turn without a normal reply.

## Layout

- `crates/core` — turn executor: memory surface seam (in-process / HTTP), LLM loop, tool-call dispatch, SSE streaming
- `crates/cli` — local driver: for-loop over turns (local/single-machine mode)
- `crates/booth` — Aura Booth binding: one turn event per execution (distributed mode)

Same execution function, three drivers: CLI (local), Aura Booth (distributed), FaaS (serverless).

## Memory connection (Phase 0)

The turn function speaks one `MemorySurface` seam; the config picks the
form:

- `store_url` — mem-server's HTTP API (the single cross-process form;
  krystallizer ADR-0007 as revised 2026-10-10)
- `store_path` — a local fjall directory, mem-core linked in-process
  (a same-machine deployment detail, not a second access mode)

`store_url` wins when both are set.
