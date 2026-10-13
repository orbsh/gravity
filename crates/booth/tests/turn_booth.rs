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
            eprintln!("DEBUG terminal envelope: {v}");
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
    assert_eq!(view.increment[0].1.role, mem_core::channel::role::USER);
    assert_eq!(view.increment[0].1.content, "hi");
    assert_eq!(view.increment[1].1.role, mem_core::channel::role::ASSISTANT);
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
