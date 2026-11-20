# Gravity

Turn executor of the stateless agent architecture. Design: [stateless-agent-architecture.md](https://github.com/orbsh/wiki/blob/main/stateless-agent-architecture.md) (wiki), Krystallizer (memory), Effector (skill runtime).

Pure Rust. The booth face runs as induction guest processes (stdio pipe protocol, `~/world/induction`): the turn streams as envelope frames on stdout and closes with `done: true` — single-call and stream share the one envelope shape.

## Model

One turn = one execution:

```
fetch_session(channel, member) -> SessionProjection  // head (rendered by k10r) + checkpoint + increment
run LLM + tool-call loop                   // skill execution via Probe
append(session_id, delta)                  // only the new messages
```

Tools (ADR-0004): the list is FIXED and hardcoded, in groups, under the names k10r's tool face already uses — `recall_memory`, `commit_memory`, `resolve_skills`, `invoke_skill`, `run` (a code row no skill names), and the task/code pairs. All are wired (`recall_memory` → `/memories/{scope}/recall`, `commit_memory` → `/memories/{scope}` ingest, `resolve_skills` → `/skills/{scope}/resolve`, `invoke_skill` → the step's code row, then `ctx.invoke` on the mount that serves the row's language — one step per call, the model drives the sequence). The list grows by door, never by enthusiasm: a tool the substrate cannot serve is a lie to the model. The `tools` array is part of the request's stable prefix, and it rides every inference round including the loop's re-opens.

Stateless between turns: no resident loop, no session state in memory. Compression: k10r owns the verdict (the trigger is evaluated at the projection read and arrives as `SessionProjection.compress` — `{checkpoint, prompt}`), and Gravity CARRIES it through both surfaces (the in-process one runs the same engine, so the two agree). The turn loop runs the summary out of band: TWO concurrent requests from the same immutable prefix — one answers, one summarizes — advanced round by round in the same loop, with the summary handed back through the checkpoint door (k10r derives the coverage point from the verdict it served). Each request has its OWN connection: a request is stateless, and what makes the fork cheap is the endpoint's prefix cache, not a shared stream. Its pulls are addressed by `stream_id`; today they land on two provider instances because the provider guest keeps one stream per process, which is the shape induction ADR-0002 removes. A branch still live when the answer ends is drained before the turn closes — not because the carrier reclaims the stream (streams live until `dispose` or eviction), but because a pull stream makes no progress without a puller and the pull loop IS the turn's call stack; the standalone/idle trigger that would free it from the turn's end is still open. Memory reads are k10r's doors; the head arrives already rendered.

## Layout

- `crates/core` — turn executor: memory surface seam (in-process / HTTP), LLM loop, tool-call dispatch, SSE streaming
- `crates/cli` — the venue client: one command = one call over the venue's own wire (`ui:turn.submit` out; `turn.delta`* / `turn.done` / `turn.error` back, marker stripped by the venue), no REPL, no client-side session state (ADR-0002)
- `crates/booth` — Aura Booth binding: one turn event per execution (the turn runs here, in the venue)

The turn function is ONE implementation (`crates/core`), executed by the booth. The CLI does not run it: it submits to the venue like any other client, so there is no second driver — a FaaS driver (same function, another host) stays deferred.

## The CLI

```
gravity [--json] [--session <id>] [--venue <ws-url>] [--cancel-after-ms <n>] <text>...
```

One argument = one turn (`-` reads stdin as ONE turn); `--json` prints one
object per turn for scripts, the default streams the deltas as they arrive.
Ctrl-C — or `--cancel-after-ms` — sends `ui:turn.cancel` as an event,
rather than dropping a turn behind a dead socket. Exit codes: 0 done, 2
usage/config, 3 auth, 4 the turn failed, 5 venue unreachable.

Config (`GRAVITY_CONFIG`, default `./gravity.json`) needs a `venue` node:

```json
{ "venue": { "url": "ws://127.0.0.1:7780", "username": "me", "password_env": "GRAVITY_VENUE_PW" } }
```

The password lives in the env var that field NAMES (never in the file). The
CLI logs in, falls back to signup once on a fresh venue, then submits.

WHO it speaks as is a uuid, because k10r's member id is the user's uuid and not a
counter: `member_id` beside the config, minted once on the first run (the run says so
on stderr and names the file), then read from there by every later run. It is plain
text on purpose — the real answer is the account's uuid from prism, pasted in, and
`--member <uuid>` overrides the file for one invocation. A client that minted per run
would be a new person to the store every time, with its own sessions and its own
executor in every mount.

No mount table in that file, and none on the wire from this client: THE BOOTH
reads the venue's `GET /admin/mounts` for the turn it is running, because the
booth is the party that lives inside the venue (prism publishes its own origin as
`PRISM_VENUE_URL`, which the spawned guest inherits). "A connected mount first
when two serve one language" is decided there, once, by `gravity-config`'s
`fetch_mounts`. That is what makes a page-submitted turn work at all: a page
carries no arguments of its own, so a table riding the call reached no browser
(prism ADR-0021, gravity ADR-0001 §7 as amended 2026-11-20).

`scripts/smoke-venue.sh` puts this binary on a real gateway — stub provider,
k10r's mem-server, prism, a real `effector` node, then `gravity --json hi` — and
asserts the reply on stdout, the message pair k10r recorded, the code row that ran
on the node and the graph it left behind. It is a script and not a test because it
wants four repos built and four fixed ports.

Two optional legs, each proving something the default cannot:

- `SMOKE_PAGE=1` drives the same turn from a page's `/channel` socket — one tab,
  two turns, nothing but the text submitted — and counts the frames the renderer
  receives (one per emit; N copies means N connections, never a growing fan-out).
- `SMOKE_PY=1` adds the RESIDENT leg: an `induction` python mount served by
  effector's guest runner (`deploy/runners/runner_py_guest.py`), which the node keeps
  alive across calls. One python row is called twice inside one turn and the second
  answer must be the first plus one — the row ran in a process that remembered — and
  the guest must still be exactly one live child of the node afterwards. The `nu`
  mounts stay what they are: one-shot `exec`, a process per call, no child left, so
  the two legs assert opposite shapes of the same fact (ADR-0014 §5).

## Memory connection (Phase 0)

The turn function speaks one `MemorySurface` seam; the config picks the
form:

- `store_url` — mem-server's HTTP API (the single cross-process form;
  krystallizer ADR-0007 as revised 2026-10-10)
- `store_path` — a local fjall directory, mem-core linked in-process
  (a same-machine deployment detail, not a second access mode)

`store_url` wins when both are set.
