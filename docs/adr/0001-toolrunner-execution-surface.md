# 0001 — ToolRunner: the execution surface is a node mount

> **Languages:** [English](0001-toolrunner-execution-surface.md) (primary) · [中文](0001-toolrunner-execution-surface.zh-CN.md)

**Status:** Accepted (2026-10-14). Nothing is implemented yet; this record fixes
the shape the phases are sequenced against.
**Date:** 2026-10-14

## Context

Gravity is the turn executor: fetch the session projection → the LLM call and
its tool-call loop → append the delta. Phase 2 landed the loop. Two questions
were left open, and the second one has a deadline built into the product.

**1. What must exist before a tool can be called?** aura routes a call by type:
`engine.call(target: InstanceId, handler, args)`, where the target is a type name
plus an instance key (`aura/crates/engine/src/lib.rs`). A remote node is reachable
only through a registered type whose body is
`Body::RemoteProbe { node_alias, language, source, encoding }`
(`aura/crates/realm/src/instance.rs`, the dispatch arm). That type is aura's
sanctioned binding record for a node — "type-affinity records ARE the binding
surface … the type definition IS the binding record; no new table needed"
(`aura/docs/PLAN.md`, 4.10) — and no production code registers one today; only
aura's own tests construct it (`aura/crates/engine/tests/remote_effector.rs`).

**2. Do emergent skills become types?** No design survives that. A skill is a
k10r graph subgraph ("a subgraph with a high tool index"), and the emergence
loop's contract is zero staleness: a call may load different experience
depending on context, and the same skill gains experience after a call and
becomes a new version. Type registration is a deployment act with deployment
semantics — introspection (a throwaway child spawn), a definition row, a storage
ns, routes, and **hot-swap on re-registration: the type's routes are dropped and
every instance of it is evicted** (`aura/crates/realm/src/registry.rs`). Binding a
skill, or a skill version, to that clock gates the emergence loop on deployment
instead of on data.

What the wire already provides (verified against the code, not assumed):

- The remote frame carries both halves: `ToolCall { call_id, kind, stream,
  session, entry, language, encoding, args, code }`. `args` is passed through
  verbatim (`args: job.args.clone()`), and `entry` is resolved by nothing on the
  effector side — "This is NOT a lookup key: the Effector resolves nothing
  against it (no registration, no validation)"
  (`effector/crates/protocol/src/lib.rs`).
- Code is content-addressed, fetched by the effector, verified against the
  frame's `sha256`, cached by hash (aura ADR-0027;
  `effector/crates/runtime/src/remote.rs`).
- A node has no inbound reachability: it dials out to the control plane, which
  holds the connection indexed by node alias
  (`effector/crates/runtime/src/remote.rs`; prism's `/effector/<alias>` mount →
  `realm.effectors`).
- A node's outbound connection is its only door, and the realm is the only side
  that holds it — so any byte that reaches a node travels the control path.
- Embedded carriers are structurally unsandboxed: `spawn_session` builds the
  steel/python sessions without a sandbox policy, and only the process carriers
  (induction/exec) are spawned under bwrap
  (`effector/crates/runtime/src/carrier/session.rs`; `sandbox.rs` says it
  outright: "in-process carriers (steel/python)"). In-venue booths are spawned
  with a hardcoded `SandboxPolicy::None` (`aura/crates/realm/src/instance.rs`),
  and aura has no per-type sandbox declaration.
- A resident session loads its source at cold start only: the same session key
  presented with different source text keeps running the old code
  (`effector/crates/runtime/src/carrier/session.rs`).

## Decision

**RULING — execution happens on a node, through a fixed runner; skills are data.**

1. **ToolRunner is the execution form.** A node mount is a `RemoteProbe` booth
   type whose source is a *fixed deployment artifact*: a runner (interpreter or
   dispatcher) that receives the operation's code and arguments as call
   arguments and executes them. The mount's code never varies per skill or per
   version; everything dynamic is data.
2. **Skills never become types.** An emergent skill stays in k10r; its script is
   fetched by gravity and travels as a call argument. No type, no ns, no route,
   no definition row, no hot-swap per skill version — and no content-addressed
   code delivery for it either: the script is KB-scale and rides the call as an
   argument, which is not "code delivery" in the protocol's sense at all.
3. **Every machine runs a node, including the control plane's.** There is no
   privileged local path and no in-process execution surface in gravity: local
   execution is a call to the local node's mount (loopback). Gravity therefore
   carries no execution-plane dependency — effector-runtime does not enter its
   dependency tree.
4. **The runner is a deployment artifact (a booth).** Fixed code, installed with
   the node, declared in the node manifest; it is infrastructure, not a graph
   citizen — it is never searched for, never named by experience, never written
   back to.
5. **Memory and retrieval tools are not ToolRunner calls.** k10r's HTTP surface
   is data access, not code execution; gravity keeps calling it directly
   (krystallizer ADR-0007 as revised: HTTP is the single cross-process form).
6. **The session key stays the caller's.** `session` is opaque residency
   identity; gravity chooses it (session, user, or both). It does not need a
   version component — because the runner's source is fixed, the cold-start-only
   load quirk cannot bite.

