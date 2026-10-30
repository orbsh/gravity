//! The booth binding contract (PLAN Phase 4): the turn execution as a Rust guest process,
//! the provider as a sibling guest, one realm, and the venue's own wire as the output.
//!
//! The shape under test is the one prism serves: the venue emits `view:turn.submit` with an
//! envelope (`{name, channel_id, sender, args}`), the turn runs to its end inside one
//! handler call, and every chunk it draws is its OWN emit — nothing pulls, so a pull face
//! that stopped existing cannot break the turn. The LLM wire is stubbed by a local SSE
//! server, the memory seam rides a real mem-server, and a compiled collector booth
//! subscribed to the `view:` prefix is the renderer's stand-in.
//!
//! What is pinned:
//!
//! - the STREAMING round trip: each token arrives as its own emit while the stream is live
//!   (not one buffered answer), the chunks assemble the reply, and the memory seam sees the
//!   one-ts-two-subs exchange after the stream ends;
//! - the ENVELOPE the venue reads: `name` names the event, `channel_id` the address, and
//!   the frames hang off `data` (a delivery, not a request);
//! - a failed provider appends nothing (the turn function's oldest rule, through the booth
//!   form) and the failure crosses as a value.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unwrap_in_result,
    clippy::panic_in_result_fn
)]

use std::sync::Arc;
use std::time::Duration;

use aura_booth::{
    call::CallSlot, Body, BoothType, InstanceId, InstanceKey, IterateOp, ReceiveDecl,
    RouteResolution,
};
use aura_engine::Engine;
use aura_realm::Realm;
use gravity_booth::TURN_TYPE;
use gravity_core::surface::MemorySurface;
use serde_json::{json, Value};

/// The two guest binaries — cargo builds this package's own bins for
/// its integration tests and exports `CARGO_BIN_EXE_<name>`, so the
/// fixtures can never run stale (the induction pair.rs pattern).
fn provider_bin() -> String {
    env!("CARGO_BIN_EXE_gravity-provider").to_string()
}

fn turn_bin() -> String {
    env!("CARGO_BIN_EXE_gravity-turn").to_string()
}

/// What the collector booth saw: the `data` payload of every `view:` emit, in arrival
/// order. The renderer's stand-in — a compiled body, because a script would need the frame
/// vocabulary to say what it observed.
#[derive(Clone, Default)]
struct Seen(Arc<std::sync::Mutex<Vec<Value>>>);

impl Seen {
    fn push(&self, value: Value) {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(value);
    }

    fn events(&self) -> Vec<Value> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn len(&self) -> usize {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }

    fn all(&self, event: &str) -> Vec<Value> {
        self.events()
            .into_iter()
            .filter(|e| e.get("name").and_then(Value::as_str) == Some(event))
            .collect()
    }
}

