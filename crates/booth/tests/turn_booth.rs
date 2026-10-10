//! The booth binding contract (PLAN Phase 4, first landing): the turn
//! execution registered as a Rust booth, the provider as a python
//! booth — one realm, the turn closure pulling tokens through
//! `ctx.iterate`. The LLM wire is stubbed by a local SSE server (the
//! same shape the real provider booth will parse against), the memory
//! seam rides a real mem-server. What is pinned:
//!
//! - the turn booth's round trip: fetch → prompt → provider stream →
//!   append (user + assistant, one ts two subs) → reply value;
//! - a failed provider pull appends nothing (the turn function's
//!   oldest rule, through the booth form);
//! - the provider booth's arg contract: endpoint/model/key_env +
//!   messages; the key NEVER in the args, only its env name.

use aura_booth::{InstanceId, InstanceKey};
use aura_engine::Engine;
use gravity_booth::{provider_booth, turn_booth, PROVIDER_HANDLER, PROVIDER_TYPE, TURN_TYPE};
use gravity_core::llm::LlmClient;
use gravity_core::surface::{HttpSurface, MemorySurface};
use mem_core::channel::role;
use std::sync::Arc;

/// A minimal OpenAI-compatible SSE endpoint: answers one streamed
/// reply assembled from `CHUNKS`, then `[DONE]`. Axum (already a
/// dev-dep of gravity-core) serving raw SSE bodies.
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

async fn boot_realm(
    surface: Arc<dyn MemorySurface>,
    client: LlmClient,
) -> anyhow::Result<Engine> {
    let engine = Engine::start(&Default::default()).await?;
    engine.register(provider_booth()).await?;
    engine.register(turn_booth(surface, client)).await?;
    Ok(engine)
}

fn llm_client(endpoint: &str) -> LlmClient {
    // LlmClient resolves the key from an env var — point it at a var
    // the test sets (the booth form re-reads it python-side).
    std::env::set_var("GRAVITY_TEST_KEY", "test-key");
    LlmClient::from_config(&gravity_config::GravityConfig {
        endpoint: endpoint.to_string(),
        model: "test-model".into(),
        api_key_env: "GRAVITY_TEST_KEY".into(),
        store_path: None,
        store_url: None,
    })
    .unwrap()
}

async fn mem_surface() -> (Arc<dyn MemorySurface>, tempfile::TempDir) {
    let dir = tempfile::TempDir::new().unwrap();
    let app = mem_server::open(dir.path().to_path_buf()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let surface: Arc<dyn MemorySurface> =
        Arc::new(HttpSurface::new(&format!("http://{addr}")));
    (surface, dir)
}

#[tokio::test]
async fn turn_booth_round_trip() {
    let chunks = vec!["hel", "lo ", "wor", "ld"];
    let endpoint = sse_server(chunks).await;
    let (surface, _dir) = mem_surface().await;
    let engine = boot_realm(Arc::clone(&surface), llm_client(&endpoint))
        .await
        .unwrap();

    let out = engine
        .invoke(
            InstanceId { booth_type: TURN_TYPE.into(), key: InstanceKey::Named("s1".into()) },
            "turn",
            serde_json::json!({"channel_id": 7, "member_id": 1, "user_message": "hi"}),
        )
        .await
        .unwrap();
    assert_eq!(out["reply"], "hello world", "the streamed tokens assemble the reply");

    // The memory seam saw the exchange: user + assistant, one ts two subs.
    let view = surface.fetch_session(7, 1).await.unwrap();
    assert_eq!(view.increment.len(), 2);
    assert_eq!(view.increment[0].1.role, role::USER);
    assert_eq!(view.increment[0].1.content, "hi");
    assert_eq!(view.increment[1].1.role, role::ASSISTANT);
    assert_eq!(view.increment[1].1.content, "hello world");
    assert_eq!(view.increment[0].0.ts_ms, view.increment[1].0.ts_ms, "one exchange one timestamp");
    assert_eq!(view.increment[0].0.sub, 0);
    assert_eq!(view.increment[1].0.sub, 1);
}

#[tokio::test]
async fn failed_provider_pull_appends_nothing() {
    // A server that rejects the request — the pull fails, the turn
    // appends nothing (the failed attempt never enters the log).
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(|| async {
            axum::http::StatusCode::INTERNAL_SERVER_ERROR
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");

    let (surface, _dir) = mem_surface().await;
    let engine = boot_realm(Arc::clone(&surface), llm_client(&endpoint))
        .await
        .unwrap();

    let out = engine
        .invoke(
            InstanceId { booth_type: TURN_TYPE.into(), key: InstanceKey::Named("s1".into()) },
            "turn",
            serde_json::json!({"channel_id": 7, "member_id": 1, "user_message": "hi"}),
        )
        .await;
    assert!(out.is_err(), "the turn fails through (no silent single-shot)");

    let view = surface.fetch_session(7, 1).await.unwrap();
    assert!(view.increment.is_empty(), "a failed turn appends nothing");
}

#[test]
fn provider_booth_source_never_carries_the_key() {
    // Key custody: the module source reads the env var named in the
    // args — the key value appears nowhere in the source (the args
    // carry `key_env`, the NAME).
    let src = provider_booth_source();
    assert!(src.contains("os.environ"), "the provider reads the env per call");
    assert!(!src.contains("sk-"), "no literal key in the module");
    let _ = (PROVIDER_HANDLER, PROVIDER_TYPE);
}

fn provider_booth_source() -> String {
    // Re-exported source (gravity-booth embeds it as a const).
    gravity_booth::PROVIDER_SOURCE.into()
}
