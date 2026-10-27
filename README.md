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

Tools (ADR-0004): the list is FIXED and hardcoded, in groups, under the names k10r's tool face already uses — `recall_memory`, `commit_memory`, `resolve_skills`, `invoke_skill`. All four are wired (`recall_memory` → `/memories/{scope}/recall`, `commit_memory` → `/memories/{scope}` ingest, `resolve_skills` → `/skills/{scope}/resolve`, `invoke_skill` → the step's code row, then `ctx.invoke` on the mount that serves the row's language — one step per call, the model drives the sequence). The list grows by door, never by enthusiasm: a tool the substrate cannot serve is a lie to the model. The `tools` array is part of the request's stable prefix, and it rides every inference round including the loop's re-opens.

Stateless between turns: no resident loop, no session state in memory. Compression: k10r owns the verdict (the trigger is evaluated at the projection read and arrives as `SessionProjection.compress` — `{checkpoint, prompt}`), and Gravity CARRIES it through both surfaces (the in-process one runs the same engine, so the two agree). The turn loop runs the summary out of band: it opens a second provider stream from the SAME prefix on its own instance key, advances both streams round by round, and hands the summary back through the checkpoint door — k10r derives the coverage point from the verdict it served. A branch still live when the answer ends is drained before the turn closes (the carrier releases the guest's state at `done`); the scheduled/idle trigger that would make it free of the turn's end is still open. Memory reads are k10r's doors; the head arrives already rendered.

## Layout

- `crates/core` — turn executor: memory surface seam (in-process / HTTP), LLM loop, tool-call dispatch, SSE streaming
- `crates/cli` — the venue client: one command = one call over prism's WS turn verbs (`turn.submit` / `turn.delta`* / `turn.done` / `turn.cancel`), no REPL, no client-side session state (ADR-0002)
- `crates/booth` — Aura Booth binding: one turn event per execution (the turn runs here, in the venue)

The turn function is ONE implementation (`crates/core`), executed by the booth. The CLI does not run it: it submits to the venue like any other client, so there is no second driver — a FaaS driver (same function, another host) stays deferred.

## The CLI

```
gravity [--json] [--session <id>] [--venue <ws-url>] [--cancel-after-ms <n>] <text>...
```

One argument = one turn (`-` reads stdin as ONE turn); `--json` prints one
object per turn for scripts, the default streams the deltas as they arrive.
Ctrl-C — or `--cancel-after-ms` — sends `turn.cancel`, the venue's own verb,
rather than dropping a turn behind a dead socket. Exit codes: 0 done, 2
usage/config, 3 auth, 4 the turn failed, 5 venue unreachable.

Config (`GRAVITY_CONFIG`, default `./gravity.json`) needs a `venue` node:

```json
{ "venue": { "url": "ws://127.0.0.1:7780", "username": "me", "password_env": "GRAVITY_VENUE_PW" } }
```

The password lives in the env var that field NAMES (never in the file). The
CLI logs in, falls back to signup once on a fresh venue, then submits.

`scripts/smoke-venue.sh` puts this binary on a real gateway — stub provider,
k10r's mem-server, prism, then `gravity --json hi` — and asserts both the reply
on stdout and the message pair k10r recorded. It is a script and not a test
because it wants three repos built and three fixed ports.

## Memory connection (Phase 0)

The turn function speaks one `MemorySurface` seam; the config picks the
form:

- `store_url` — mem-server's HTTP API (the single cross-process form;
  krystallizer ADR-0007 as revised 2026-10-10)
- `store_path` — a local fjall directory, mem-core linked in-process
  (a same-machine deployment detail, not a second access mode)

`store_url` wins when both are set.
