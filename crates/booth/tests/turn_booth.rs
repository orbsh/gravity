//! The booth binding contract (PLAN Phase 4, re-landed on induction):
//! the turn execution as a Rust guest process, the provider as a
//! sibling guest — one realm, the turn producer pulling the provider's
//! stream through the ctx iterate arms and re-emitting each delta as
//! its own envelope item. The LLM wire is stubbed by a local SSE
//! server, the memory seam rides a real mem-server. What is pinned:
//!
//! - the STREAMING round trip: each token arrives as an envelope item
//!   while the stream is live (not one buffered answer), the deltas
//!   assemble the reply, and the memory seam sees the one-ts-two-subs
//!   exchange after the stream ends;
//! - a failed provider pull appends nothing (the turn function's
//!   oldest rule, through the booth form).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unwrap_in_result, clippy::panic_in_result_fn)]

use aura_booth::{call::CallSlot, InstanceId, InstanceKey, IterateOp};
use aura_engine::Engine;
use aura_realm::Realm;
use gravity_booth::TURN_TYPE;
use gravity_core::surface::MemorySurface;

/// The two guest binaries — cargo builds this package's own bins for
/// its integration tests and exports `CARGO_BIN_EXE_<name>`, so the
/// fixtures can never run stale (the induction pair.rs pattern).
fn provider_bin() -> String {
    env!("CARGO_BIN_EXE_gravity-provider").to_string()
}

fn turn_bin() -> String {
    env!("CARGO_BIN_EXE_gravity-turn").to_string()
}

