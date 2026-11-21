# 0015 — a context is the unit of work somebody can watch

Status: Proposed (2026-11-21). The mechanism this record describes is landed: the CLI's
`--tag`, the booth's declared-agent rule, `find_member`, `supervise`, and a smoke leg that runs
two invocations of one context and shows the second one the first. What the record adds is the
GENERAL statement — that a background task and a parallel batch are the same object, opened by a
different hand.

## Context

The CLI is gravity's own door, not the venue's (ADR-0002): it reaches the agent through prism
because prism is the only wire, but what it drives is the agent. That made one question
unavoidable, and the answer decides the shape of everything else:

**Who is speaking when work was started by a person but is not that person?**

The first answer was the obvious one — the CLI speaks as whoever logged in, because the venue
stamps `sender.user` on every page event and k10r's member id IS the user's uuid (k10r ADR-0025).
It is also wrong in the way that matters: a script that runs nightly would put its rows on the
person's own line, mixed with what they said themselves, and a person reading their own channel
would find a machine talking as them. Worse, the same failure repeats for every other source of
work — a trigger that fires on an event, a batch of parallel jobs — each of which would either
borrow a person's identity or have none at all.

## Decision

**A context is the unit of agent work: a name, an agent member derived from it, one channel, one
turn per invocation.**

1. **The name IS the identity.** `--tag "drift check"` derives a member id from the name
   (FNV-1a twice, uuid-shaped) and declares it to the turn along with `member_name`. The same name
   is therefore the same speaker on every run, and — because a member's rows in a channel are its
   conversation — the same name in the same channel is the same CONTEXT. Two names are two agents.
   No file keeps them: the identity is a function of the name, so there is nothing to lose or
   forget.

2. **A process may be an agent; it may never be a person.** The booth's rule, in order: a venue
   stamp with no declared identity uses the stamp; a declared identity that k10r reports as AGENT
   (role 1) overrides the stamp; a declared identity that is a PERSON does not (the stamp wins —
   a client cannot claim an identity it cannot prove); a declared identity the store has never
   heard of is registered as an agent under the `member_name` it came with; unknown and unnamed is
   the caller's own fallback id, so the stamp wins there too. No stamp and no identity is a
   refusal: there is no such speaker, and a row that names nobody is a lie.
   **A person's member row is BORN NAMED, and RE-NAMED on the next turn.** When the speaker is the
   venue's own stamp, the venue has also said what that account is called (`sender.display_name`,
   prism ADR-0017), and the booth states it every turn — the store keeps the first `since` and takes
   the newer name (k10r ADR-0009 §1), which is how a rename in the account reaches a transcript that
   was written before it. The booth never looks a name up: the registry is the venue's, and a copy
   kept here would go stale exactly when it matters. An agent's name is its tag, restated the same
   way. A name this side does NOT have is stated by omission — "nobody is renaming me" — which the
   store must not confuse with `""`, "nobody has named this member". The role is stated the same
   time and it is not decoration: k10r withholds the transcript from an agent (its `view::assemble`),
   so stating a person's row as an agent empties the very projection the turn is built from.

3. **The store decides what a turn is shown, and a program is shown no transcript.**
   `GET …/members/{m}/session` is where the decision lives. For a PERSON the projection carries the
   record — head, summary, increment. For an AGENT (`member_role::AGENT`) it carries the head and
   nothing else, and its rows stay in the log as the supervision record a person opens. The
   asymmetry is about INSTRUCTION: a person issues instructions that change the context in ways
   nobody can predict, so the model needs part of the conversation to follow them; a program runs a
   known shape, and what it must remember it commits to memory or opens as a task — the same door a
   person's turn uses.

   This is deliberately NOT decided at the call site. A caller that assembled its own prompt would
   put the knowledge policy in every call site — gravity, a page, a future driver — each with its
   own opinion about what a model should see, and the reason this store exists (turning what a
   deployment learned into knowledge it can hand out) would have to be reimplemented beside every
   call. So: the caller renders what the projection gives it, in the order it gives it. Nothing is
   baked into a prompt anywhere: which facts, which skills, how much of the record, and which open
   tasks reach a turn remain the store's decision, made per read. `owed` — the "work this memory
   still holds" block — moved here from the booth for exactly this reason, and k10r ADR-0021 says
   so in its own words.

4. **A person's line is named, not guessed.** The engine used to take the first channel a member
   was in, which was true while a member had one channel and wrong the moment an agent could add a
   person to the channel it works in — a supervisor's own words would land in a machine's line. k10r
   records which channel is somebody's own (`solo`, k10r ADR-0025) and the engine asks for it by
   name; the id derived from the member is the OPENING id of a first use, never the way back to a
   line that already exists.

   Which means WHO SPEAKS is decided BEFORE which channel: the line belongs to the speaker, and the
   speaker is not the person who logged in. A context run from a page's session arrives stamped as
   the operator and declares another identity; resolving the channel from the stamp first put the
   context's rows in the operator's own line — the two-narrators-one-transcript failure, arrived at
   by ordering rather than by design. The CLI therefore pins no channel by default (`--session` is
   for a caller that wants one) and a person's first message carries no id at all.

5. **Every source of work has this shape.** A user's instruction that spawns a background task, a
   trigger that fires on an event, one item of a parallel batch: each is a context, each speaks as
   its own agent, and each is watched the same way. What differs is who opens the turn — a person
   typing, a scheduler, a fan-out — and that is the only thing that differs. The opener is not a
   property of the line.

6. **Watching is data, not a flag.** k10r's rows name their sender (k10r ADR-0009 §1), so a mixed
   line is readable; the channel's memberships are the visibility set. The agent decides who
   supervises — `find_member` asks the directory by name, `supervise` joins the people it chose —
   because the agent is the one that knows what the work is for and who could act on it. There can
   be several supervisors or none: work an agent does for itself has nobody to report to. The
   client carries no such flag, and the abandoned `--supervisor` parameter is the evidence that a
   client-side list was the wrong shape.

7. **Parallel work is one channel per context.** Two contexts sharing one channel would each read
   the other's rows: two narrators, one transcript, and a prompt that mixes two jobs. The line is
   the unit of isolation, and "watch it" means "open its channel".

## Consequences

- A conversation with a person and a background job are the same object with a different opener —
  one implementation, one set of doors (`append` with a sender, the directory, memberships).
- Supervision lives in the store as memberships, so it survives the process that chose it and is
  visible to every reader rather than to one client.
- The venue keeps its authority over people: nothing here lets a caller speak as somebody else,
  which is the property a token mechanism (PLAN, open items) must preserve.
- What is NOT here: a name registry, an ownership record on the context, and any notion of a
  context being "done". A context is a line, not a job; tasks stay k10r's rows in the deployment's
  scope (ADR-0003 §3).
- A context has no continuity of CONVERSATION, only of memory — and that is a property of the
  projection, not of each caller. Two runs of the same name are the same speaker in the same
  channel; the second is not shown the first, and if it must remember, it commits a fact or opens
  a task. Reading the rows back is a thing people do.