7. **The node's language surface is declared, small, and never chosen by data.**
   Required installs: nushell and python (steel is compiled into the node binary —
   no install). nu is the systems language and python is the business-logic
   language; there is no shell carrier and no fallback to bash — OS-level
   operations route to nu mounts, because a `bash` path would reintroduce
   quoting, invocation, and portability surface that nu's built-ins exist to
   remove. Enforcement is structural, not advisory: a language with no mount
   cannot be called, and no data path mints mounts. (A nu script could still
   shell out to an external binary; that is a sandbox-policy concern — what the
   node's bwrap policy exposes — not a routing one.) An operation's language is
   a property of the skill (its script is data, and data has a language) and
   gravity routes to the matching mount; the LLM never picks a carrier at call
   time. Precision: the manifest's `language` names the CARRIER, while the
   operation's language rides the mount's NAME by convention (a mount runs what
   its runner runs — `runner_nu` / `runner_py`); making it a first-class
   manifest field is open.
8. **A runner per language; where the boundary matters, a process — and
   in-process evaluation decides who can be resident.** Two runner forms: the
   one-shot `exec` carrier (argv spec, request JSON on stdin, stdout read whole
   — no frames, no residency, no ctx bridge), enough for stateless system
   operations and the simplest thing to deploy; and the resident `induction`
   carrier (a guest speaking the frame protocol, carrying residency plus the ctx
   bridge), in which the delivered code would run in the guest's own process.
   Verified: **that second form is not available in nu** — nu cannot evaluate a
   runtime string in-process (`source` demands a parse-time-constant path, and
   nu 0.116 has no `eval` command), so a nu guest could only shell out to a
   child, and a child never sees the guest's ctx scope. In-process evaluation is
   what puts residency within reach: python can — a guest process whose `exec`
   runs the delivered script in its own scope, where the guest's ctx helpers are
   visible — and nu cannot. So nu is the one-shot systems language
   (`deploy/runners/runner_nu.nu`), python is one-shot today
   (`deploy/runners/runner_py.py`) and the resident candidate once a python
   guest exists, and that guest is a deployment artifact that does not exist
   yet. The embedded python carrier (pyo3, in-process CPython in the node's own
   process) is still NOT a runner form: it admits no process boundary.

## Considered and rejected

- **Integrating effector into gravity (an in-process execution surface).** It
  buys cohesion for a single-artifact release, and costs the sandbox:
  embedded carriers admit no process boundary by construction, so
  LLM-generated code would run inside gravity's process. It also creates a
  second residency owner (a second session table, eviction policy, and sandbox
  configuration) beside the venue's — the split that ruling 3 exists to avoid.
  The release-cohesion goal is served by packaging instead: the venue and its
  nodes ship together when a self-contained install is wanted.
- **Per-call content-addressed code refs, or restoring an inline form.** Both are
  answers to "where do the bytes live", and that question only exists if the call
  carries *code*. With a runner the varying part is an argument, so the question
  does not arise: ADR-0027 is untouched (the runner is a definition — blob +
  ref, exactly today's case), and no protocol edit or errata is needed.
- **A local in-venue runner booth, no per-machine node.** Saves one resident
  process per machine. Rejected because in-venue spawns hardcode
  `SandboxPolicy::None` and there is no per-type sandbox declaration to use, and
  because a steel/python in-venue booth would be embedded — no process boundary
  at all. It would also make local execution behaviorally different from remote
  execution of the same call.
- **Gravity executing locally via the effector library, remotely via the mount.**
  Two residency owners, two sandbox policies, two test surfaces for one call
  site. The call site would be one; the semantics would not.

## Consequences

- **gravity:** no execution-plane dependency; the tool list is fixed and tiny
  (skill search + run); one call site — `invoke(mount, key, entry, args{code,
  args})`; scripts come from k10r's HTTP face and travel as arguments.
- **k10r:** the script store, with gravity as its only client. The node never
  talks to it — the byte path is gravity → control path → node.
- **prism:** one new piece of work — a mount-registration face. Per (alias ×
  language) a `RemoteProbe` type whose source is the runner artifact.
  `RemoteProbe` definitions are not persisted, so this is an idempotent
  registration per boot, and the dial-in `Frame::Register { node_alias,
  carriers }` already supplies the alias and the carrier set for the trigger.
- **aura:** no change. Only existing surfaces are used (`engine.register`,
  `Body::RemoteProbe`).
- **effector:** no change. The runner is a definition; the skill script is an
  argument; the frame vocabulary, the codec, and the code reference all stand.
- **Deployment:** every machine (control plane included) runs a node; one runner
  artifact per language — or one dispatcher — ships with it; the node manifest
  declares carriers, sandbox policy, and capacity. "Deployed where, can operate
  there" (aura ADR-0015) becomes literal: a machine's abilities are its manifest.
- **Residency:** one session table, one eviction policy, one sandbox
  configuration — the venue's and the node's, never gravity's.
- **Foreclosed:** gravity running standalone *with* execution ability; any local
  privileged execution path; per-skill or per-version registration; a
  protocol-level per-call code form.
- **Settled here:** the runner's bound entry is `execute` — the effector's own
  fallback convention (an unbound name falls back to `execute` when the code
  defines it) makes it the one robust choice, and a test pins it.
  One mount per language, with dispatch in the caller: gravity routes by the
  skill's declared language to the matching mount. A node-side dispatcher
  process was rejected — it would re-implement the carrier runtime's session,
  pool, and sandbox machinery inside a deployment artifact, for a saving of a
  handful of static registrations. Manifest authorship and approval belong to
  the deploy layer (ADR-0015: node identity is not user identity); *who may use
  which node* belongs to the application layer (gravity's realms/instance keys
  plus prism's auth gate).
- **Open:** prism discards `Frame::Register.carriers` today (`_(carriers)` in
  `crates/prism/src/effectors.rs`), so the language↔node fact has no home yet —
  either declare it in the node manifest or wire the carrier set into
  registration and routing.

## Related records

aura ADR-0025/0031 (tier-1 remote execution — the mount this builds on),
aura ADR-0027 (content-addressed code delivery; untouched), aura ADR-0015 (node
identity and the approval surface), aura ADR-0044 (no in-process booth),
krystallizer ADR-0007 (HTTP is the single cross-process form),
`docs/PLAN.md` (sequencing: Phase 4.6, Phase 6).