/// A minimal OpenAI-compatible SSE endpoint: answers one streamed
/// reply assembled from `CHUNKS`, then `[DONE]`.
async fn sse_server(body_chunks: Vec<&'static str>) -> String {
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move || {
            let chunks = body_chunks.clone();
            async move {
                let mut sse = String::new();
                for c in chunks {
                    let payload = serde_json::json!({
                        "choices": [{"delta": {"content": c}}]
                    });
                    sse.push_str(&format!("data: {payload}\n\n"));
                }
                sse.push_str("data: [DONE]\n\n");
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// One SSE body assembled from `chunks` (the reply is their concatenation,
/// one chunk per token) and closed by `[DONE]`.
fn sse_body(chunks: &[&'static str]) -> String {
    let mut sse = String::new();
    for c in chunks {
        let payload = serde_json::json!({ "choices": [{ "delta": { "content": c } }] });
        sse.push_str(&format!("data: {payload}\n\n"));
    }
    sse.push_str("data: [DONE]\n\n");
    sse
}

/// An SSE stub that answers the answer and the compression branch
/// DIFFERENTLY, keyed on the request itself: k10r's tail prompt is the last
/// message of the side branch (the turn appends it in place of the user's
/// message), so a request that ends with it asks for the summary and every
/// other request asks for the reply.
async fn sse_server_two_branches(
    reply: Vec<&'static str>,
    summary: Vec<&'static str>,
) -> String {
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move |body: axum::body::Bytes| {
            let reply = reply.clone();
            let summary = summary.clone();
            async move {
                let asked = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| {
                        v.get("messages")
                            .and_then(|m| m.as_array())
                            .and_then(|m| m.last())
                            .and_then(|m| m.get("content"))
                            .and_then(|c| c.as_str())
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                let chunks = if asked.starts_with("Summarize the conversation span") {
                    summary
                } else {
                    reply
                };
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from(sse_body(&chunks)))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

async fn boot() -> Engine {
    // The provider guest reads the LLM key from the env var its args NAME
    // (`key_env`), and that var rides the PROCESS environment the guest
    // child inherits. The fixture owns the requirement — without it the
    // provider bails at Start and every happy-path turn ends empty, so
    // the suite would depend on an ambient variable no machine is told
    // to export.
    std::env::set_var("GRAVITY_TEST_KEY", "test-key");
    let engine = Engine::start(&Default::default())
        .await
        .expect("engine boot");
    engine
        .register(aura_booth::BoothType::script(
            "llm_provider",
            "induction",
            // The spawn spec is argv; the fixture bin path is absolute
            // (no spaces — the shellish split rides them verbatim).
            provider_bin(),
        ))
        .await
        .unwrap();
    engine
        .register(aura_booth::BoothType::script(
            "gravity_turn",
            "induction",
            turn_bin(),
        ))
        .await
        .unwrap();
    engine
}

async fn mem_server() -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let app = mem_server::open(dir.path().to_path_buf()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // Leak the guard: the store files must outlive the test (the same
    // discipline the python-cut tests used).
    std::mem::forget(dir);
    format!("http://{addr}")
}

/// A mem-server whose compression policy fires on any non-empty increment
/// (k10r's own budget-only recipe, copied from its compression surface test):
/// the turn's session read then carries a `compress` verdict, which is what
/// the side branch rides. The cache clock is out of reach, so only the budget
/// can speak.
async fn mem_server_with_compression() -> String {
    let dir = tempfile::TempDir::new().unwrap();
    let app = mem_server::open_with_compression(
        dir.path().to_path_buf(),
        None,
        mem_core::CompressionParams {
            context_bytes: 100,
            budget_percent: 50,
            cache_ttl_ms: 3_600_000,
            floor_bytes: 1,
        },
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    std::mem::forget(dir);
    format!("http://{addr}")
}

fn turn_args(store_url: &str, endpoint: &str, msg: &str) -> serde_json::Value {
    serde_json::json!({
        "channel_id": 7,
        "member_id": 1,
        "user_message": msg,
        "store_url": store_url,
        "endpoint": endpoint,
        "model": "test-model",
        "key_env": "GRAVITY_TEST_KEY",
        // The k10r namespace the memory tools reach (ADR-0004 §5): k10r's
        // FACT_NS, the only scope a fact tuple can land in.
        "memory_scope": 37,
    })
}

/// The gateway's shape (prism turn.submit does exactly this): Start,
/// then Next pulls in a loop, collecting `item.delta` per envelope.
async fn drain_turn(engine: &Engine, store_url: &str, endpoint: &str, msg: &str) -> Vec<String> {
    let slot = Realm::iterate(
        &engine.realm,
        IterateOp::Start {
            target: InstanceId {
                booth_type: TURN_TYPE.into(),
                key: InstanceKey::Named("s1".into()),
            },
            handler: "turn".into(),
            args: turn_args(store_url, endpoint, msg),
        },
    )
    .await
    .unwrap();
    let (mut rx, mut stream_id) = match slot {
        CallSlot::Hot { rx, .. } => (rx, None),
        _ => panic!("iterate is hot-only"),
    };
    let mut deltas = Vec::new();
    loop {
        let v = match tokio::time::timeout(std::time::Duration::from_secs(20), rx).await {
            Ok(Ok(Ok(v))) => v,
            Ok(Ok(Err(e))) => panic!("pull failed: {e}"),
            Ok(Err(_)) => panic!("reply dropped"),
            Err(_) => panic!("pull {stream_id:?} stalled for 20s"),
        };
        if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
            // A failed guest answers a STRING value (the carrier wraps a
            // bare error reply as `{done:true, value:"…"}`), while a
            // finished turn answers `{value:{reply}}`. Reading `done`
            // alone turns "the provider never started" into "the model
            // said nothing" — the exact failure this fixture is here to
            // catch, so it fails loudly instead.
            if let Some(err) = v.get("value").and_then(|value| value.as_str()) {
                panic!("turn failed: {err}");
            }
            return deltas;
        }
        if stream_id.is_none() {
            stream_id = Some(
                v.get("stream_id")
                    .and_then(|s| s.as_str())
                    .unwrap()
                    .to_string(),
            );
        }
        if let Some(d) = v
            .get("item")
            .and_then(|i| i.get("delta"))
            .and_then(|d| d.as_str())
        {
            deltas.push(d.to_string());
        }
        let id = stream_id.clone().unwrap();
        match Realm::iterate(&engine.realm, IterateOp::Next { stream_id: id })
            .await
            .unwrap()
        {
            CallSlot::Hot { rx: next, .. } => rx = next,
            _ => panic!("iterate is hot-only"),
        }
    }
}

#[tokio::test]
async fn turn_booth_streams_deltas() {
    let endpoint = sse_server(vec!["hel", "lo ", "wor", "ld"]).await;
    let store_url = mem_server().await;
    let engine = boot().await;

    let deltas = drain_turn(&engine, &store_url, &endpoint, "hi").await;
    assert_eq!(
        deltas.join(""),
        "hello world",
        "tokens stream one per envelope"
    );
}

#[tokio::test]
async fn stream_appends_the_exchange_after_done() {
    let endpoint = sse_server(vec!["hel", "lo"]).await;
    let store_url = mem_server().await;
    let engine = boot().await;

    let deltas = drain_turn(&engine, &store_url, &endpoint, "hi").await;
    assert_eq!(deltas.join(""), "hello");

    // The memory seam saw the exchange: user + assistant, one ts, subs
    // 0/1. Read through gravity's own HTTP surface (the same wire the
    // guest posted through).
    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    let view = surface.fetch_session(7, 1).await.unwrap();
    assert_eq!(view.increment.len(), 2);
    assert_eq!(view.increment[0].1.role.as_deref(), Some("user"));
    assert_eq!(view.increment[0].1.content, "hi");
    assert_eq!(view.increment[1].1.role.as_deref(), Some("assistant"));
    assert_eq!(view.increment[1].1.content, "hello");
    assert_eq!(
        view.increment[0].0.ts_ms, view.increment[1].0.ts_ms,
        "one exchange one timestamp"
    );
    assert_eq!(view.increment[0].0.sub, 0);
    assert_eq!(view.increment[1].0.sub, 1);
}

#[tokio::test]
async fn failed_provider_pull_appends_nothing() {
    // A server that rejects the request — the provider's open fails,
    // the turn's Start errors, the pull that observed it fails as a
    // value, and nothing appends.
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");
    let store_url = mem_server().await;
    let engine = boot().await;

    let slot = Realm::iterate(
        &engine.realm,
        IterateOp::Start {
            target: InstanceId {
                booth_type: TURN_TYPE.into(),
                key: InstanceKey::Named("s1".into()),
            },
            handler: "turn".into(),
            args: turn_args(&store_url, &endpoint, "hi"),
        },
    )
    .await
    .unwrap();
    // The failure surfaces as a terminal envelope whose value carries the
    // error (a guest handler that dies before opening a stream = the
    // invoke shape, ADR-0036 §2 — the error never silently swallows).
    let failed = match slot {
        CallSlot::Hot { rx, .. } => match rx.await {
            Ok(Err(_)) => true,
            Ok(Ok(v)) => {
                v.get("done").and_then(|d| d.as_bool()) == Some(true)
                    && v.get("value")
                        .and_then(|v| v.as_str())
                        .map(|s| s.contains("provider stream failed to open"))
                        .unwrap_or(false)
            }
            Err(_) => false,
        },
        _ => panic!("hot-only"),
    };
    assert!(failed, "the turn fails through (no silent swallow)");

    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    let view = surface.fetch_session(7, 1).await.unwrap();
    assert!(view.increment.is_empty(), "a failed turn appends nothing");
}

/// Transport-level retry (Phase 1): transient failures (503) are
/// retried with constant backoff — the turn succeeds once the endpoint
/// recovers; a client error (401) is NOT transient and fails through
/// on the first attempt.
#[tokio::test]
async fn transient_endpoint_failures_are_retried() {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_for_route = hits.clone();
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move || {
            let n = hits_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                if n < 2 {
                    axum::response::Response::builder()
                        .status(axum::http::StatusCode::SERVICE_UNAVAILABLE)
                        .body(axum::body::Body::empty())
                        .unwrap()
                } else {
                    let mut sse = String::new();
                    sse.push_str(r#"data: {"choices":[{"delta":{"content":"recovered"}}]}"#);
                    sse.push_str("\n\ndata: [DONE]\n\n");
                    axum::response::Response::builder()
                        .header("content-type", "text/event-stream")
                        .body(axum::body::Body::from(sse))
                        .unwrap()
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");
    let store_url = mem_server().await;
    let engine = boot().await;

    let deltas = drain_turn(&engine, &store_url, &endpoint, "hi").await;
    assert_eq!(deltas.join(""), "recovered", "the retry landed the stream");
    assert!(
        hits.load(std::sync::atomic::Ordering::SeqCst) >= 3,
        "the provider actually retried"
    );
}

#[tokio::test]
async fn client_errors_fail_through_without_retry() {
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_for_route = hits.clone();
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move || {
            let n = hits_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                assert_eq!(n, 0, "a 401 must not be retried");
                axum::response::Response::builder()
                    .status(axum::http::StatusCode::UNAUTHORIZED)
                    .body(axum::body::Body::empty())
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");
    let store_url = mem_server().await;
    let engine = boot().await;

    let slot = Realm::iterate(
        &engine.realm,
        IterateOp::Start {
            target: InstanceId {
                booth_type: TURN_TYPE.into(),
                key: InstanceKey::Named("s1".into()),
            },
            handler: "turn".into(),
            args: turn_args(&store_url, &endpoint, "hi"),
        },
    )
    .await
    .unwrap();
    let failed = match slot {
        CallSlot::Hot { rx, .. } => match rx.await {
            Ok(Err(_)) => true,
            Ok(Ok(v)) => v
                .get("value")
                .and_then(|v| v.as_str())
                .map(|s| s.contains("rejected the request (401)"))
                .unwrap_or(false),
            Err(_) => false,
        },
        _ => panic!("hot-only"),
    };
    assert!(failed, "the 401 fails through");
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "exactly one attempt — a 401 is not transient"
    );
}

/// Phase 2 e2e, on the fixed list (ADR-0004): the provider asks for a tool by
/// name, the turn dispatches it to the DOOR that name belongs to, the result
/// feeds back into the next inference round, and the write actually landed in
/// k10r (read back over the graph door, not trusted from the tool's own text).
///
/// `commit_memory` is the entry chosen because its door is deterministic: the
/// ingest answer says which of landed / rewritten / dropped happened, and the
/// graph read needs no embedding, so the test proves the path without an
/// engine under the fixture.
#[tokio::test]
async fn a_tool_call_reaches_its_door_and_feeds_back() {
    // The provider answers the loop's TWO inference rounds the way a real
    // endpoint does: the first asks for the tool, the SECOND (the re-open
    // carrying the fed-back result) answers content. The bodies are kept so
    // the test can assert what the model was OFFERED, not just what it got.
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let bodies = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let hits_for_route = hits.clone();
    let bodies_for_route = bodies.clone();
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move |request: axum::extract::Request| {
            let bodies = bodies_for_route.clone();
            async move {
                let n = hits_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let bytes = axum::body::to_bytes(request.into_body(), 1 << 20)
                    .await
                    .unwrap();
                bodies
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&bytes).to_string());
                let mut sse = String::new();
                if n == 0 {
                    sse.push_str(
                        r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c1","function":{"name":"commit_memory","arguments":"{\"facts\":[[\"alice\",\"person\",{},\"likes\",{},\"tea\",\"drink\",{}]]}"}}]}}]}"#,
                    );
                } else {
                    sse.push_str(r#"data: {"choices":[{"delta":{"content":"all good"}}]}"#);
                }
                sse.push_str("\n\ndata: [DONE]\n\n");
                axum::response::Response::builder()
                    .header("content-type", "text/event-stream")
                    .body(axum::body::Body::from(sse))
                    .unwrap()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");
    let store_url = mem_server().await;
    let engine = boot().await;

    let slot = Realm::iterate(
        &engine.realm,
        IterateOp::Start {
            target: InstanceId {
                booth_type: TURN_TYPE.into(),
                key: InstanceKey::Named("s1".into()),
            },
            handler: "turn".into(),
            args: turn_args(&store_url, &endpoint, "hi"),
        },
    )
    .await
    .unwrap();
    let (mut rx, mut stream_id) = match slot {
        CallSlot::Hot { rx, .. } => (rx, None),
        _ => panic!("iterate is hot-only"),
    };
    let mut reply = String::new();
    loop {
        let v = match rx.await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => panic!("pull failed: {e}"),
            Err(_) => panic!("reply dropped"),
        };
        if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
            break;
        }
        if stream_id.is_none() {
            stream_id = Some(
                v.get("stream_id")
                    .and_then(|s| s.as_str())
                    .unwrap()
                    .to_string(),
            );
        }
        if let Some(delta) = v.get("item").and_then(|i| i.get("delta")).and_then(|d| d.as_str()) {
            reply.push_str(delta);
        }
        let id = stream_id.clone().unwrap();
        match Realm::iterate(&engine.realm, IterateOp::Next { stream_id: id })
            .await
            .unwrap()
        {
            CallSlot::Hot { rx: next, .. } => rx = next,
            _ => panic!("iterate is hot-only"),
        }
    }
    // The stream carries the ANSWER and nothing else (prism's face takes
    // `item.delta`; a tool_result item would kill the turn at the pump).
    assert_eq!(reply, "all good", "the answer streamed after the tool round");

    // The list rode BOTH rounds: the first request offered it, and the
    // re-open (the next inference round) carried the same affordances.
    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "two inference rounds");
    for body in &bodies {
        let request: serde_json::Value = serde_json::from_str(body).expect("a JSON request");
        let tools = request["tools"].as_array().expect("the tools array is sent");
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec!["recall_memory", "commit_memory", "resolve_skills"],
            "{body}"
        );
    }
    // The tool's answer rode back INTO the loop: the re-open carries the
    // assistant's call and the tool message (OpenAI's wire shape), which is
    // the only place a tool result belongs.
    let second: serde_json::Value = serde_json::from_str(&bodies[1]).expect("a JSON request");
    let tool_message = second["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the tool result is fed back to the model");
    assert_eq!(tool_message["tool_call_id"], "c1");
    assert!(
        tool_message["content"].as_str().unwrap_or("").contains("outcome"),
        "the ingest's answer reached the model: {tool_message}"
    );

    // And the write LANDED: the graph door shows the entity the tuple named.
    // The read runs on a BLOCKING thread: mem-server lives in this process, so
    // a blocking client call on the test's single-threaded runtime would starve
    // the very server it is asking (the fixture deadlock, not the product's).
    let graph_url = format!("{store_url}/graph/37");
    let graph = tokio::task::spawn_blocking(move || {
        ureq::get(&graph_url)
            .call()
            .unwrap()
            .into_json::<serde_json::Value>()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(
        graph.to_string().contains("alice"),
        "the fact reached k10r through commit_memory: {graph}"
    );
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the loop re-opened the provider exactly once"
    );
}

/// The dispatch loop's cap is a runaway stop, not a budget to spend
/// quietly: a provider that answers tool calls to EVERY inference round
/// never hands the turn a content stream, and the turn fails with a
/// NAMED error — never an empty first token, never a stream that opens
/// and then dies on an empty reply.
#[tokio::test]
async fn a_provider_that_never_stops_asking_for_tools_fails_the_turn() {
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(|| async {
            let mut sse = String::new();
            sse.push_str(
                r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c1","function":{"name":"echo","arguments":"{\"x\":42}"}}]}}]}"#,
            );
            sse.push_str("\n\ndata: [DONE]\n\n");
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .body(axum::body::Body::from(sse))
                .unwrap()
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");
    let store_url = mem_server().await;
    let engine = boot().await;

    let slot = Realm::iterate(
        &engine.realm,
        IterateOp::Start {
            target: InstanceId {
                booth_type: TURN_TYPE.into(),
                key: InstanceKey::Named("s1".into()),
            },
            handler: "turn".into(),
            args: turn_args(&store_url, &endpoint, "hi"),
        },
    )
    .await
    .unwrap();
    // The failure rides the Start face's terminal envelope (ADR-0036
    // §2: a handler that never opens a stream answers the invoke shape).
    let v = match slot {
        CallSlot::Hot { rx, .. } => match rx.await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => panic!("the failure must ride as a value: {e}"),
            Err(_) => panic!("reply dropped"),
        },
        _ => panic!("iterate is hot-only"),
    };
    assert!(
        v.get("value")
            .and_then(|s| s.as_str())
            .map(|s| s.contains("did not converge"))
            .unwrap_or(false),
        "the round cap names itself: {v}"
    );

    // Nothing appended: a turn that never produced content is not an
    // exchange (the oldest rule, through the loop's failure path).
    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    let view = surface.fetch_session(7, 1).await.unwrap();
    assert!(view.increment.is_empty(), "a failed turn appends nothing");
}

