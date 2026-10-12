//! gravity-booth — the aura booth binding (PLAN Phase 4): the turn
//! execution as a Booth type, streaming end to end.
//!
//! One python booth (`turn` handler, generator): fetch the session
//! view from mem-server (HTTP — the booth's own code decides its
//! access method, ADR-0031), assemble the prompt, pull tokens from the
//! provider booth through `ctx_iterate` (ADR-0034's native pull loop —
//! the provider is a sibling citizen, not gravity-internal plumbing),
//! `yield {"delta": ...}` per token, then append the one-ts-two-subs
//! exchange and return. The instance key IS the session (partition =
//! session_id per aura PLAN Phase 6's contract).
//!
//! Streaming note: the whole turn is the generator — every token
//! crosses as an envelope item while the stream is live, the terminal
//! value carries the full reply. prism's `turn.delta` contract rides
//! this unchanged (the stub this booth replaces had the same shape).
//!
//! Custody: the LLM key is read from the env var NAMED in the args
//! (the value never in the module or args, rotation without reload);
//! the store URL rides the args the same way (a deployment fact the
//! caller supplies per call — the booth holds no connection state).
//!
//! Naming note (ADR-0043): the iterate face is called through the
//! current injected wrapper `ctx_iterate`; ADR-0043 renames it to
//! `ctx.iterate` with no compatibility layer — this source follows at
//! that landing (one call site, mechanical rename).

/// The turn booth's type name.
pub const TURN_TYPE: &str = "gravity_turn";

/// The provider booth's type name (the `ctx_iterate` target).
pub const PROVIDER_TYPE: &str = "llm_provider";

/// The handler the turn generator invokes on the provider.
pub const PROVIDER_HANDLER: &str = "stream";

/// The provider booth source: `stream(args)` yields `{"delta": "..."}`
/// per token event. SSE parsing is inline (httpx's line iterator — no
/// extra dependency); errors raise (the pull fails as a value).
///
/// Args: `{"endpoint", "model", "key_env", "messages": [{role,
/// content}, ...]}`. The key is read from the environment PER CALL.
pub const PROVIDER_SOURCE: &str = r#"
import httpx, json, os

def stream(args):
    key = os.environ.get(args["key_env"], "")
    url = args["endpoint"].rstrip("/") + "/chat/completions"
    headers = {"Authorization": f"Bearer {key}"}
    body = {"model": args["model"], "messages": args["messages"], "stream": True}
    with httpx.stream("POST", url, headers=headers, json=body, timeout=120) as r:
        r.raise_for_status()
        for line in r.iter_lines():
            if not line.startswith("data: "):
                continue
            data = line[6:]
            if data.strip() == "[DONE]":
                break
            chunk = json.loads(data)
            delta = chunk["choices"][0].get("delta", {}).get("content")
            if delta:
                yield {"delta": delta}
"#;

/// The turn booth source. `turn(args)` is a generator:
///
/// 1. `GET {store_url}/channels/{c}/members/{m}/session` — the
///    projection (summary + increment), the mem-server wire shape.
/// 2. Prompt assembly: summary preamble (system) + increment (wire
///    roles) + the user message.
/// 3. `ctx_iterate(PROVIDER_TYPE, "", "stream", provider_args)` —
///    each item's `delta` is yielded straight through (the stream IS
///    the turn; nothing buffers).
/// 4. Append user + assistant with one shared `ts_ms`, subs 0/1 —
///    ONLY after a successful stream (a failed turn appends nothing;
///    the user re-sends next turn). `time.time()*1000` from the
///    booth's own clock (gravity's clock IS the channel's time,
///    ADR-0008 §3).
///
/// The provider pull failing raises → the generator dies → the pull
/// that observed it fails as a value → the turn fails through. The
/// python wrapper's `finally` disposes the provider stream (the
/// mandatory dual) — abandonment is governed, not leaked.
///
/// Args: `{"channel_id", "member_id", "user_message", "store_url",
/// "endpoint", "model", "key_env"}`.
pub const TURN_SOURCE: &str = r#"
import httpx, json, time

def turn(args):
    base = args["store_url"].rstrip("/")
    channel_id = args["channel_id"]
    member_id = args["member_id"]
    user_message = args["user_message"]

    # 1. Fetch the session projection (mem-server's SessionDto shape).
    r = httpx.get(
        f"{base}/channels/{channel_id}/members/{member_id}/session", timeout=30
    )
    r.raise_for_status()
    view = r.json()

    # 2. Prompt assembly: summary preamble + increment + user message.
    # Role bytes -> wire strings (the storage layer stores the byte,
    # this function interprets it: 1=assistant, 2=tool, 3=system,
    # else user — mem-core's channel role constants).
    role_wire = {1: "assistant", 2: "tool", 3: "system"}
    messages = []
    if view.get("summary"):
        messages.append({
            "role": "system",
            "content": f"Conversation summary so far:\n{view['summary']}",
        })
    for entry in view.get("increment", []):
        row = entry["row"]
        messages.append({"role": role_wire.get(row["role"], "user"), "content": row["content"]})
    messages.append({"role": "user", "content": user_message})

    # 3. The LLM stream THROUGH the provider booth. Each token yields
    # straight through — the stream IS the turn. Reply text accumulates
    # for the append (step 4) and the terminal value.
    provider_args = {
        "endpoint": args["endpoint"],
        "model": args["model"],
        "key_env": args["key_env"],
        "messages": messages,
    }
    reply_parts = []
    for item in ctx_iterate("llm_provider", "", "stream", provider_args):
        delta = item.get("delta", "")
        reply_parts.append(delta)
        yield {"delta": delta}
    reply = "".join(reply_parts)
    if not reply:
        raise RuntimeError("provider stream yielded no tokens")

    # 4. Append the exchange: one ts, subs 0/1 — only on success.
    ts = int(time.time() * 1000)
    base_append = f"{base}/channels/{channel_id}/messages"
    for sub, role, content in ((0, 0, user_message), (1, 1, reply)):
        r = httpx.post(base_append, json={
            "ts_ms": ts, "sub": sub, "role": role, "kind": 0, "content": content,
        }, timeout=30)
        r.raise_for_status()

def interface_schema(args):
    return {"auth": {"turn": True}}
"#;

/// The provider booth type (python).
pub fn provider_booth() -> aura_booth::BoothType {
    aura_booth::BoothType::script(PROVIDER_TYPE, "python", PROVIDER_SOURCE)
}

/// The turn booth type (python, generator — session-backed, the
/// carrier holds the stream state).
pub fn turn_booth() -> aura_booth::BoothType {
    aura_booth::BoothType::script(TURN_TYPE, "python", TURN_SOURCE)
}
