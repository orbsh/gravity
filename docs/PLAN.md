# PLAN

Design lives in the wiki (`stateless-agent-architecture.md`, `krystallizer.md`) and in this repo's ADRs (`docs/adr/`: 0001 the execution surface, 0002 the CLI form, 0003 the skill execution shape, 0004 the fixed tool list, 0005–0012 as each landed). This file is the index: what landed when, what is left, and where the reason is written.

## Milestone A — the local turn booth

- [x] 0 — Workspace skeleton, one happy-path turn; the seam's currency is the projection — 2026-10-10 … 2026-10-20 — ADR-0007
- [x] 1 — The LLM layer: one blocking SSE reader in the turn's own process, no provider booth — 2026-10-13 — `gravity-core llm::sse`, `llm::tool_stream`
- [x] 2 — The tool-call loop against a fixed tool list; a tool failure is a value, never a turn failure — 2026-10-26 — ADR-0004
- [x] 2.5 — Transcript/history split: the net effect as rows, one per round, failures stay process — 2026-10-31 — k10r ADR-0024
  - [ ] the cold CALL's result path — owned by 4.5
- [x] 2.6 — Progress as data: a task row per step, completed at the call site — 2026-10-31 — ADR-0003, ADR-0011
- [ ] 3 — Compression as a side branch — ADR-0003
  - [x] the turn-loop half: the branch opens off k10r's verdict and lands at the checkpoint door — 2026-10-26
  - [ ] the standalone trigger: no idle/no-turn tick exists anywhere in the tree — the protocol half exists (`induction_guest::Ctx::timer_register(at_ms, tag)` / `timer_cancel`), and nothing in gravity registers one, so a compression that no turn caused cannot start
  - [ ] the branch's owner after the turn returns — `drain_tails` waits INLINE up to 60s, so a slow summary holds the turn open and a longer one is abandoned as "outlived the turn". Its stated precondition is met and stale: induction ADR-0002's steps 1–3 landed 2026-11-02 (one route per request, replies paired by id, `host.call` naming the request), so one booth CAN serve two requests. What is missing is a PULLER that outlives the entry — the turn guest is resident (`BoothType::script(TURN_TYPE, "induction", …)`, one child per key), so the drain can be handed to a thread in the same process

## Milestone B — distributed execution

- [x] 4 — The booth binding: one turn event, one execution — 2026-10-10
- [x] 4.1 — The guest re-landed on induction: pure Rust, no embedded interpreter — 2026-10-13
  - [x] no pull face: a turn is one handler call — 2026-10-30
  - [x] the deployment facts move into the field (`settings` / `models` rows) and the management view — 2026-10-30, screen 2026-10-31 — ADR-0007
  - [x] two renderings (`protocol: data | fluxen`); an event-routed turn says its own end — 2026-10-29 … 2026-10-31 — ADR-0009
  - [x] the speaker comes from the venue's envelope, never from a form — 2026-11-04 … 2026-11-05
  - [x] a row NAMES its speaker (k10r `sender`), and a process may be an AGENT while it may never be a person — 2026-11-21, ADR-0015 §2
  - [x] a context is the unit of work somebody can watch: `--tag` derives the agent from the name, one invocation is one turn and the channel is the history, the agent picks its supervisors (`find_member` + `supervise`) — 2026-11-21, ADR-0015
  - [x] the CLI pins no channel by default: `--session` is optional and absent means "the speaker's own line", so a first message carries no id — and WHO SPEAKS is decided before WHICH CHANNEL, or a context's rows land in the operator's line (`a_context_speaks_in_its_own_line_not_in_the_operators`) — 2026-11-21
  - [x] a person's line is NAMED, not guessed: k10r records `solo` on the channel row and the engine asks `?app=…&solo=1` instead of taking the first channel a member is in (a supervisor's words used to land in the machine's line) — `a_members_own_line_is_the_solo_one_not_the_first_in_the_list` (k10r) and `a_persons_turn_lands_in_their_own_line_not_the_first_channel_they_are_in` — 2026-11-21
  - [x] the tasks block moves into the projection (`session.owed`, k10r ADR-0021): the booth renders what the store decided and keeps only the code-row→task bindings it needs to CLOSE work (behaviour, not prompt text) — 2026-11-21
  - [x] a context's turn carries NO TRANSCRIPT, and the STORE decides that: k10r's projection withholds the increment for an AGENT reader and hands it to a person (their instructions move the context, so they need the record), so no caller assembles a prompt of its own — `a_person_reads_the_conversation_and_an_agent_reads_none_of_it` (k10r), and gravity's `a_context_turn_is_not_given_the_history_a_person_turn_is` end to end — 2026-11-21, ADR-0015 §3
  - [x] the shell declares the platform's `auth` region; auth rides the route declaration — 2026-11-06
