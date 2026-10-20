# Gravity

Turn executor of the stateless agent architecture. Design: [stateless-agent-architecture.md](../../.hermes/wiki/stateless-agent-architecture.md) (wiki), Krystallizer (memory), Probe (skill runtime).

Pure Rust. The booth face runs as induction guest processes (stdio pipe protocol, `~/world/induction`): the turn streams as envelope frames on stdout and closes with `done: true` — single-call and stream share the one envelope shape.

## Model

One turn = one execution:

```
fetch_session(channel, member) -> SessionProjection  // head (rendered by k10r) + checkpoint + increment
run LLM + tool-call loop                   // skill execution via Probe
append(session_id, delta)                  // only the new messages
```

Stateless between turns: no resident loop, no session state in memory. Compression: k10r owns the verdict (the trigger is evaluated at the projection read and arrives as `SessionProjection.compress` — `{checkpoint, prompt}`), and Gravity CARRIES it through both surfaces (the in-process one runs the same engine, so the two agree); running the summary turn out of band and handing the text back through the checkpoint door is the turn loop's half, still unwired. Gravity no longer states a coverage point where it calls k10r — the door derives it from the verdict it served. Memory reads are k10r's doors; the head arrives already rendered.

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
