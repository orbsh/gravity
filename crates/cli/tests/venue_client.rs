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
            // Two faces on one port, like the venue itself: `/admin` answers plain HTTP and
            // closes, everything else is the gateway's WS upgrade. The client reads the mount
            // table from the first WHILE it holds a turn connection on the second, so the two
            // are served concurrently and this loop runs until the turn connection ends.
            let (done_tx, mut done_rx) = tokio::sync::mpsc::channel::<()>(1);
            loop {
                tokio::select! {
                    Some(()) = done_rx.recv() => return,
                    accepted = listener.accept() => {
                        let (mut stream, _) = accepted.unwrap();
                        // BOTH faces open with `GET /`, so the request line cannot tell them
                        // apart: the WS upgrade carries `Sec-WebSocket-Key` and the page does not.
                        let mut head = vec![0u8; 2048];
                        let mut request = String::new();
                        for _ in 0..100 {
                            let peeked = stream.peek(&mut head).await.unwrap();
                            request = String::from_utf8_lossy(&head[..peeked]).to_ascii_lowercase();
                            if request.contains("\r\n\r\n") {
                                break;
                            }
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                        }
                        if !request.contains("sec-websocket-key") {
                            serve_admin(&mut stream).await.unwrap();
                        } else {
                            let recorder = recorder.clone();
                            let done = done_tx.clone();
                            tokio::spawn(async move {
                                serve(stream, mode, recorder).await.unwrap();
                                let _ = done.send(()).await;
                            });
                        }
                    }
                }
            }
        });
    });
    let addr: SocketAddr = addr_rx.recv().unwrap();
    (format!("ws://{addr}"), seen)
}

/// The venue's own answer (prism ADR-0021). Two mounts for one language, the absent one first in
/// the array: the client must put the CONNECTED one ahead, which is the only ordering fact worth
/// pinning here.
async fn serve_admin(stream: &mut TcpStream) -> anyhow::Result<()> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut scratch = vec![0u8; 1024];
    let _ = stream.read(&mut scratch).await?;
    let body = json!({
        "mounts": [
            { "type": "mac.python", "alias": "mac", "language": "python", "carrier": "exec",
              "runner": "/runners/runner_py_guest.py", "requires": "container", "encoding": "json",
              "registered": true, "node_connected": false },
            { "type": "srv.python", "alias": "srv", "language": "python", "carrier": "exec",
              "runner": "/runners/runner_py_guest.py", "requires": "container", "encoding": "json",
              "registered": true, "node_connected": true }
        ],
        "refused": ["mount [broken.nu] (alias mac) did not register: a refusal the client repeats"]
    })
    .to_string();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
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
        // A cancel is served by the VENUE itself (it holds authority over this
        // connection) and gets NO RECEIPT: what a client can read is the turn ENDING,
        // and a cancelled turn ends with `turn.done` carrying `reason: "cancelled"`
        // (ADR-0008 point 4). The old arm answered `turn.cancel.result`, a frame no
        // venue sends — which is how this stub came to describe a layer that is gone.
        if frame["dest"] == "turn.cancel" {
            send(
                &mut ws,
                json!({ "ev": "turn.done", "args": { "reason": "cancelled", "reply": "partial" } }),
            )
            .await?;
            continue;
        }
        let ev = frame["ev"].as_str().unwrap_or_default().to_string();
        // THE VENUE'S RULE, in the venue's words (prism's auth surface reads `login_name`). This
        // stub used to accept any credential frame, which is how this client came to send its own
        // word for the field and still pass its tests — the smoke script, not the stub, is what
        // found it.
        if matches!(ev.as_str(), "login" | "signup")
            && frame["args"]["login_name"].as_str().is_none()
        {
            send(&mut ws, error("auth `register` needs `login_name`")).await?;
            continue;
        }
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
            "ui:turn.submit" => match mode {
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
    run_cli_in(dir.path(), venue_url, args)
}

