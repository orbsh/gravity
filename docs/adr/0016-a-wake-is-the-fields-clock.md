# 0016 — A compression nobody's message asked for is a wake the booth asks the field for

> **Languages:** [English](0016-a-wake-is-the-fields-clock.md) (primary) · [中文](0016-a-wake-is-the-fields-clock.zh-CN.md)

**Status:** Accepted (2026-11-21) — landed the same day: the `__on_timer` hook, the tag, and the registration site are in `crates/booth` (see Consequences)
**Date:** 2026-11-21

## Context

Every compression pass this repo runs today is born inside a turn: the streaming loop opens the side branch (`open_tail`) while the answer is being pulled, and the drain lands the product at the checkpoint door (`raise_tails`, 2026-11-21 — the branch now outlives the entry that opened it instead of holding it open for up to sixty seconds).

Two situations have no home in that shape:

1. **The branch never lands.** `raise_tails` hands the branch to a thread in the resident guest, so the turn no longer waits for it — which also means the turn can no longer be the thing that notices it failed. A worker that dies, a provider that answers with an error, or a span the store closed under it leaves k10r with an open compression node: the session answer keeps saying `compressing: true` (`SessionDto`, `mem-server/src/lib.rs`), and nothing in this repo looks again. The next message in that channel reads, sees the open node and may open a new branch; a channel nobody writes to again keeps the span open for good.
2. **A compression whose cause was not a message.** k10r's verdict is computed ON READ — what is owed is decided when somebody asks. A policy instant ("this context has been idle long enough to fold") produces no read, because reads are what turns do, and a turn needs a message.