- [ ] 4.5 — The unified call model: hot and cold behind one await — ADR-0014 (Proposed; the tier itself already works, the record pins who owns what)
  - [x] 冷热 = the EXECUTOR's retention, not the machine's presence — the key is in the pool or it is not; "not connected" is a third thing, said separately (ADR-0014 §1)
  - [x] the split: the realm declares the identity (`{type}/{key}` of the instance), the effector checks and starts under the slot lock; the caller declares and learns nothing (§2)
  - [x] what a VALUE-shaped caller receives when the target is cold — a value or a failure; the load rides the same await, never a scheduling state (§3)
  - [x] a start's failure named as a start — effector `7b66372`: a spawn or load that fails now says which executor it failed to start (`executor {key} could not be started (carrier {language}): …`), and residency is a test over three real subprocesses. The `language not resident-carried by this effector build` sentence is what an unknown carrier answers, which is a build fact, not a script bug
  - [x] the tests that make the contract hold — `one_key_is_one_child_and_a_second_key_is_a_second_child`, `a_dead_child_is_evicted_and_the_next_call_starts_a_new_one`, `a_start_failure_names_the_start_not_the_code` (effector `crates/runtime/tests/exec_carrier.rs`, real children, pids read from inside the guest)
  - [x] RESIDENCY IS THE CARRIER'S (§5) — `exec` is one process per call by declaration; a warm call needs a carrier that keeps its process AND a language that can run a delivered row inside it, which is python (nu cannot: no eval). The artifact existed (`effector/deploy/runners/runner_py_guest.py`) and NO MOUNT USED IT; the smoke's `SMOKE_PY=1` declares an `induction` python mount, calls one row twice in a turn, requires consecutive answers and requires the guest still to be one live child — warm state through a real tool call, 2026-11-20. The one-shot half stays pinned too (no child after the `nu` calls)
  - [ ] what the caller writes down when it ends its entry — the minimal replay set is FIXED (`target, key, entry, args digest, code digest, deadline`), the mechanism waits for a caller that must end its entry mid-call (ADR-0014, "What ends the entry")
  - [ ] re-entry: the result arrives as an ordinary dispatch carrying the minimal replay set — same deferral
- [x] 4.6 — The execution surface is the runner mount; `start`/`advance`/`run`, `write_code` — 2026-10-26 … 2026-11-09 — ADR-0001, ADR-0004, ADR-0011
  - [x] the whole stack, with a real node: `scripts/smoke-venue.sh` runs a code row on an `effector` node through the venue (stub provider → gravity → prism → mount → `runner_nu` → JSON back) — 2026-11-20
- [x] 5 — Skill pull-through and the weight write-back — ADR-0003 §5, ADR-0004 — closed 2026-11-21
  - [x] the counters at the call: attempt before a node runs anything, success after, on both rows — 2026-10-26
  - [x] the proposal write side: a walk that grew files the composition it walked — 2026-10-26
  - [x] the task tools, `write_code` / `read_code` — 2026-10-27 … 2026-10-31
  - [x] PULL-THROUGH, restated in the terms that now exist: a skill is a k10r row, its steps name code rows, and both are fetched AT THE CALL (`turn.rs`'s one GET for the step) — the booth holds no copy, so a revised script is what runs next time. `resolve_skills` answers the candidates, `start` runs one as a run, `run` runs a row no skill names (ADR-0011)
  - [x] THE WEIGHT WRITE-BACK, as the counter it was ruled to be (ADR-0003 §5): `tools.rs` touches `skills/{scope}/{id}/touch` and `code/{scope}/{id}/touch` with `attempt` around the call and `success` after it — numbers on existing rows, never topology. The counters are asserted in `tools.rs`'s own tests
  - NOTE the door this does NOT cover: k10r's fact weight (`POST /memories/{scope}/edges/{key}/touch`, k10r ADR-0005's `tool_invoke_count`) has no production caller anywhere in `~/world`, and a turn cannot honestly report which fact it used — `recall_memory` answers with rendered text, so the model's use of one fact is not observable to the process that asked. The door waits for a consumer that shows a fact structurally (a UI, a dossier); until then the index never moves and a plain read stays its equal
- [x] 6 — The CLI is a client of the venue, kubectl-shaped, no second driver — 2026-10-26 — ADR-0002

## Open items

- [ ] A duplicated patch frame: one patch delivered N times per page — seen 2026-11-05 in a six-connection probe, re-measured 2026-11-20 in a clean venue with ONE tab (`SMOKE_PAGE=1 scripts/smoke-venue.sh`): turn 1 draws 2 deltas + `opened` + `done`, turn 2 draws 2 deltas + `done`, nothing twice and nothing carried over. prism's fan-out is one delivery per `/channel` CONNECTION (pinned by `ui_leg::one_emit_draws_exactly_one_frame_however_much_was_said_before`), so a frame that arrives N times means N connections for one screen — the page's connection management (reflux), not the venue. Parked here until a page can be driven for real
- [ ] `scripts/smoke-venue.sh` is not a gate: it passes end to end (provider, k10r, prism, a real effector node, a code row run on that node) but no workflow references it, and this repo has no CI to reference it from
- [x] ADR-0007's model row promised `affinity` with no such field — settled the other way (2026-11-20): the affinity key is the SESSION, so it is not a column, and the ADR no longer lists it
- [x] Cancellation is a marker the turn polls at its chunk boundary — 2026-11-09 — ADR-0008, aura ADR-0047, effector's `Frame::Interrupt`
- [x] `run` for a script that belongs to no skill — 2026-11-09
- [x] The CLI's credential for an unattended caller is a TOKEN (`venue.token_env`) — 2026-11-21, ADR-0002: the venue mints one from the account's password (`POST /admin/tokens`, login over HTTP) and the client presents it on the dial with no login frame, so a loop logs in nowhere. A config names `token_env` or `username` + `password_env`; neither is refused at first sight, and a token the venue does not know is an auth failure (exit 3), not a turn run as nobody

## Deferred

- A FaaS driver — after the booth path is stable; another driver of the same function, not new code
- Multi-model routing — after the single-model path is proven; LLM identity stays gravity's, k10r never learns it