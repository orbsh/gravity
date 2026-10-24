//! Turn-loop tests with a stubbed LLM: the same assertions run against
//! BOTH memory surfaces — in-process (real stores on a temp dir) and HTTP
//! (real mem-server on a local port) — pinning the seam contract: the turn
//! function must not behave differently across the deployment forms.
//!
//! The projection the seam hands out is k10r's own (ADR-0010 §5): the head
//! arrives RENDERED, and an increment entry carries the LLM role NAME. Both
//! surfaces must produce the same one — that is what the head test drives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unwrap_in_result, clippy::panic_in_result_fn)]

use gravity_core::llm::WireMessage;
use gravity_core::surface::{InProcessSurface, MemorySurface};
use gravity_core::TurnInput;
use mem_core::channel::{kind, role};
use mem_core::FactTuple;

/// The turn body, parameterized over the reply — a test-local copy so
/// the LLM call is stubbed while the memory-seam behavior (fetch →
/// append on success only, one ts two subs) is the real code under
/// test. `turn()`'s signature takes `&LlmClient` with no injectable
/// transport, so the loop cannot drive a stub through it yet; when
/// Phase 1 grows the injectable LLM transport (provider booth /
/// byte-stability contract test), this copy collapses into the real
/// `turn()` and only the stub remains.
async fn turn_with(
    store: &dyn MemorySurface,
    reply: &str,
    fail: bool,
    input: TurnInput,
) -> anyhow::Result<String> {
    let view = store
        .fetch_session(input.channel_id, input.member_id)
        .await?;

    let mut messages: Vec<WireMessage> = Vec::new();
    if !view.head.is_empty() {
        messages.push(WireMessage {
            role: "system".into(),
            content: view.head.clone(),
        });
    }
    if let Some(summary) = &view.summary {
        messages.push(WireMessage {
            role: "system".into(),
            content: format!("Conversation summary so far:\n{summary}"),
        });
    }
    for (_, row) in &view.increment {
        messages.push(WireMessage {
            role: row.role.clone().unwrap_or_else(|| "user".into()),
            content: row.content.clone(),
        });
    }
    messages.push(WireMessage {
        role: "user".into(),
        content: input.user_message.clone(),
    });
    assert!(!messages.is_empty());

    if fail {
        anyhow::bail!("LLM down");
    }
    let reply = reply.to_string();

    let ts = gravity_core::now_ms();
    store
        .append_message(
            input.channel_id,
            ts,
            0,
            role::USER,
            kind::TEXT,
            &input.user_message,
        )
        .await?;
    store
        .append_message(input.channel_id, ts, 1, role::ASSISTANT, kind::TEXT, &reply)
        .await?;
    Ok(reply)
}