/// Phase 3's turn-loop half: k10r's verdict rode the projection, so the turn
/// fires the side branch from the SAME prefix (without this turn's message),
/// runs it alongside the answer, and lands the summary through the checkpoint
/// door before the next message — k10r derives the coverage point from the
/// verdict it served, so the caller never states one.
#[tokio::test]
async fn the_compression_branch_lands_the_served_summary() {
    let endpoint =
        sse_server_two_branches(vec!["hel", "lo ", "wor", "ld"], vec!["compressed ", "span"]).await;
    let store_url = mem_server_with_compression().await;
    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    // Three records with a wide gap before the last one: the verdict's split
    // is that gap's end (k10r's own recipe).
    for (ts, text) in [
        (1_000u64, "first message of the span"),
        (2_000, "second message of the span"),
        (9_000, "third message of the span"),
    ] {
        surface.append_message(7, ts, 0, 0, 0, text).await.unwrap();
    }

    let engine = boot().await;
    let deltas = drain_turn(&engine, &store_url, &endpoint, "hi").await;
    assert_eq!(deltas.join(""), "hello world", "the answer still streams");

    let view = surface.fetch_session(7, 1).await.unwrap();
    assert_eq!(
        view.summary.as_deref(),
        Some("compressed span"),
        "the checkpoint door took the summary the side branch produced"
    );
    assert_eq!(view.coverage_ts, 2_000, "the split k10r proposed");
}