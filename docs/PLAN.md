# PLAN

Design lives in the wiki (stateless-agent-architecture.md / krystallizer.md); this file only sequences phases.

## Milestone A — Local turn executor

- [ ] Phase 0 — Workspace skeleton: `crates/{core,cli,config}`. One happy-path turn: fetch_session (full mode) → LLM call → append delta. No tools yet; plain chat against Krystallizer.
- [ ] Phase 1 — LLM layer: provider config (model/adapter), streaming (token stream out), prompt-view byte-stability contract test (fetched view prefix must be byte-identical — context-cache correctness depends on it).
- [ ] Phase 2 — Tool-call loop: parse LLM tool calls → dispatch to Probe → record tool_call/tool_result into delta → feed back. One mode only; no privileged calls. Delta boundary: only this turn's additions, never a full-session write-back.
- [ ] Phase 2.5 — Transcript/history split + cold-call declaration: the in-turn tool loop keeps a working transcript in resident memory (calls, errors, retries — what the next LLM inference needs); session history receives only the net effect at turn end (intermediate failures are process, not memory). Every tool declares its execution nature at registration (fast/idempotent vs human-touch/external — declared per tool, static, rides `tool_invoke_count`'s registration info); declared-cold tools trigger the cold path at call time (transcript persist + prune before flush: history takes final state, transcript takes the minimal replay set), everything else stays hot with zero flushes. Gravity never branches on local vs remote — execution location lives in target resolution (user namespace + node alias + capability from the tool list), invoke targets come from the tool list, not from Gravity code.
- [ ] Phase 3 — Compression (passive): zero awareness in the normal path — run whatever view Krystallizer fetched, including a trailing compression prompt; memory tool calls flow back inside the delta. Active compression: `summarize` on external signals (upstream slowdown, user idle) ends the turn with no normal reply.

## Milestone B — Distributed execution

- [ ] Phase 4 — Aura Actor binding (`crates/actor`): the same execution function registered as an Actor type; one turn event = one execution; streaming via high-frequency emit events. CLI becomes a for-loop over the same function; Actor is its single call.
- [ ] Phase 4.5 — Unified call model adoption: tool calls go through `ctx.invoke()` (Aura CallSlot) — target = user namespace + node alias + capability (e.g. `probe:home-pc:read_file`); two-tier waiting handled by the runtime behind one `await` per call-site (hot: park on oneshot; cold: entry-split — no park, transcript flush, task ends, re-entry on result). Retention-window residency: executor stays in memory between calls/turns of the same session, session persisted at turn end or expiry; crash within window rebuilds from event stream. Multi-machine orchestration (e.g. "send the file from home PC to office PC") = two invokes composed in the turn loop — no remote/local branching in Gravity code.
- [ ] Phase 5 — Skill pull-through: on each tool call, fetch skill spec from Krystallizer via the pull-through path (Probe → Gravity → Krystallizer, never direct); view processing + `tool_invoke_count` weight write-back at this data gate.
- [ ] Phase 6 — CLI over Prism WS: wrap the WS protocol for remote sessions; local CLI (direct Krystallizer) and remote CLI (via Prism) share one protocol surface.

Deferred gates:

- FaaS driver: same execution function on serverless platforms — only after the Aura Actor path is stable; no new code, just another driver of the function.
- Multi-model routing: model selection per turn/session — after the single-model path is proven; LLM identity stays Gravity-side (cache binding), Krystallizer never learns it.
