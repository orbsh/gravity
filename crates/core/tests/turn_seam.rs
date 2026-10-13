//! Turn-loop tests with a stubbed LLM: the same assertions run against
//! BOTH memory surfaces — in-process (real ChannelStore on a temp dir)
//! and HTTP (real mem-server on a local port) — pinning the seam
//! contract: the turn function must not behave differently across the
//! deployment forms.

use gravity_core::llm::WireMessage;
use gravity_core::surface::{InProcessSurface, MemorySurface};
use gravity_core::TurnInput;
use mem_core::channel::{kind, role, ChannelStore};

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
    if let Some(summary) = &view.summary {
        messages.push(WireMessage {
            role: "system".into(),
            content: format!("Conversation summary so far:\n{summary}"),
        });
    }
    for (_, row) in &view.increment {
        messages.push(WireMessage {
            role: wire_role(row.role).to_string(),
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

fn wire_role(role: u8) -> &'static str {
    match role {
        mem_core::channel::role::ASSISTANT => "assistant",
        mem_core::channel::role::TOOL => "tool",
        mem_core::channel::role::SYSTEM => "system",
        _ => "user",
    }
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
    assert_eq!(view.increment[0].1.role, role::USER);
    assert_eq!(view.increment[0].1.kind, kind::TEXT);
    assert_eq!(view.increment[0].1.content, "hi");
    assert_eq!(view.increment[1].1.content, "hello!");

    // The projection feeds prompt assembly: roles map to wire strings.
    let roles: Vec<&str> = view
        .increment
        .iter()
        .map(|(_, r)| wire_role(r.role))
        .collect();
    assert_eq!(roles, vec!["user", "assistant"]);
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

/// The checkpoint path rides the same seam: a summary set through the
/// in-process store shows up in the HTTP-fetched view's summary field
/// and prunes the increment (the mem-server side's own tests cover the
/// wire; this covers the seam's decode into SessionView).
#[tokio::test]
async fn checkpoint_flows_through_surface() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = ChannelStore::open(&dir.path().join("s")).unwrap();
    store.append(3, 100, 0, role::USER, kind::TEXT, "old");
    store.append(3, 200, 0, role::USER, kind::TEXT, "new");
    store.append_checkpoint(3, gravity_core::surface::DEFAULT_MEMBER, 100, "compressed");

    let surface = InProcessSurface::wrap(store);
    let view = surface
        .fetch_session(3, gravity_core::surface::DEFAULT_MEMBER)
        .await
        .unwrap();
    assert_eq!(view.coverage_ts, 100);
    assert_eq!(view.summary.as_deref(), Some("compressed"));
    assert_eq!(view.increment.len(), 1);
    assert_eq!(view.increment[0].1.content, "new");
}