/// Shared assertions over any surface: success appends the delta with
/// one ts / two subs; failure appends nothing; the fetched view feeds
/// prompt assembly (roles + contents round-trip).
async fn seam_contract(store: &dyn MemorySurface) {
    let ch = 7;
    let m = gravity_core::surface::DEFAULT_MEMBER;

    // A failed turn leaves no trace.
    let err = turn_with(
        store,
        "unused",
        true,
        TurnInput {
            channel_id: ch,
            member_id: m,
            user_message: "ghost".into(),
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("LLM down"));
    let view = store.fetch_session(ch, m).await.unwrap();
    assert!(view.increment.is_empty(), "failed turn must not append");

    // A successful turn appends user+assistant under one ts.
    turn_with(
        store,
        "hello!",
        false,
        TurnInput {
            channel_id: ch,
            member_id: m,
            user_message: "hi".into(),
        },
    )
    .await
    .unwrap();
    let view = store.fetch_session(ch, m).await.unwrap();
    assert_eq!(view.increment.len(), 2);
    assert_eq!(
        view.increment[0].0.ts_ms, view.increment[1].0.ts_ms,
        "one exchange, one ts"
    );
    assert_eq!(view.increment[0].0.sub, 0);
    assert_eq!(view.increment[1].0.sub, 1);
    // The role NAME is k10r's mapping — the seam never sees the stored byte.
    assert_eq!(view.increment[0].1.role.as_deref(), Some("user"));
    assert_eq!(view.increment[1].1.role.as_deref(), Some("assistant"));
    assert_eq!(view.increment[0].1.kind, kind::TEXT);
    assert_eq!(view.increment[0].1.content, "hi");
    assert_eq!(view.increment[1].1.content, "hello!");
}

#[tokio::test]
async fn in_process_surface_contract() {
    let dir = tempfile::TempDir::new().unwrap();
    let surface = InProcessSurface::open(dir.path()).unwrap();
    seam_contract(&surface).await;
}

#[tokio::test]
async fn http_surface_contract() {
    // Boot a real mem-server on an OS port; drive the SAME assertions
    // through the HTTP seam.
    let dir = tempfile::TempDir::new().unwrap();
    let app = mem_server::open(dir.path().to_path_buf()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let surface = gravity_core::surface::HttpSurface::new(&format!("http://{addr}"));
    seam_contract(&surface).await;
}

/// The head is TEXT, rendered by k10r, on BOTH surfaces: the same corpus
/// answers the same block, and the block is the user node's one fact.
///
/// This is the contract that broke when the projection's head stopped being
/// a fact array (2026-10-20) — a caller that decoded it as data (or that
/// tried to render it itself) is exactly what these assertions catch.
#[tokio::test]
async fn head_is_rendered_by_both_surfaces() {
    let tuple =
        || FactTuple::from_json(r#"["用户","person",{},"PREFERS",{},"简洁","style",{}]"#).unwrap();

    // In-process: the fact lands through the service's own graph handle
    // (the surface has no ingest — that is the caller's write door).
    let dir = tempfile::TempDir::new().unwrap();
    let mut stores = mem_core::open_service(dir.path()).unwrap();
    stores.graph.ingest(1, &tuple()).unwrap();
    let in_process = InProcessSurface::wrap(stores);
    let local = in_process
        .fetch_session(7, gravity_core::surface::DEFAULT_MEMBER)
        .await
        .unwrap();

    // HTTP: the same corpus through the door.
    let dir2 = tempfile::TempDir::new().unwrap();
    let app = mem_server::open(dir2.path().to_path_buf()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}");
    let status = reqwest::Client::new()
        .post(format!("{base}/memories/1"))
        .json(&serde_json::json!([
            "用户",
            "person",
            {},
            "PREFERS",
            {},
            "简洁",
            "style",
            {}
        ]))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, reqwest::StatusCode::OK);
    let http = gravity_core::surface::HttpSurface::new(&base);
    let remote = http
        .fetch_session(7, gravity_core::surface::DEFAULT_MEMBER)
        .await
        .unwrap();

    assert_eq!(
        local.head, remote.head,
        "both surfaces hand back k10r's rendering, byte for byte"
    );
    assert!(
        local.head.contains("用户 (person) -[PREFERS]-> 简洁"),
        "the head renders the user node's fact: {}",
        local.head
    );
}

/// The checkpoint path rides the same seam: a summary set through the
/// in-process store shows up in the view's summary field and prunes the
/// increment.
#[tokio::test]
async fn checkpoint_flows_through_surface() {
    let dir = tempfile::TempDir::new().unwrap();
    let surface = InProcessSurface::open(dir.path()).unwrap();
    for (ts, content) in [(100u64, "old"), (200, "new")] {
        surface
            .append_message(3, ts, 0, role::USER, kind::TEXT, content)
            .await
            .unwrap();
    }
    drop(surface);

    // The compression side's write: a fresh handle on the SAME directory
    // (Fjall locks it, so the surface must be dropped first).
    let mut stores = mem_core::open_service(dir.path()).unwrap();
    stores
        .channels
        .append_checkpoint(3, (100, 0), "compressed", &[]);
    let surface = InProcessSurface::wrap(stores);
    let view = surface
        .fetch_session(3, gravity_core::surface::DEFAULT_MEMBER)
        .await
        .unwrap();
    assert_eq!(view.coverage_ts, 100);
    assert_eq!(view.summary.as_deref(), Some("compressed"));
    assert_eq!(view.increment.len(), 1);
    assert_eq!(view.increment[0].1.content, "new");
}
