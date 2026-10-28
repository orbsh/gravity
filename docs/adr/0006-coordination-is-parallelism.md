# 0006 — Coordination is parallelism, never messaging

> **Languages:** [English](0006-coordination-is-parallelism.md) (primary) · [中文](0006-coordination-is-parallelism.zh-CN.md)

**Status:** Accepted (2026-10-28). A standing constraint on the turn executor; no implementation work of its own.
**Date:** 2026-10-28

## Context

Gravity is where a model decides anything. A turn is opened by a client (prism, the CLI) or by a venue trigger, runs one LLM stream with a tool-call loop, and appends the delta. Many turns may run at once, on many nodes, for many goals — that is what "multi-agent" means here.

The question this record settles is whether those turns also TALK to each other: mailboxes, subscriptions, an agent-to-agent protocol, an MCP client, a supervisor that instructs workers. Every current framework answers yes to some of it. The matrix answers no, and the reason is not taste.

Two things make the answer structural.

First, the target form. The stack is not built for a personal assistant with one owner and one working directory; it is built for a liquid organization of super-individuals, where crews form against a goal and dissolve when it is done. In that form "who can do what" is live state — a capability boundary drifts with use, and permission is topology. A messaging plane assumes the opposite: that the set of participants is enumerable, that a peer can be addressed, and that a channel between two of them is worth maintaining. None of that survives recomposition.

Second, the arithmetic. Notification-style collaboration costs one maintained channel per pair: N agents is N(N-1)/2 channels, and a broker only trades that for N channels plus a topological single point. The cost is not bandwidth but the semantics each channel must negotiate — identity, capability declaration, schema version, timeouts, retries, delivery semantics, idempotency. At the count a liquid organization reaches, the channel layer is the failure surface, not the capability.

## Decision

**RULING — gravity takes part in no agent-to-agent messaging. Coordination is state in the substrate, and parallelism comes from the task graph's topology.**

1. **A turn has one input and one output, and no inbox.** The input is the turn args plus the session projection; the output is the session append. Nothing else crosses the boundary: no message queue, no mailbox, no unread cursor, no acknowledgement. A turn that wants another turn to happen writes the fact that makes it happen (a task vertex, a memory fact) and ends.

2. **No protocol client, no tool registry.** Gravity speaks no MCP, no A2A, and registers no tool at runtime: the tool list is fixed, hardcoded, in groups, and every entry is one of this stack's own doors (ADR-0004). A protocol surface is paid for by interoperability between unfamiliar implementations — there are none here; every peer is the same stack, deployed by the same manifest. What a protocol would add is a dynamic trust surface: being able to hot-plug a tool or a peer at runtime is being able to rewrite the capability boundary at runtime.

3. **Coordination is state, so the tools that write it are the task and memory doors.** `open_task` / `move_task` / `list_tasks` (k10r's task surface, k10r ADR-0021) and `commit_memory` are how one turn makes work visible to others. Concurrency follows: an unfilled task vertex with no unmet dependency is available work; the graph is the scheduler's input, not a queue of messages. A dependency is an edge, not a request someone has to receive.

4. **What looks like a notification belongs to the venue, not to gravity.** A timer or a task event that activates a turn is Aura's event plane (a booth activation); the turn it produces still starts from the session projection, not from a payload handed over by a colleague. Gravity emits no agent-directed events, and no other component's turn is opened by something gravity addresses to it.

5. **The failure surface stays local.** With no delivery, there is no delivery semantics to design: no in-flight message to lose, no duplicate to de-duplicate, no ordering to guarantee, no backpressure to negotiate, and no retry that re-runs a model. A turn either writes its result to the substrate or fails visibly to its caller.

## Considered and rejected

- **A per-turn mailbox in k10r.** It would make the substrate a message store, add unread/ack state per turn, and put a second source of truth beside the session log and the fact graph.
- **An A2A or MCP client, so a turn could delegate to another vendor's agent.** This is the one case where a protocol pays off — and it pays off in a form this stack does not have: foreign implementations. A delegation that crosses the boundary is a turn submitted through prism with its own session, which already exists.
- **Pub/sub over Aura realm events as an agent instruction channel.** Realm events are the engine's runtime routing surface (stalls, keys, the persistent queue); using them to carry semantic instructions between turns would weld the cognitive layer's addressing to the runtime's, and put agent semantics inside a mechanism built for stall decoupling.
- **A supervisor turn that decomposes work and instructs worker turns.** It re-creates the message layer as prompt text, adds a hop that must be watched and retried, and makes quality a function of a hop count. Decomposition is `open_task` with `next` edges; the workers read the graph.
- **Passing context between turns as a message rather than through shared memory.** Two stores of the same knowledge drift; the session log and the graph are the one place a later turn looks.

## Consequences

- **Concurrency is bounded by the substrate, not by a broker:** k10r's write path and the mount capacity of the nodes decide how much runs at once. Provisioning is a deployment question, not a routing one.
- **The audit trail is the graph and the session log.** Correlating agent behaviour means reading facts, task vertices and appends — there is no message log to join against, and no "who told whom" to reconstruct.
- **The task doors must exist and stay in the fixed list** for this ruling to mean anything (ADR-0004 §2, group 3). A substrate that cannot answer `list_tasks` leaves its turns blind to each other's work, and the temptation to add a channel returns.
- **Open:** the activation rule that turns a newly opened task into a turn (venue-side, Aura's event/timer plane), and whether a task vertex needs a claim or fence so two turns do not take the same available work — that is task-graph semantics and belongs to k10r's task surface, not to this record.

## Related records

ADR-0001 (the execution surface: mounts, the script as an argument), ADR-0002 (the client form — a turn is submitted, never chatted with), ADR-0004 (the fixed tool list, and the task group that carries coordination), ADR-0005 (a skill's body is code), k10r ADR-0021 (the task door), aura ADR-0017 (the connection plane). The organizational form this follows from is argued in the wiki: `graph-memory.md` §液态化 and `liquefaction.md`.