The mechanism for the second half is not missing, and it does not belong to this repo. It is landed in aura (ADR-0016 there): one `DelayQueue` driver per realm, deliver entries that become an `__on_timer` job — `deliver_timer` sends `{tag}` and nothing else, to the instance the timer was registered against (`realm/src/instance.rs`) — and `ctx.timer.register {at_ms, tag, durable}` in the bridge (`realm/src/ctx.rs`), with durable registrations written as ns 27 rows and the wheel rebuilt from them at boot. The minimal host declines the arm (`induction-host`'s `timer_register` says "mudra has none today"); the field's host supplies it. So what gravity lacks is not a clock. It is a question never asked.

## Decision

### 1. The clock is the field's; this repo only asks

A booth that schedules its own sleep is a booth the field cannot see: the sleep dies with the process, no other component can cancel it, and a venue that restarts leaves the schedule in a thread that no longer exists. Aura's ADR-0016 already rejected one task per timer for the same reason and kept one driver per realm. Gravity registers and cancels; it never sleeps.

### 2. The tag is the booth's own pointer: `compress:<channel>`

The delivery carries `{tag}` and nothing else, and the guest is not told its own instance key — this booth reads no `IND_*` environment and no frame field names it. Whatever a wake must know therefore has to travel in the tag the booth armed. Gravity's tags are `<intent>:<channel>`, and exactly one function parses them; an unparsable tag prints the tag it could not read rather than doing nothing quietly, because a silent mismatch is a wake that fires forever for no reason.

### 3. A wake is a question, not an instruction

The handler re-reads the projection for the channel the tag names and acts only if the store still says `compressing`. The store's answer is the only authority on whether work is owed: the span may have been landed by a turn that arrived after the wake was armed, or by another process. A timer says "look again"; it never says "compress this". This is also what keeps two passes off one span — the `compressing` flag exists for exactly that, and a wake that compresses unconditionally pays a second model call for a summary that is already in flight.

### 4. Durable, because the window outlives residency

A gravity instance is evicted after its idle TTL, and the window a compression wake covers is longer than that by construction (aura ADR-0016 §1 names this case: the cache-clock check). A memory-tier timer on an evicted instance is a schedule that vanished — and the span it was meant to land stays open, which is failure mode 1 again. So the registration is `durable`, and the row it writes is the reason the schedule survives a restart.

`induction_guest::Ctx::timer_register(at_ms, tag, durable)` carries that flag (2026-11-21: the third parameter, built by `timer_args`, with `the_timer_arm_carries_the_durable_ask` pinning the frame's shape). The minimal host still declines the arm — a host that keeps no timer table cannot honour a request to survive a restart, and declining is more honest than a wake nobody can find afterwards.

### 5. Re-arm is explicit and computed from now, and armed one delay after the turn

The wake that finds its span still open either opens the branch or, when the material is not there yet, arms the next wake from the current instant. It never arms a chain in advance and never fires a burst for instants it slept through: aura ADR-0016 §3 already ruled skipping over catch-up, and a burst of compressions after downtime is the exact failure shape the compression policy was written to avoid.

The turn arms one wake, sixty seconds after a projection that asked for a compression (`WAKE_DELAY_MS`) — not one per branch, and not before the answer is written. Sixty seconds is a policy number, not a structural one: long enough that a branch still pulling lands first (a wake that opens a second pass over a span in flight pays twice for one summary), short enough that a branch which died is noticed while the conversation is still recent. A wake that finds the span closed costs one read and stops.

### 6. A wake is maintenance: it writes no exchange row

The wake runs the same two landing doors a turn uses — the checkpoint for the summary, the memory door for facts — and nothing else. It does not write a message row, does not emit into the channel's UI stream, and does not appear in the conversation it maintains. Maintenance that shows up as somebody speaking is a defect in the product, not a detail of the mechanism.

### 7. It enters the booth as a third entry, gated like the others

`__on_timer` is declared beside `turn` and `save` (`.on("__on_timer", …)`) and sits behind the same gate as `turn`, because it spends the same capability: it calls the model and writes to the store. A wake that could not write would be useless; a wake that could write unauthenticated would be a hole of the same size as an unauthenticated turn.

## Honest semantic cost

- **A retried compression is not the text the lost attempt would have produced.** The material is re-derived from the log — the same rows — but the model is not deterministic, so a span landed by a wake may carry a differently worded summary than the branch that died. The log is what is authoritative; the summary is a rendering of it.
- **A wake that finds a closed span is a wasted read.** It costs a projection read and nothing else. Trying to avoid that waste would mean the booth tracking spans itself, which is the store's job and would be stale by definition.
- **The tag is a contract between two runs of this repo.** A format change without a parse change is a wake that fires for a channel nobody can name; hence the loud miss.
- **Durable wakes outlive the deployment that armed them.** A booth whose configuration was replaced while a wake was pending will wake under the new configuration. The wake is a question about the log, so this is correct rather than unfortunate — but it does mean a wake cannot carry a decision.

## Alternatives

- **A sleeping thread in the guest.** Dies with the process, invisible to cancellation, and survives no restart. Aura ADR-0016 rejects the per-timer task for the same three reasons.
- **k10r schedules the wake.** It holds the policy and computes the verdict, but it holds no model: its answer says what is owed, never who can do it. A store that schedules work it cannot perform is a scheduler with a hole in the middle.
- **A venue-side cron (prism).** The pass must run inside the instance whose key is the channel, because the material it folds is that instance's projection. A second scheduler for one booth is two places to look when a span stays open.
- **A wake that compresses unconditionally.** Rejected in §3: it doubles the model cost on any span a turn is already compressing.

## Consequences

- Gravity gains a third entry (`__on_timer`), one tag parser, and one registration site: a turn that ends with `compressing: true` and no landed summary arms a wake for that channel; a wake that opens a branch also heads off a turn that would have opened the same one.
- **Landed with this record**, in `crates/booth`: `.hook("__on_timer", …)` (a HOOK, not `.on(…)` — the name is the host's, and induction refuses it as a subscription), `arm_wake` after a projection that asked to compress, `wake_tag`/`parse_wake_tag` for `<intent>:<channel>:<member>`, `wants_compress` as the only decision, and `branch_material` extracted so the branch's prefix is derived from the projection alone instead of from a turn's own state. A failed arm is logged and never fails the turn it was armed for.
- **A host-delivered name costs four edits, because the vocabulary is duplicated.** `__on_timer` had to be added to the reserved list in induction's guest, aura's booth builder, and effector's python and steel carriers — four copies of one array, each rejecting the name in the same words. The landing made them agree; nothing makes them stay agreed, and that is the shape to fix before the next reserved name is invented.
- Tests: `a_wake_folds_a_span_that_no_turn_is_watching` (a real store, a real clock-delivered `__on_timer`, no turn anywhere — the summary lands and the log keeps only the records it folded), `a_wake_only_acts_on_the_stores_verdict`, `a_wake_tag_names_its_channel_and_member_and_reads_back`, `a_branch_sees_the_head_the_summary_and_the_numbered_records`.
- **The `durable` ask reached the guest** (2026-11-21): `Ctx::timer_register` takes it as a third parameter and the frame carries it, so §4's residual is closed on this side. What is still open is gravity's own wiring, below.
- Tests, when the wiring lands: a stub host that delivers `__on_timer` and asserts the branch lands through the checkpoint door; a wake whose span was closed by another writer, asserting the booth writes nothing; an unparsable tag, asserting the miss is printed rather than swallowed.
