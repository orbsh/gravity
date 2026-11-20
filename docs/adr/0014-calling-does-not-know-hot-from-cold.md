# 0014 — hot and cold are the executor's, and the call never learns which

Status: Proposed (2026-11-20). Reviewed against the code: the tier this record
describes ALREADY WORKS, and the only thing that lands with the record is a
correction to PLAN 4.5, which said the opposite.

## Context

Hot and cold are about the RETENTION OF THE EXECUTOR — whether the code that will
run is already loaded in a live runtime — and not about whether a machine is
dialled in. The distinction decides who owns the machinery, so it is stated with
the evidence:

- **The tier exists twice, as one rule.** `ExecutorSet` in effector's runtime (a
  node) and `Realm.executors` in aura (the bodies that run inside the venue's own
  process) are the same object with the same semantics, keyed by
  `{booth_type}/{instance key}` (aura ADR-0042 for the instance half). A node
  prefixes its keys with `effector/{node_alias}/` for readability only: its
  registry is process-local and the alias disambiguates nothing it cannot see.
- **Cold means one thing: the key is absent from the pool.** The call path takes
  the slot lock, spawns and loads if the key is missing, then runs; a second call
  for the same key finds a live executor and reuses it, runtime state included
  (module globals, a child's memory). There is no other state — no "warming", no
  "draining".
- **Residency dies with the process.** An executor's liveness is its child's own;
  `sweep_dead` evicts an entry whose child exited, so the next call cold-starts.
  No plane-side table records residency, and none could stay right across a
  restart.
- **The key must be stable across calls**, and the wire says so in the field's own
  words: `executor` "cannot be the per-call id: the unit must be stable across
  calls, or every call would start a cold one".
- **A tool call reaches the tier, keyed by the conversation.** gravity hands
  `world.session` where the key goes (`doors.mount(&mount, world.session, …)` →
  `ctx.invoke(type, key, "execute", args)` → `{mount}/{session}`), so one
  conversation's calls share one slot and two conversations never share module
  state. What stays in that slot is the CARRIER's business (§5): the
  registry is reached, and a one-shot carrier still spawns a process per call.

PLAN 4.5 recorded the opposite — "the tier exists in aura; nothing reaches it from
a tool call" — and that line is stale rather than wrong when written: the key
existed before anything passed a session through it.

## Decision

1. **Hot and cold are the EXECUTOR's residency, in two states.** The key is in the
   pool or it is not. A node that is not dialled in is a THIRD thing and is said
   that way (`effector 'x' not connected`). A report that calls a missing machine a
   cold executor, or the reverse, sends whoever reads it to the wrong machine.

2. **Two hands, one rule.** The REALM declares the identity — the key, hence which
   calls share an executor — and the EFFECTOR checks and starts. The check belongs
   to the process that holds the child, because the pool IS that process's
   knowledge; the identity belongs one level up, because sharing is a semantic
   decision about the caller's work, not a property of the machine.

3. **One await.** The first call for a key pays spawn and load inside its own call.
   The caller receives a value or a failure. "Cold" is never a return value and
   "pending" is not in the vocabulary: a caller that can act on "pending" is a
   scheduler, and the caller here is a model in a tool loop.

4. **A start's failure is named as a start.** Today a load that cannot happen
   surfaces as the call's own error — `language not resident-carried by this
   effector build: python` reads like a bug in someone's script when it is a fact
   about the node's build or its spawn spec. The fix is the error's SUBJECT, not a
   new state: the message says the executor could not be brought up and what the
   node did about it.

5. **The key decides SHARING; the carrier decides whether there is anything to
   share.** `{mount}/{session}` says: a conversation's executor is its own, warm
   inside the conversation, never shared across people. A per-call key would make
   every call cold and discard state a script legitimately builds; a key that
   ignored the session would let one person's module globals answer another's
   call. No caller chooses a key, and there is no knob.

   A resident carrier (`induction`) keeps one child per key and state rides it. A
   one-shot carrier (`exec`) is one process per call BY DECLARATION (effector
   ADR-0035): the registry holds a slot, the slot's executor starts and exits, and
   every call is cold in the only sense that carrier has. That is a choice written
   into the mount rather than an accident — nushell cannot evaluate a delivered
   script in a resident process (no `eval`, and `source` wants a parse-time
   constant), so a nu runner is one-shot by necessity. A warm leg needs both a
   carrier that keeps its process AND a language that can run a delivered row inside
   it: python has it (`exec` into a namespace the guest builds), and effector's guest
   runner is the artifact that uses it.

6. **Residency is not tracked on the plane.** A plane-side table of "which
   executors are warm" would drift from the children it describes, and the first
   call after a node restart is a cold call by construction — correct with no
   bookkeeping at all.

7. **Eviction is the node's, and only on death.** A child that exited is not a
   cache entry (`sweep_dead`), and the next call starts a new one. Nothing is
   evicted for being idle: the idle grace is the node PROCESS's lifetime rule
   (effector ADR-0003 §11), not a residency policy — a distinction that matters
   because a node leaving takes every executor with it.

## Out of scope, and why the words collide

- **Bringing up a machine that is not dialled in** — a container that has not
  started — is a different question that also gets called "cold". It is excluded
  here for a structural reason, not a scheduling one: a call that could build
  machines would put launcher power in the caller, and that power was deliberately
  kept out of the component that runs untrusted code (effector ADR-0003 §10).
  Whether a venue, a cluster job or a systemd unit starts a container is a
  question about MACHINES and gets its own record.
- **Which node serves a call** — labels, load, late binding — is deferred by aura
  ADR-0053 §7 and untouched here: this record says a call does not learn whether
  its executor was warm, not which of several machines is chosen.

## What ends the entry (fixed here, built later)

A call whose START outlives the entry that made it — the load takes longer than the
turn's budget — is identified by the MINIMAL REPLAY SET: `(target, key, entry,
digest of args, digest of code, deadline)`. That is what a re-entry needs to
re-establish the call's identity; not the caller's whole state, which is the
caller's row's business (gravity ADR-0011: a run's steps are rows). The result then
arrives as an ordinary dispatch to the calling instance, carrying that identity.

Nothing needs this yet, and that is the trigger: a turn blocks inside one entry by
design (it answers a person), and a run's steps are already rows. The replay set is
written down now because it is a wire fact — a field added later leaves two shapes
in flight, and an args digest that never travelled with the call cannot be
reconstructed afterwards.

## Rejected

- **A `pending` return, or a `cold` verb beside `invoke`.** A scheduling state in a
  value-shaped caller's hands (§3), and two verbs for one operation means every
  call site picks a tier it cannot see.
- **A plane-side residency table.** It drifts (§6), and it turns the plane's memory
  of a node's children into a second source of truth about processes it does not
  own.
- **A per-call key**, for the reason the wire already gives: every call would be
  cold.
- **A key that ignores the session**, to save a few spawns: module state would
  cross between people's conversations.
- **Reporting "not connected" as cold.** A cold executor needs a call to come up; an
  absent machine needs an operator. Same word, opposite instructions.
- **Letting the caller start anything.** See "Out of scope": that is launcher power.

## Consequences

- **The tier becomes testable in one place**, and the shape of the tests is the
  contract: one spawn for two calls of one key, two spawns for two keys, a dead
  child evicted so the next call starts, and a start's error naming the start.
- **The node's error vocabulary gains a subject.** No status enum, no retry logic —
  one sentence that says whose failure it is.
- **gravity gains a pinned reason** for handing the session where the key goes, so
  that "optimization" cannot quietly turn a warm conversation cold.
- **Both shapes now exist and both are proven, in one script.** gravity's `nu`
  runner is the one-shot `exec` form (a process per call — the smoke asserts the
  node holds no child once the calls are done), and its python mount is the RESIDENT
  form: effector's `deploy/runners/runner_py_guest.py`, a guest the node's
  `induction` carrier keeps alive across calls, where a delivered row reads `ARGS`,
  keeps what it wants in `STATE`, and prints one JSON document. The smoke's resident
  leg calls ONE python row twice inside one turn, requires the second answer to be
  the first plus one, and requires the guest to still be ONE live child of the node —
  the inside and the outside of the same fact.
- **PLAN 4.5 is corrected**: the tier is reached, and what remains open is the
  replay set's mechanism, not the residency rule.

## Tests

Planned, not written:

- effector — `one_spawn_serves_two_calls_of_one_key` and
  `a_dead_child_is_evicted_and_the_next_call_starts_a_new_one`, counting spawns
  through the runner the smoke already uses (`runner_nu`); a third test pins the
  wording of a start failure.
- aura — one test naming the key a mount call produces (`{type}/{key}` of the
  instance), so a refactor that changes what is shared fails at home.
- `scripts/smoke-venue.sh` (`SMOKE_PY=1` for the resident leg) — the one-shot half:
  after the CLI's turn and the page's two, the node holds NO child, because the `nu`
  mount declares `exec`. The resident half: one python row called twice, the second
  answer the first plus one, the row in the graph, and exactly one live guest child —
  warm state through a real tool call, on a real node.

Written (effector `crates/runtime/tests/exec_carrier.rs`, on real children, pids
read from inside the guest):

- `one_key_is_one_child_and_a_second_key_is_a_second_child`
- `a_dead_child_is_evicted_and_the_next_call_starts_a_new_one`
- `a_start_failure_names_the_start_not_the_code`

The resident leg is asserted from BOTH sides in the smoke (`SMOKE_PY=1`): the answers
must be consecutive, and the guest must still be one child of the node afterwards.

## Related records

effector ADR-0003 §10–11 (the node is the container's root process; the launcher's
four facts; the idle grace is the process's own lifetime), effector ADR-0035 (the
pipe protocol and the resident child), ADR-0036 (one envelope for a dispatch), aura
ADR-0042 (the instance-key format the executor key is built from), aura ADR-0053 §7
(which node — deferred), gravity ADR-0001 (the mount is the execution surface, and
`execute` is the entry convention), gravity ADR-0011 (a run is a log), `docs/PLAN.md`
2.5 and 4.5.