//! The CLI is a CLIENT of the venue (ADR-0002): these tests pin the client's
//! side of the contract — the frames it sends, the answers it reads, and the
//! exit code each failure class earns — against a hand-rolled WS venue. The
//! server's own end-to-end path (WS → gateway → gravity's booth → k10r) is
//! prism's `turn_e2e`; nothing here re-implements it.
//!
//! The venue runs on its OWN thread with its own runtime: the test body blocks
//! on the CLI process (`Command::output`), which would starve a shared
//! current-thread runtime and deadlock the fixture.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unwrap_in_result, clippy::panic_in_result_fn)]

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::process::Output;
use std::sync::{Arc, Mutex};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Login is refused (fresh venue), signup succeeds, the turn streams and
    /// closes with a value.
    Happy,
    /// The turn dies: `turn.error`.
    TurnError,
    /// Neither login nor signup is accepted.
    AuthRefused,
    /// The turn never completes on its own: it streams one delta and waits,
    /// so the client's cancel path is what ends it.
    SlowTurn,
}

/// A venue that speaks the four frames the CLI uses, recording what it was
/// sent so a test can assert the CLIENT said the right thing.
fn venue(mode: Mode) -> (String, Arc<Mutex<Vec<Value>>>) {
    let (addr_tx, addr_rx) = std::sync::mpsc::channel();
    let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async move {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            addr_tx.send(listener.local_addr().unwrap()).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, mode, recorder).await.unwrap();
        });
    });
    let addr: SocketAddr = addr_rx.recv().unwrap();
    (format!("ws://{addr}"), seen)
}

async fn serve(
    stream: TcpStream,
    mode: Mode,
    seen: Arc<Mutex<Vec<Value>>>,
) -> anyhow::Result<()> {
    let mut ws = tokio_tungstenite::accept_async(stream).await?;
    send(&mut ws, json!({ "ev": "connected", "args": {} })).await?;
    while let Some(message) = ws.next().await {
        let message = message?;
        let Message::Text(text) = message else {
            continue;
        };
        let frame: Value = serde_json::from_str(text.as_str())?;
        seen.lock().unwrap().push(frame.clone());
        let ev = frame["ev"].as_str().unwrap_or_default().to_string();
        match ev.as_str() {
            // A fresh venue: no account yet. The CLI's fallback is signup.
            "login" => send(&mut ws, error("no account for that name")).await?,
            "signup" => {
                if mode == Mode::AuthRefused {
                    send(&mut ws, error("signup refused")).await?;
                } else {
                    send(
                        &mut ws,
                        json!({ "ev": "signup.result", "args": { "user_id": 7 } }),
                    )
                    .await?;
                }
            }
            "turn.submit" => match mode {
                Mode::TurnError => {
                    send(
                        &mut ws,
                        json!({ "ev": "turn.error", "args": { "message": "the provider died" } }),
                    )
                    .await?
                }
                Mode::SlowTurn => {
                    send(
                        &mut ws,
                        json!({ "ev": "turn.delta", "args": { "delta": "partial" } }),
                    )
                    .await?;
                }
                _ => {
                    send(
                        &mut ws,
                        json!({ "ev": "turn.delta", "args": { "delta": "hel " } }),
                    )
                    .await?;
                    send(
                        &mut ws,
                        json!({ "ev": "turn.delta", "args": { "delta": "lo" } }),
                    )
                    .await?;
                    send(
                        &mut ws,
                        json!({ "ev": "turn.done", "args": { "value": { "reply": "hel lo" } } }),
                    )
                    .await?;
                }
            },
            "turn.cancel" => {
                send(
                    &mut ws,
                    json!({ "ev": "turn.cancel.result", "args": { "ok": true } }),
                )
                .await?;
                // What the venue's pump reports when the stream it was
                // reading dies under it.
                send(
                    &mut ws,
                    json!({ "ev": "turn.error", "args": { "message": "turn cancelled" } }),
                )
                .await?;
            }
            _ => {}
        }
    }
    Ok(())
}

async fn send(
    ws: &mut tokio_tungstenite::WebSocketStream<TcpStream>,
    frame: Value,
) -> anyhow::Result<()> {
    ws.send(Message::Text(frame.to_string())).await?;
    Ok(())
}

fn error(message: &str) -> Value {
    json!({ "ev": "error", "args": { "ev": "login", "message": message } })
}

/// Run the CLI against `venue_url` with a config file of its own.
fn run_cli(venue_url: &str, args: &[&str]) -> Output {
    let dir = tempfile::TempDir::new().unwrap();
    let config = dir.path().join("gravity.json");
    std::fs::write(
        &config,
        serde_json::to_string(&json!({
            "venue": { "url": venue_url, "username": "alice", "password_env": "GRAVITY_TEST_PW" }
        }))
        .unwrap(),
    )
    .unwrap();
    std::process::Command::new(env!("CARGO_BIN_EXE_gravity"))
        .env("GRAVITY_CONFIG", &config)
        .env("GRAVITY_TEST_PW", "secret")
        .args(args)
        .output()
        .unwrap()
}

/// One command = one call (ADR-0002 §2): the reply comes back on stdout, and a
/// second positional argument is a SECOND turn on the same connection.
#[test]
fn one_command_is_one_turn_per_argument() {
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--json", "hello", "again"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "two turns, one JSON line each: {stdout}");
    for line in lines {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["reply"], "hel lo");
        assert_eq!(value["session"], "1");
    }
    let frames = seen.lock().unwrap().clone();
    assert_eq!(
        frames.iter().filter(|f| f["ev"] == "turn.submit").count(),
        2,
        "two submits"
    );
    assert_eq!(
        frames.iter().filter(|f| f["ev"] == "signup").count(),
        1,
        "one signup, after the login was refused"
    );
}

/// A turn the venue reports as failed is exit 4, never a zero with an empty
/// answer.
#[test]
fn a_failed_turn_exits_4() {
    let (url, _seen) = venue(Mode::TurnError);
    let out = run_cli(&url, &["hi"]);
    assert_eq!(out.status.code(), Some(4));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("the provider died"), "stderr: {stderr}");
}

/// Neither login nor signup accepted = exit 3.
#[test]
fn a_refused_auth_exits_3() {
    let (url, _seen) = venue(Mode::AuthRefused);
    let out = run_cli(&url, &["hi"]);
    assert_eq!(out.status.code(), Some(3));
}

/// The cancel path sends the venue's own verb (Ctrl-C / --cancel-after-ms),
/// and a cancelled turn is not a completed one: exit 4 with the venue's word.
#[test]
fn cancel_sends_the_venue_verb_and_ends_the_turn() {
    let (url, seen) = venue(Mode::SlowTurn);
    let out = run_cli(&url, &["--cancel-after-ms", "100", "hi"]);
    assert_eq!(out.status.code(), Some(4));
    let frames = seen.lock().unwrap().clone();
    assert_eq!(
        frames.iter().filter(|f| f["ev"] == "turn.cancel").count(),
        1,
        "the client cancelled through the protocol, not by dropping the socket"
    );
}

/// An unreachable venue is exit 5 — again, never a silent zero.
#[test]
fn an_unreachable_venue_exits_5() {
    // Bind and drop: the port is closed, so the connect is refused.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let out = run_cli(&format!("ws://{addr}"), &["hi"]);
    assert_eq!(out.status.code(), Some(5));
}