/// The same, in an installation the caller keeps: the member id lives beside the config, so two
/// invocations share it only if they share the directory.
fn run_cli_in(dir: &std::path::Path, venue_url: &str, args: &[&str]) -> Output {
    let config = dir.join("gravity.json");
    std::fs::write(
        &config,
        serde_json::to_string(&json!({
            "venue": {
                "url": venue_url,
                "username": "alice",
                "password_env": "GRAVITY_TEST_PW",
                // No mount table here: the client reads the venue's `/admin/mounts` instead, so
                // there is nothing to keep in sync (ADR-0001 §7 as amended, prism ADR-0021).
            }
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
    let submits: Vec<&Value> = frames
        .iter()
        .filter(|f| f["ev"] == "ui:turn.submit")
        .collect();
    assert_eq!(submits.len(), 2, "two submits");
    // The payload a page would send: the conversation, who is speaking, the text and the event its
    // chunks are spoken as on the PLANE. NOT the mount table — that is the booth's to read from the
    // venue it runs inside (ADR-0001 §7 as amended), so this client carries none and a page's turn
    // sees the same table a CLI turn does.
    let args = &submits[0]["args"];
    assert_eq!(args["channel_id"], 1, "the default channel: {args}");
    // WHO this client is: a uuid, not a number — k10r's member id IS the user's uuid, and the
    // client mints one on first run and keeps it (a fresh id per invocation would make every
    // command a different person to the store). The shape is asserted, not a value: the id is the
    // deployment's, and the test's temp dir is a fresh installation.
    let member = args["member_id"].as_str().expect("member_id is a string uuid");
    assert_eq!(member.len(), 36, "uuid text: {member}");
    assert_eq!(member.matches('-').count(), 4, "uuid text: {member}");
    assert_eq!(args["user_message"], "hello", "{args}");
    assert_eq!(args["event"], "ui:turn.delta", "{args}");
    assert!(
        args.get("mounts").is_none(),
        "no copy of the venue's table rides the call: {args}"
    );
    assert_eq!(
        frames.iter().filter(|f| f["ev"] == "signup").count(),
        1,
        "one signup, after the login was refused"
    );
}

/// A CONTEXT IS A SPEAKER OF ITS OWN: `--context` names it and derives its identity from that
/// name, so the same name is the same agent on every run — which is exactly what makes a series of
/// ONE-TURN invocations accumulate into one conversation in the channel. The person who started
/// the process is not the voice of it; that is the whole point of a separate identity.
#[test]
fn a_context_speaks_as_an_agent_derived_from_its_name() {
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--context", "drift check", "hi"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let args = submit_args(&seen);
    assert_eq!(args["member_name"], "drift check", "{args}");
    assert!(
        args.get("supervisor").is_none(),
        "the agent decides who supervises; the client does not carry one: {args}"
    );
    let first = args["member_id"].as_str().unwrap().to_string();
    assert_eq!(first.len(), 36, "a uuid-shaped identity: {first}");

    // The same context again: the same identity, because the derivation is the name.
    let (again_url, again_seen) = venue(Mode::Happy);
    let again = run_cli(&again_url, &["--context", "drift check", "hi"]);
    assert!(again.status.success(), "exit: {:?}", again.status);
    assert_eq!(submit_args(&again_seen)["member_id"], first.as_str());

    // A different context: a different agent.
    let (other_url, other_seen) = venue(Mode::Happy);
    let other = run_cli(&other_url, &["--context", "nightly", "hi"]);
    assert!(other.status.success(), "exit: {:?}", other.status);
    assert_ne!(submit_args(&other_seen)["member_id"], first.as_str());

    // Two ways of saying who speaks are one too many: refused before a socket is opened.
    let (url, seen) = venue(Mode::Happy);
    let both = run_cli(&url, &["--context", "x", "--member", first.as_str(), "hi"]);
    assert_eq!(both.status.code(), Some(2), "usage failure");
    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
}

/// The args of the (single) `ui:turn.submit` a fixture recorded.
fn submit_args(seen: &Arc<Mutex<Vec<Value>>>) -> Value {
    let frames = seen.lock().unwrap().clone();
    frames
        .iter()
        .find(|f| f["ev"] == "ui:turn.submit")
        .expect("one submit")["args"]
        .clone()
}

/// THE MEMBER ID IS AN IDENTITY, so it is minted once and kept: the first run writes one beside
/// the config and says so, later runs read it, and an operator who pastes the account's uuid out
/// of prism changes who the client speaks as without touching a flag.
#[test]
fn the_member_id_is_minted_once_kept_beside_the_config_and_editable() {
    let dir = tempfile::TempDir::new().unwrap();

    let (first_url, first_seen) = venue(Mode::Happy);
    let first = run_cli_in(dir.path(), &first_url, &["hi"]);
    assert!(first.status.success(), "exit: {:?}", first.status);
    let path = dir.path().join("member_id");
    let minted = std::fs::read_to_string(&path).expect("the id was written beside the config")
        .trim()
        .to_string();
    assert_eq!(minted.len(), 36, "uuid text: {minted}");
    let stderr = String::from_utf8(first.stderr).unwrap();
    assert!(stderr.contains("first run"), "a first run says so: {stderr}");
    let used = first_seen.lock().unwrap().clone();
    let submit = used
        .iter()
        .find(|f| f["ev"] == "ui:turn.submit")
        .expect("one submit")
        .clone();
    assert_eq!(submit["args"]["member_id"], minted.as_str(), "{submit}");

    // A second run in the same installation reads the file instead of minting: same person, no
    // second notice, and the file untouched.
    let (second_url, second_seen) = venue(Mode::Happy);
    let second = run_cli_in(dir.path(), &second_url, &["hi"]);
    assert!(second.status.success(), "exit: {:?}", second.status);
    let stderr = String::from_utf8(second.stderr).unwrap();
    assert!(!stderr.contains("first run"), "no second notice: {stderr}");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap().trim(),
        minted,
        "the file is the identity, not a per-run default"
    );
    let used = second_seen.lock().unwrap().clone();
    let submit = used
        .iter()
        .find(|f| f["ev"] == "ui:turn.submit")
        .expect("one submit")
        .clone();
    assert_eq!(submit["args"]["member_id"], minted.as_str(), "{submit}");

    // The file is plain text for a reason: the real answer is the account's uuid from prism.
    std::fs::write(&path, "6f1d3d0e-0000-4000-8000-000000000001\n").unwrap();
    let (third_url, third_seen) = venue(Mode::Happy);
    let third = run_cli_in(dir.path(), &third_url, &["hi"]);
    assert!(third.status.success(), "exit: {:?}", third.status);
    let used = third_seen.lock().unwrap().clone();
    let submit = used
        .iter()
        .find(|f| f["ev"] == "ui:turn.submit")
        .expect("one submit")
        .clone();
    assert_eq!(
        submit["args"]["member_id"], "6f1d3d0e-0000-4000-8000-000000000001",
        "editing the file changes who the client is: {submit}"
    );
}

/// `--session` is the conversation and `--member` is who speaks in it: both ride the call, and a
/// session that is not a channel id is refused before anything is sent.
#[test]
fn the_conversation_and_the_speaker_ride_the_call() {
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--session", "9", "--member", "alice's-own-id", "hi"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let frames = seen.lock().unwrap().clone();
    let submit = frames
        .iter()
        .find(|f| f["ev"] == "ui:turn.submit")
        .expect("one submit");
    assert_eq!(submit["args"]["channel_id"], 9, "{submit}");
    assert_eq!(
        submit["args"]["member_id"], "alice's-own-id",
        "--member wins over the file, verbatim: {submit}"
    );

    // A session that is not a channel id never reaches the socket: usage failure, exit 2.
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--session", "not-a-number", "hi"]);
    assert_eq!(out.status.code(), Some(2), "a bad id is a usage failure");
    assert!(
        seen.lock().unwrap().is_empty(),
        "nothing was sent to the venue"
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
        frames.iter().filter(|f| f["dest"] == "turn.cancel").count(),
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