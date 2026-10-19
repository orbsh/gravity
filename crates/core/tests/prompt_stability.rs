//! Prompt-view byte-stability contract (PLAN Phase 1): the SAME session
//! state fetched and assembled TWICE must produce byte-identical
//! message lists — context-cache correctness depends on it (the LLM
//! provider's prefix KV cache keys on the exact prompt bytes; a view
//! that reorders, reformats, or re-timestamps between turns silently
//! invalidates the cache and pays full price per token).

use gravity_core::llm::WireMessage;
use gravity_core::surface::{InProcessSurface, MemorySurface};
use mem_core::channel::{kind, role};
use mem_core::FactTuple;

/// The head's corpus: one fact about the default user node (node 1 of
/// scope 1), so the head block participates in the byte comparison.
fn user_fact() -> FactTuple {
    FactTuple::from_json(r#"["用户","person",{},"PREFERS",{},"简洁","style",{}]"#).unwrap()
}

/// The prompt assembly, verbatim from the seam test's stub (the real
/// `turn()` collapses this when Phase 1's injectable transport lands —
/// until then the copy IS the contract under test; the assertion below
/// pins what "stable" means for whoever reimplements it).
fn assemble(
    view: &gravity_core::surface::SessionProjection,
    user_message: &str,
) -> Vec<WireMessage> {
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
        content: user_message.to_string(),
    });
    messages
}

fn render(messages: &[WireMessage]) -> String {
    messages
        .iter()
        .map(|m| format!("{}:\x1f{}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\x1e")
}

#[tokio::test]
async fn same_state_assembles_byte_identical_views() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut stores = mem_core::open_service(dir.path()).unwrap();
    stores.graph.ingest(1, &user_fact()).unwrap();
    let surface = InProcessSurface::wrap(stores);
    assert!(
        !surface.fetch_session(7, 1).await.unwrap().head.is_empty(),
        "the head is part of the bytes under test"
    );

    // A conversation with every role the assembly maps (user, assistant,
    // tool) — the richest stable view.
    surface
        .append_message(7, 100, 0, role::USER, kind::TEXT, "first question")
        .await
        .unwrap();
    surface
        .append_message(7, 100, 1, role::ASSISTANT, kind::TEXT, "first answer")
        .await
        .unwrap();
    surface
        .append_message(7, 200, 0, role::TOOL, kind::TEXT, "tool observation")
        .await
        .unwrap();

    let v1 = surface.fetch_session(7, 1).await.unwrap();
    let p1 = render(&assemble(&v1, "second question"));

    // Fetch + assemble again — NO state change between.
    let v2 = surface.fetch_session(7, 1).await.unwrap();
    let p2 = render(&assemble(&v2, "second question"));
    assert_eq!(p1, p2, "same state must assemble byte-identical prompts");

    // And through the HTTP seam — the deployment form must not reformat
    // (the wire DTO round trip rides the same field order). HTTP owns
    // its own store (one fjall directory = one process, ADR-0007), so
    // this drives the SAME logical conversation through the wire and
    // compares the assembled bytes.
    let dir2 = tempfile::TempDir::new().unwrap();
    let app = mem_server::open(dir2.path().to_path_buf()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = gravity_core::surface::HttpSurface::new(&format!("http://{addr}"));
    let status = reqwest::Client::new()
        .post(format!("http://{addr}/facts/1"))
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
    http.append_message(7, 100, 0, role::USER, kind::TEXT, "first question")
        .await
        .unwrap();
    http.append_message(7, 100, 1, role::ASSISTANT, kind::TEXT, "first answer")
        .await
        .unwrap();
    http.append_message(7, 200, 0, role::TOOL, kind::TEXT, "tool observation")
        .await
        .unwrap();
    let v3 = http.fetch_session(7, 1).await.unwrap();
    let p3 = render(&assemble(&v3, "second question"));
    assert_eq!(
        p1, p3,
        "the HTTP seam assembles the same bytes as in-process"
    );
}

#[tokio::test]
async fn appended_history_is_prefix_stable() {
    // The NEXT turn's prompt must start with the previous prompt's
    // content (prefix cache hits ride this): append an exchange, the
    // old messages keep their exact bytes, only additions follow.
    let dir = tempfile::TempDir::new().unwrap();
    let surface = InProcessSurface::open(dir.path()).unwrap();

    surface
        .append_message(7, 100, 0, role::USER, kind::TEXT, "q1")
        .await
        .unwrap();
    let v1 = surface.fetch_session(7, 1).await.unwrap();
    let p1 = render(&assemble(&v1, "a1"));

    // The turn lands: user + assistant appended (one ts, subs 0/1 —
    // a fresh ts, the same-key overwrite is the caller's bug).
    surface
        .append_message(7, 150, 0, role::USER, kind::TEXT, "a1")
        .await
        .unwrap();
    surface
        .append_message(7, 150, 1, role::ASSISTANT, kind::TEXT, "r1")
        .await
        .unwrap();
    let v2 = surface.fetch_session(7, 1).await.unwrap();
    let p2 = render(&assemble(&v2, "q2"));

    assert!(
        p2.starts_with(&p1),
        "history is append-only: the previous prompt is a byte prefix\n--- p1:\n{p1}\n--- p2:\n{p2}"
    );
}

#[tokio::test]
async fn checkpoint_compression_rewrites_the_view_deliberately() {
    // The one legal way the prompt bytes change: checkpoint compression
    // swaps [H] for [S] — an EXPLICIT memory-system decision, not view
    // instability. This test documents the boundary: pre-checkpoint
    // prompts are stable, the swap happens at the watermark, and the
    // post-checkpoint view is stable again. (The checkpoint is a raw
    // mem-core call — gravity never writes checkpoints; this stands in
    // for Krystallizer's compression side.)
    let dir = tempfile::TempDir::new().unwrap();
    let surface = InProcessSurface::open(dir.path()).unwrap();
    surface
        .append_message(7, 100, 0, role::USER, kind::TEXT, "old q")
        .await
        .unwrap();
    surface
        .append_message(7, 100, 1, role::ASSISTANT, kind::TEXT, "old a")
        .await
        .unwrap();
    surface
        .append_message(7, 200, 0, role::USER, kind::TEXT, "new q")
        .await
        .unwrap();
    let before = surface.fetch_session(7, 1).await.unwrap();
    let p_before = render(&assemble(&before, "next"));

    // Now the compression side's write, through a fresh handle on the
    // SAME directory (InProcessSurface holds the store; a second open
    // would contend on the fjall lock — so drop and reopen).
    drop(surface);
    let mut stores = mem_core::open_service(dir.path()).unwrap();
    stores
        .channels
        .append_checkpoint(7, 1, 100, "compressed summary");
    let surface2 = InProcessSurface::wrap(stores);
    let after = surface2.fetch_session(7, 1).await.unwrap();
    let p_after = render(&assemble(&after, "next"));

    assert_ne!(p_before, p_after, "the swap is the deliberate rewrite");
    assert!(
        !p_after.contains("old q"),
        "compressed history left the increment"
    );
    assert!(p_after.contains("compressed summary"));
    assert!(p_after.contains("new q"), "post-checkpoint messages stay");
}
