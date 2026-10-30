# 0007 — The model configuration is gravity's own, and it lives in the field

> **Languages:** [English](0007-model-configuration-is-gravitys.md) (primary) · [中文](0007-model-configuration-is-gravitys.zh-CN.md)

**Status:** Accepted (2026-10-30). A ruling on ownership and on the place of the facts; the management face and the pool rule are named as owed, not decided here.
**Date:** 2026-10-30

## Context

A turn needs five facts that belong to no request: the memory store's URL, the LLM endpoint, the model name, the name of the env var holding the key, and the scope k10r's memory doors are addressed in. Today prism assembles all of them (`crates/prism/src/turns.rs`'s `TurnConfig::from_env`) and hands them to the booth as call arguments.

That is the wrong repository. Prism is a generic gateway: it authenticates a connection, parses what the page sent, and puts an event on the plane. Which model answers, at which endpoint, with which credential NAME, is not a gateway fact — it is the executor's own configuration, and a gateway that holds it turns "change the model" into "redeploy the gateway" and puts a second component's business vocabulary inside the component that is supposed to know nothing about it.

So the facts move to gravity. But a fact needs a place, and gravity has none: the turn guest declares no storage. The mechanism is already there — `Engine::register` introspects a script booth, persists the definition (schema included) and allocates the type's ns on the way, and the realm builds that type's `ctx.store` plan from the persisted copy — but nothing in this tree has used it: the shape is exercised by the steel-carrier fixtures, and no induction guest has declared a `storage` block yet. A Rust-body booth can never have one (a compiled handler is not persistable), which is why the test collectors print `no storage ns`. The field is where it belongs, and the field is aura: the realm's embedded store, reached from a guest through the remote-store arm that is already on the ctx (`PutDocument` / `GetDocument` / `PutFields` / `GetFields` / `Scan` / `DeleteDocument`), with rows that outlive every process and are one read away inside the turn.

Custody is unchanged and it constrains the shape: the KEY VALUE never leaves the process environment. A row may name the env var (`key_env`); it may never carry the key.

## Decision

**RULING — gravity owns its model configuration, and it is stored as rows in the field (aura, through the remote store), managed through an interface gravity provides. Prism passes only what the page knows.**

1. **Prism keeps no model vocabulary.** No endpoint, no model name, no `key_env`, no memory scope: prism's turn implementation is deleted, and a page's submit carries what the page can know — the conversation it is in, the text, and the output shape it wants back. A deployment fact that only the executor reads belongs in the executor.

2. **The facts are rows in gravity's own collection, read by the turn at its start.** One settings document (store URL, memory scope, the default model) plus a model table addressed by alias. The read is local and cheap — the store is the engine's, not a network hop — and it happens per turn because a turn is where the choice is applied.

3. **Every model row is a full recipe, so a pool is the same table.** `{alias, endpoint, model, key_env, weight, enabled, affinity}`: one row is the simple deployment (alias `default`, weight 1), many rows are the pooled one. The affinity key is the SESSION: one conversation stays on one model, because that is the unit a prompt cache is built around. How a pool picks among eligible rows (weight, round-robin, ties) is a later mode and NOT fixed here; its rows are, because a pool that lives somewhere else becomes a second source of truth the moment it exists.

4. **The management face is a VIEW, like everything else the application shows.** gravity answers `view:gravity-model` by emitting frames that expand a model-management layout; the buttons inside it emit ordinary events, gravity performs them, and the layout re-draws. This is the stack's general interaction pattern rather than a gravity invention: the venue pushes a layout, a page's uplink and downlink carry NO prefix (prism adds `view:` on the way in and strips it on the way out — prism ADR-0017), every action is an event, and an AI can drive the same events with no second surface — the pattern itself is modelled in prism `docs/MODELING.md`. It is deliberately NOT an admin HTTP verb, a config file, or an environment variable: one home, and it is a home the user can see. The same holds for storage itself — several applications on one field each get their own ns, so an application declares its collections once and stops thinking about where rows live.

5. **A fresh deployment has no rows, and that is a named error.** The turn fails loudly, naming the document and the key that is missing, instead of falling back to an environment variable. A fallback is a second home by another name.

6. **Custody is untouched.** Rows carry `key_env` (a name); the value lives in the guest process's environment, read by the provider guest through that name. No configuration surface — stored, argued, or served — ever carries the key.

## Considered and rejected

- **Prism injects the facts into the envelope when it routes the event** (the version this record replaces). It keeps prism's code thin in line count and wrong in kind: a gateway that carries the executor's endpoint is a gateway redeployed whenever a model changes, and the injection point is the one place where nobody would look for it.
- **Environment variables only.** No interface for the user, no pool, no per-deployment override without a redeploy, and no way to answer "which model is answering right now" other than reading a process's environment.
- **A configuration file inside gravity's deployment.** A file and a store disagree the first time either is edited; the store is already the mechanism the booth reads and writes as a citizen.
- **k10r as the home.** k10r is the memory substrate — the session log, the fact graph, tasks. A model recipe is executor deployment state: it is read before a turn has any memory to consult, it is not recalled by similarity, and putting it behind the HTTP hop would make the turn's own start depend on a service the turn is about to call anyway.
- **Letting the page choose the model.** The page is a renderer; a model choice is a capability and a cost decision. If a user must choose per conversation, that is a row plus an affinity rule, not a field in an uplink.

## Consequences

- **Prism's `turns.rs` goes with the rest of its turn code** (`TurnConfig`, the slot counter, `run_turn`), and the layout's `turn.submit` becomes an ordinary event on the plane. What prism keeps is venue infrastructure: its own node manifest (`mounts`) and the plane itself.
- **The turn's args shrink to what the page knows** — conversation, text, output shape. Deployment facts stop travelling per call, so a caller can no longer forget one and get a different behaviour for the same input.
- **Gravity's booth declares `.storage(...)` beside its `.auth(...)`, and that is the whole registration work**: `Engine::register` introspects the guest, persists the schema (allocating the type's ns on the way), and the realm builds the plan from the persisted copy — so the plan also survives a restart. What is UNPROVEN is the induction carrier's half: no induction guest in this tree has declared storage yet (the path is exercised by steel-carrier fixtures), so the first implementation proves it rather than assuming it.
- **A pool becomes a table edit, not a deploy.** Weight and affinity are data; the selection rule is code, and it is gravity's.
- **The CLI reads the DATA protocol, and the two modes are the caller's choice.** `protocol: fluxen` draws (frames, slots the caller names); a delta consumer names the event its chunks are spoken as, and the venue strips the `view:` marker on the way down (prism ADR-0017), so the CLI sees the plain names it already speaks. Neither mode is a second implementation: one turn, two renderings.
- **The turn addresses a DEVICE, never a channel.** The transport knows devices and users (prism ADR-0018); a channel is an application's business, and this application has none — no groups, and two users do not talk to each other. A turn answers the device that asked, and a booth's own emit names no `sender`, because the venue drops a sender from its own event's fan-out.
- **Open:** the pool's selection rule (weights, ties), and which facts beyond the model triple belong in the same document — `store_url` and `memory_scope` are deployment facts of the executor too, while `mounts` describes prism's own nodes and legitimately stays on the args.

## Related records

ADR-0001 (the execution surface — the mount and the script as an argument), ADR-0002 (the client form), ADR-0004 (the fixed tool list), gravity PLAN Phase 4.1's `NO PULL FACE` note (the turn is an event handler that draws, which is why a turn's facts can no longer ride a request a puller assembles). prism `docs/MODELING.md` (the general pattern: an application IS a view, administration included — this record is its first worked instance). prism ADR-0017/0018 (the wire; devices, not channels). aura ADR-0026 (a type's storage plan comes from its persisted declaration) and aura ADR-0031/0034 (a booth's own code owns its access; the ctx is the cross-booth surface). k10r's `docs/SEAM.md` for what a turn reads from the memory store once it has the URL.