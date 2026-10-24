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

async fn boot() -> Engine {
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

fn turn_args(store_url: &str, endpoint: &str, msg: &str) -> serde_json::Value {
    serde_json::json!({
        "channel_id": 7,
        "member_id": 1,
        "user_message": msg,
        "store_url": store_url,
        "endpoint": endpoint,
        "model": "test-model",
        "key_env": "GRAVITY_TEST_KEY",
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
        let v = match rx.await {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => panic!("pull failed: {e}"),
            Err(_) => panic!("reply dropped"),
        };
        if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
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

/// Phase 2 e2e: the provider's stream carries a tool_call — the turn
/// parses it and dispatches through ctx.invoke to a SIBLING booth (the
/// tool target type, here effector's induction_loop fixture speaking
/// the same pipe), and the tool result surfaces as the stream's own
/// envelope item. The cross-repo fixture path is the standing
/// convention (aura's exec_booth.rs does the same).
#[tokio::test]
async fn tool_call_dispatches_to_the_sibling_booth() {
    // The provider answers the loop's TWO inference rounds the way a
    // real endpoint does: the first request asks for the tool, the
    // SECOND (the loop's re-open, carrying the fed-back result) answers
    // content. A stub answering tool calls to every request can never
    // converge — the dispatch loop would burn its whole round budget
    // and the turn would die on an empty reply.
    let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_for_route = hits.clone();
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move || {
            let n = hits_for_route.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move {
                let mut sse = String::new();
                if n == 0 {
                    sse.push_str(
                        r#"data: {"choices":[{"delta":{"tool_calls":[{"id":"c1","function":{"name":"echo","arguments":"{\"x\":42}"}}]}}]}"#,
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
    // The tool target's TYPE is the caller's declaration (the deployment
    // fact) — registered under a non-default name so a turn that
    // ignored `tool_target` would dispatch into nothing and answer an
    // error instead of the echo.
    register_tool_target(&engine, "tool_booth").await;

    let mut args = turn_args(&store_url, &endpoint, "hi");
    args["tool_target"] = serde_json::json!("tool_booth");
    let slot = Realm::iterate(
        &engine.realm,
        IterateOp::Start {
            target: InstanceId {
                booth_type: TURN_TYPE.into(),
                key: InstanceKey::Named("s1".into()),
            },
            handler: "turn".into(),
            args,
        },
    )
    .await
    .unwrap();
    let (mut rx, mut stream_id) = match slot {
        CallSlot::Hot { rx, .. } => (rx, None),
        _ => panic!("iterate is hot-only"),
    };
    let mut saw_tool_result = None;
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
        if let Some(tr) = v.get("item").and_then(|i| i.get("tool_result")) {
            saw_tool_result = Some(tr.clone());
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
    let tr = saw_tool_result.expect("the tool result surfaced as an envelope item");
    assert_eq!(tr["tool_call_id"], "c1");
    // The fixture's echo handler wraps args: {"echoed": {"x": 42}}.
    assert!(
        tr["content"].to_string().contains("42"),
        "the sibling booth answered through ctx.invoke: {tr}"
    );
    // One dispatch loop round: the provider saw the first request (the
    // tool call) and exactly one re-open carrying the fed-back result —
    // the second round answered content and let the pull face take over.
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        2,
        "the loop re-opened the provider exactly once"
    );
}

/// Register the sibling tool target: effector's `induction_loop` fixture
/// (its `echo` handler answers `{"echoed": args}`), speaking the same
/// pipe. The cross-repo fixture path is the standing convention (aura's
/// exec_booth.rs does the same) — built in ~/world/effector. The booth
/// TYPE is the caller's declaration; the registered name is whatever the
/// turn's `tool_target` arg names.
async fn register_tool_target(engine: &Engine, booth_type: &str) {
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../effector/target/debug/examples/induction_loop"
    );
    engine
        .register(aura_booth::BoothType::script(
            booth_type,
            "induction",
            fixture.to_string(),
        ))
        .await
        .unwrap();
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
    // The default deployment fact: `tool_target` absent names `effector`.
    register_tool_target(&engine, "effector").await;

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