/// A minimal OpenAI-compatible SSE endpoint: answers one streamed
/// reply assembled from `body_chunks`, then `[DONE]`.
async fn sse_server(body_chunks: Vec<&'static str>) -> String {
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move || {
            let chunks = body_chunks.clone();
            async move {
                let mut sse = String::new();
                for c in chunks {
                    let payload = json!({ "choices": [{"delta": {"content": c}}] });
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
        let payload = json!({ "choices": [{ "delta": { "content": c } }] });
        sse.push_str(&format!("data: {payload}\n\n"));
    }
    sse.push_str("data: [DONE]\n\n");
    sse
}

/// An SSE stub that answers the answer, the compression branch and the review
/// branch DIFFERENTLY, keyed on the request itself: k10r's tail prompt is the last
/// message of the side branch (the turn appends it in place of the user's
/// message), so a request that ends with it asks for the summary and every
/// other request asks for the reply.
async fn sse_server_three_branches(
    reply: Vec<&'static str>,
    summary: Vec<&'static str>,
    review: Vec<&'static str>,
) -> String {
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(move |body: axum::body::Bytes| {
            let reply = reply.clone();
            let summary = summary.clone();
            let review = review.clone();
            async move {
                let asked = serde_json::from_slice::<Value>(&body)
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
                // The branch is recognised by the PROMPT it was handed: k10r
                // serves one prompt per operation, so the stub needs no ordering
                // assumptions about which branch connects first.
                let chunks = if asked.starts_with("Two products from the conversation span") {
                    summary
                } else if asked.starts_with("Review the conversation span") {
                    review
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

/// Boot the realm: both guests, the turn registered with the venue's subscription route
/// (what `gravity_booth::register` does in a deployment), and a collector standing in for
/// the renderer.
async fn boot(store_url: &str, endpoint: &str) -> (Engine, Seen) {
    let (engine, seen) = boot_bare().await;
    seed(&engine, store_url, endpoint);
    (engine, seen)
}

/// The realm with both guests and the collector, and NO deployment rows — the state a fresh
/// deployment is in before anyone configures it.
async fn boot_bare() -> (Engine, Seen) {
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
        .register(BoothType::script(
            "llm_provider",
            "induction",
            // The spawn spec is argv; the fixture bin path is absolute
            // (no spaces — the shellish split rides them verbatim).
            provider_bin(),
        ))
        .await
        .unwrap();
    engine
        .register(
            BoothType::script(TURN_TYPE, "induction", turn_bin())
                // The venue's own word for "run a turn", exactly as `register` declares it:
                // the entry is `turn`, the address field is the envelope's `channel_id`.
                .on_as("view:turn.submit", "device", "turn"),
        )
        .await
        .unwrap();
    let seen = Seen::default();
    let collector = seen.clone();
    engine
        .register(BoothType {
            name: "collector".into(),
            body: Body::Rust(Arc::new(move |_ctx, args| {
                let collector = collector.clone();
                Box::pin(async move {
                    collector.push(args);
                    Ok(Value::Null)
                })
            })),
            idle_ttl: None,
            max_exec: None,
            hooks: std::collections::BTreeMap::new(),
            // TWO ROUTES: the prefix every UI-facing emit uses (the renderer's declaration,
            // prism's `install`) and the data event this fixture's callers name — a booth
            // sees exactly what it declared, and the data protocol is a DIFFERENT event, not
            // a `view:` one.
            receives: vec![
                ReceiveDecl {
                    event: "view:*".into(),
                    entry: None,
                    resolution: RouteResolution::Singleton,
                    wildcard: true,
                },
                ReceiveDecl {
                    event: "turn.delta".into(),
                    entry: None,
                    resolution: RouteResolution::Singleton,
                    wildcard: false,
                },
            ],
        })
        .await
        .unwrap();
    (engine, seen)
}

/// THE DEPLOYMENT FACTS ARE ROWS (ADR-0007): every fixture writes them into the turn type's own
/// namespace before driving a turn — which is also the proof that the induction carrier's
/// `storage` block reaches the realm as a plan (the guest declares it; `Engine::register`
/// persists the schema and allocates the type's ns).
fn seed(engine: &Engine, store_url: &str, endpoint: &str) {
    let (store, plan) = {
        let realm = engine.realm.try_lock().expect("realm");
        (
            realm.mq.clone(),
            realm
                .plan_of(TURN_TYPE)
                .cloned()
                .expect("the turn declares `settings` + `models`"),
        )
    };
    let put = |collection: &str, key: Value, doc: Value| {
        aura_realm::store_exec::execute(
            &store,
            &plan,
            &aura_booth::StoreOp {
                collection: collection.to_string(),
                op: aura_booth::StoreOpKind::PutDocument { key, doc },
            },
        )
        .expect("seed")
    };
    put(
        "models",
        json!({ "kind": 2, "id": 1 }),
        json!({"alias": "default", "endpoint": endpoint, "model": "test-model", "key_env": "GRAVITY_TEST_KEY", "weight": 1, "enabled": 1}),
    );
    put(
        "settings",
        json!({ "kind": 1, "id": 1 }),
        json!({"store_url": store_url, "memory_scope": 37, "memory_format": "plaintext", "default_model": 1}),
    );
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

/// The caller's payload. `event` is the data protocol's name for its chunks — gravity owns
/// no event names, so a consumer that wants `{"delta": …}` says what to call it.
fn turn_args(msg: &str) -> Value {
    json!({
        // What the page knows: the conversation it is in, the text, and the output shape it wants.
        // The deployment facts (store, scope, model recipe) are the application's own rows now.
        "channel_id": 7,
        "member_id": 1,
        "user_message": msg,
        "event": "turn.delta",
    })
}

/// The same payload asking for FRAMES: the drawing caller names its slots (gravity does not
/// own a layout, so it cannot know them).
fn view_args(msg: &str) -> Value {
    let mut args = turn_args(msg);
    args["protocol"] = json!("fluxen");
    args["slots"] = json!({ "rack": "chat", "text": "say" });
    args
}

/// Drive the handler directly (the sibling-booth / test shape: flat args, no envelope) and
/// await the turn's terminal. Returns the failure text when the turn failed — a guest
/// handler's error crosses the seam as a STRING value, so reading `done` alone would turn
/// "the provider never started" into "the model said nothing".
async fn start_turn(engine: &Engine, args: Value) -> Result<(), String> {
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
    let rx = match slot {
        CallSlot::Hot { rx, .. } => rx,
        _ => panic!("iterate is hot-only"),
    };
    let v = match tokio::time::timeout(Duration::from_secs(30), rx).await {
        Ok(Ok(Ok(v))) => v,
        Ok(Ok(Err(e))) => return Err(e.to_string()),
        Ok(Err(_)) => panic!("reply dropped"),
        Err(_) => panic!("the turn stalled for 30s"),
    };
    assert_eq!(
        v.get("done").and_then(Value::as_bool),
        Some(true),
        "a handler that opens no stream answers the terminal shape: {v}"
    );
    match v.get("value").and_then(Value::as_str) {
        Some(error) => Err(error.to_string()),
        None => Ok(()),
    }
}

/// Emitted events are queued on the realm MQ and drained by the consumer's own instance, so
/// "the guest has written them" and "the collector has read them" are two moments. The pipe
/// order is fixed (every emit precedes the handler's reply on the same channel), so a quiet
/// window after the terminal is enough — and it is the only wait this fixture has.
async fn settle(seen: &Seen) {
    let mut last = seen.len();
    let mut quiet = 0;
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let now = seen.len();
        if now == last {
            quiet += 1;
            if quiet >= 10 {
                return;
            }
        } else {
            last = now;
            quiet = 0;
        }
    }
}

/// One turn through the direct route, returning the deltas the caller's event saw.
async fn run_turn(engine: &Engine, seen: &Seen, args: Value) -> Result<Vec<String>, String> {
    let before = seen.len();
    let outcome = start_turn(engine, args).await;
    settle(seen).await;
    // An emit's payload is the ENVELOPE the venue reads: `{name, data}` — the chunk is the
    // envelope's `data`, which is what a consumer destructures.
    let deltas = seen.events()[before..]
        .iter()
        .filter_map(|e| e.get("data").and_then(|d| d.get("delta")))
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    outcome.map(|()| deltas)
}

#[tokio::test]
async fn the_turn_streams_its_chunks_as_events() {
    let endpoint = sse_server(vec!["hel", "lo ", "wor", "ld"]).await;
    let store_url = mem_server().await;
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let deltas = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();
    assert_eq!(
        deltas.join(""),
        "hello world",
        "tokens arrive as their own events, in order"
    );
}

#[tokio::test]
async fn the_exchange_lands_after_the_stream_ends() {
    let endpoint = sse_server(vec!["hel", "lo"]).await;
    let store_url = mem_server().await;
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let deltas = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();
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
async fn failed_provider_appends_nothing() {
    // A server that rejects the request — the provider's open fails,
    // the turn fails through as a value, and nothing appends.
    let app = axum::Router::new().route(
        "/chat/completions",
        axum::routing::post(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let endpoint = format!("http://{addr}");
    let store_url = mem_server().await;
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let failure = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap_err();
    assert!(
        failure.contains("provider stream failed to open"),
        "the failure names itself: {failure}"
    );
    assert!(
        seen.events().is_empty(),
        "a failed turn draws nothing: {:?}",
        seen.events()
    );

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
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let deltas = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();
    assert_eq!(deltas.join(""), "recovered", "the retry landed the stream");
    assert!(
        hits.load(std::sync::atomic::Ordering::SeqCst) >= 3,
        "the provider actually retried"
    );
}

#[tokio::test]
async fn client_errors_fail_through_without_retry() {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let failure = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap_err();
    assert!(
        failure.contains("rejected the request (401)"),
        "the 401 fails through: {failure}"
    );
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
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
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
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let deltas = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();
    // The wire carries the ANSWER and nothing else (a tool result is the model's).
    assert_eq!(
        deltas.join(""),
        "all good",
        "the answer streamed after the tool round"
    );

    // The list rode BOTH rounds: the first request offered it, and the
    // re-open (the next inference round) carried the same affordances.
    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(bodies.len(), 2, "two inference rounds");
    for body in &bodies {
        let request: Value = serde_json::from_str(body).expect("a JSON request");
        let tools = request["tools"]
            .as_array()
            .expect("the tools array is sent");
        let names: Vec<&str> = tools
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            names,
            vec![
                "recall_memory",
                "commit_memory",
                "resolve_skills",
                "invoke_skill",
                "create_skill",
                "open_task",
                "move_task",
                "list_tasks"
            ],
            "{body}"
        );
    }
    // The tool's answer rode back INTO the loop: the re-open carries the
    // assistant's call and the tool message (OpenAI's wire shape), which is
    // the only place a tool result belongs.
    let second: Value = serde_json::from_str(&bodies[1]).expect("a JSON request");
    let tool_message = second["messages"]
        .as_array()
        .expect("messages")
        .iter()
        .find(|message| message["role"] == "tool")
        .expect("the tool result is fed back to the model");
    assert_eq!(tool_message["tool_call_id"], "c1");
    assert!(
        tool_message["content"]
            .as_str()
            .unwrap_or("")
            .contains("outcome"),
        "the ingest's answer reached the model: {tool_message}"
    );

    // And the write LANDED: the graph door shows the entity the tuple named.
    let graph_url = format!("{store_url}/graph/37");
    let graph = tokio::task::spawn_blocking(move || {
        ureq::get(&graph_url)
            .call()
            .unwrap()
            .into_json::<Value>()
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

/// Two requests answered in ONE round: one assistant message carrying both
/// calls, one result per call, and the user still gets an answer.
///
/// The fragments are split the way a real endpoint splits them (id and name
/// first, then slices of the argument document, calls interleaved by `index`),
/// because that is what made this defect invisible: a stub that streams one
/// whole call per delta never exercises either half.
#[tokio::test]
async fn two_calls_in_one_round_reach_the_model_as_one_round() {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let bodies = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
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
                    for fragment in [
                        r#"{"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"open_task","arguments":""}}]}}"#,
                        r#"{"delta":{"tool_calls":[{"index":1,"id":"c2","type":"function","function":{"name":"open_task","arguments":""}}]}}"#,
                        r#"{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"name\":\"alpha\","}}]}}"#,
                        r#"{"delta":{"tool_calls":[{"index":1,"function":{"arguments":"{\"name\":\"beta\"}"}}]}}"#,
                        r#"{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"next\":\"a\"}"}}]}}"#,
                    ] {
                        sse.push_str(&format!("data: {{\"choices\":[{fragment}]}}\n\n"));
                    }
                } else {
                    sse.push_str(r#"data: {"choices":[{"delta":{"content":"both opened"}}]}"#);
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
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let deltas = run_turn(
        &engine,
        &seen,
        turn_args("open alpha and beta"),
    )
    .await
    .unwrap();
    assert_eq!(
        deltas.join(""),
        "both opened",
        "the round's answer reached the user"
    );

    // One assistant message, two calls — not one per call. This is the shape the
    // endpoint reported as malformed by answering an empty string.
    let bodies = bodies.lock().unwrap().clone();
    assert_eq!(
        bodies.len(),
        2,
        "two inference rounds: the round of calls, then the answer"
    );
    let second: Value = serde_json::from_str(&bodies[1]).expect("a JSON request");
    let messages = second["messages"].as_array().expect("messages");
    let assistants: Vec<&Value> = messages
        .iter()
        .filter(|message| message["role"] == "assistant")
        .collect();
    assert_eq!(
        assistants.len(),
        1,
        "the call round is fed back once: {messages:?}"
    );
    let round = assistants[0]["tool_calls"]
        .as_array()
        .expect("the one assistant message carries the calls");
    assert_eq!(round.len(), 2, "and it carries both of them");
    let ids: Vec<&str> = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        ids,
        vec!["c1", "c2"],
        "one result per call, in the calls' order"
    );

    // Both doors ran: the fragments reassembled into two complete argument documents.
    let tasks_url = format!("{store_url}/tasks/37");
    let tasks = tokio::task::spawn_blocking(move || {
        ureq::get(&tasks_url)
            .call()
            .unwrap()
            .into_json::<Value>()
            .unwrap()
    })
    .await
    .unwrap();
    let names: Vec<String> = tasks
        .as_array()
        .expect("a task list")
        .iter()
        .map(|task| task["name"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(names, vec!["alpha", "beta"], "the store kept both: {tasks}");
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
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let failure = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap_err();
    assert!(
        failure.contains("did not converge"),
        "the round cap names itself: {failure}"
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
    let endpoint = sse_server_three_branches(
        vec!["hel", "lo ", "wor", "ld"],
        vec!["compressed ", "span"],
        vec![r#"{"facts": []}"#],
    )
    .await;
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

    let (engine, seen) = boot(&store_url, &endpoint).await;
    let deltas = run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();
    assert_eq!(deltas.join(""), "hello world", "the answer still streams");

    let view = surface.fetch_session(7, 1).await.unwrap();
    assert_eq!(
        view.summary.as_deref(),
        Some("compressed span"),
        "the checkpoint door took the summary the side branch produced"
    );
    assert_eq!(view.coverage_ts, 2_000, "the split k10r proposed");
}

/// The compression's SECOND product: the pass says where the span would have
/// been better cut, the turn resolves that INDEX against the records it numbered,
/// and the checkpoint records it beside the summary without moving the coverage
/// point.
// A blocking ureq call against a mem-server sharing this process would starve a
// single-threaded runtime (the deadlock this suite already knows); two workers
// keep the server live while the assertion's request blocks.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cut_the_pass_chooses_lands_beside_the_summary() {
    let endpoint = sse_server_three_branches(
        vec!["hel", "lo ", "wor", "ld"],
        vec![r#"{"summary": "the span in three lines", "cut": 1}"#],
        vec![r#"{"facts": []}"#],
    )
    .await;
    let store_url = mem_server_with_compression().await;
    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    for (ts, text) in [
        (1_000u64, "first message of the span"),
        (2_000, "second message of the span"),
        (9_000, "third message of the span"),
    ] {
        surface.append_message(7, ts, 0, 0, 0, text).await.unwrap();
    }

    let (engine, seen) = boot(&store_url, &endpoint).await;
    run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();

    let raw: Value = ureq::get(&format!(
        "{store_url}/channels/7/members/1/session?format=plaintext"
    ))
    .call()
    .unwrap()
    .into_json()
    .unwrap();
    assert_eq!(
        raw["summary"], "the span in three lines",
        "the JSON product is unwrapped"
    );
    assert_eq!(
        raw["suggested_ts"], 1_000,
        "the index the model gave, resolved to the record it numbered"
    );
    assert_eq!(
        raw["coverage_ts"], 2_000,
        "the label does NOT move the coverage point: the node closes where the verdict said"
    );
}

/// The review pass (k10r ADR-0017 §4): the second operation the SAME verdict asks
/// for. It reads the span from the cut compression suggested and writes what the
/// conversation did not — through the memory door `commit_memory` uses, so a
/// review needs no vocabulary of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_review_branch_writes_the_facts_it_found() {
    let endpoint = sse_server_three_branches(
        vec!["hel", "lo ", "wor", "ld"],
        vec![r#"{"summary": "a span", "cut": 0}"#],
        vec![r#"{"facts": [["acme","repository",{},"OWNS",{},"issues-api","repository",{}]]}"#],
    )
    .await;
    let store_url = mem_server_with_compression().await;
    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    for (ts, text) in [
        (1_000u64, "first message of the span"),
        (2_000, "second message of the span"),
        (9_000, "third message of the span"),
    ] {
        surface.append_message(7, ts, 0, 0, 0, text).await.unwrap();
    }

    let (engine, seen) = boot(&store_url, &endpoint).await;
    run_turn(&engine, &seen, turn_args("hi"))
        .await
        .unwrap();

    // The fact landed in the graph the review posted it to (scope 37, the turn's
    // `memory_scope`): the proof is a node the conversation never named.
    let graph: Value = ureq::get(&format!("{store_url}/graph/37"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    // `GET /graph/{scope}` answers `{nodes, links}`; the fact is a node the
    // conversation never named, so its presence is the review's own doing.
    let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    assert!(
        nodes
            .iter()
            .any(|node| node["name"].as_str() == Some("acme")),
        "the review's fact reached the graph: {graph}"
    );
}

/// THE VENUE ROUTE, which is the one a page actually uses: `view:turn.submit` with an
/// envelope reaches the booth by subscription, the turn draws without anyone pulling, the
/// frames are addressed to the device the envelope named, and the end is said as its own
/// event.
#[tokio::test]
async fn the_venue_route_draws_a_turn_nobody_pulls() {
    let endpoint = sse_server(vec!["hel", "lo"]).await;
    let store_url = mem_server().await;
    let (engine, seen) = boot(&store_url, &endpoint).await;
    let device = "0f9a7c1e-2b3d-4e5f-8a9b-0c1d2e3f4a5b";

    Realm::emit(
        &engine.realm,
        None,
        "view:turn.submit",
        json!({
            "name": "view:turn.submit",
            "device": device,
            "sender": { "device": device },
            "data": view_args("hi"),
        }),
    )
    .await
    .unwrap();

    // The end is the signal this leg has: the page's submit was an emit, so it cannot read
    // an answer — `view:turn.done` is what stops its streaming state.
    let mut done = None;
    for _ in 0..300 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        done = seen.all("view:turn.done").into_iter().next();
        if done.is_some() {
            break;
        }
    }
    let done = done.expect("the turn said it was done");

    // Every emit carries the DEVICE that asked, and names the event its payload is for.
    // No channel, and no `sender`: the venue excludes a sender from its own event's
    // fan-out, so naming the asker there would send the answer to everyone but the asker.
    //
    // The collector subscribes to the prefix, so it also sees the page's own request
    // (`view:turn.submit`) — that one is the input, not the booth's speech, and the venue
    // drops it from the fan-out because it names a sender.
    let emits: Vec<Value> = seen
        .events()
        .into_iter()
        .filter(|event| event.get("name").and_then(Value::as_str) != Some("view:turn.submit"))
        .collect();
    assert!(emits.len() >= 3, "frames + done: {emits:?}");
    for event in emits {
        assert_eq!(
            event.get("device").and_then(Value::as_str),
            Some(device),
            "an emit is delivered to the device that asked: {event}"
        );
        assert!(
            event.get("sender").is_none(),
            "a booth's own speech names no sender: {event}"
        );
        assert!(
            event
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.starts_with("view:")),
            "a payload the venue can route names its recipient event: {event}"
        );
    }
    assert_eq!(
        done.get("name").and_then(Value::as_str),
        Some("view:turn.done")
    );

    // The frames: the first batch sets the stage (question, region, bubble) and patches the
    // region; every batch is a list of Accrete operations the renderer can draw.
    let batches: Vec<Vec<Value>> = seen
        .all("view:turn")
        .into_iter()
        .filter_map(|e| e.get("data").and_then(Value::as_array).cloned())
        .collect();
    assert_eq!(batches.len(), 2, "one batch per chunk: {batches:?}");
    let first = &batches[0];
    assert_eq!(
        first.len(),
        4,
        "the stage is set in one ordered batch: {first:?}"
    );
    assert_eq!(
        first[0]["slot"], "chat",
        "the user's line lands in the rack"
    );
    assert_eq!(
        first[0]["data"]["children"][0]["bind"]["value"]["default"],
        "hi"
    );
    assert_eq!(first[1]["action"], "set");
    assert_eq!(
        first[1]["slot"], "say",
        "the region is the caller's own name"
    );
    assert_eq!(
        first[2]["data"]["children"][0]["bind"]["value"]["source"],
        "say"
    );
    assert_eq!(first[3]["action"], "patch");
    assert_eq!(first[3]["value"], "hel");
    assert_eq!(batches[1][0]["value"], "lo");

    // And the exchange landed: the stream was live enough to be a turn.
    let surface = gravity_core::surface::HttpSurface::new(&store_url);
    let view = surface.fetch_session(7, 1).await.unwrap();
    assert_eq!(
        view.increment
            .last()
            .map(|(_, row)| row.content.clone())
            .unwrap_or_default(),
        "hello"
    );
}

/// A drawing caller that names no slots is REFUSED by name: gravity owns no layout, so it
/// cannot invent them, and a booth that guessed would draw into a slot the page does not
/// have.
#[tokio::test]
async fn a_drawing_caller_must_name_its_slots() {
    let endpoint = sse_server(vec!["hel"]).await;
    let store_url = mem_server().await;
    let (engine, seen) = boot(&store_url, &endpoint).await;

    let mut args = turn_args("hi");
    args["protocol"] = json!("fluxen");
    let failure = run_turn(&engine, &seen, args).await.unwrap_err();
    assert!(
        failure.contains("needs `slots`"),
        "the refusal names the missing declaration: {failure}"
    );
}

/// A fresh deployment has no rows, and that is a NAMED error (ADR-0007 §5): the turn says which
/// document and which keys are missing, instead of falling back to an environment variable.
#[tokio::test]
async fn a_turn_without_a_settings_row_names_what_is_missing() {
    let (engine, seen) = boot_bare().await;
    let failure = run_turn(&engine, &seen, turn_args("hi")).await.unwrap_err();
    assert!(failure.contains("no settings row"), "{failure}");
    assert!(failure.contains("store_url"), "the refusal names the keys: {failure}");
    assert!(
        failure.contains("default_model"),
        "and the model the settings must point at: {failure}"
    );
}
