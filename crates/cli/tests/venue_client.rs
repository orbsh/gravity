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

/// The same, in an installation the caller keeps (a config file of its own): the tag is the
/// identity, so the directory does not decide who runs.
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
    let out = run_cli(&url, &["--tag", "smoke", "--json", "hello", "again"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let stdout = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "two turns, one JSON line each: {stdout}");
    for line in lines {
        let value: Value = serde_json::from_str(line).unwrap();
        assert_eq!(value["reply"], "hel lo");
        // No `--session`: the channel followed the SPEAKER (k10r's `solo` line), so the client has
        // no id to echo — `null` is the honest answer, and the store chose.
        assert!(value["session"].is_null(), "unpinned session: {value}");
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
    // ABSENT, not `1`: a caller that does not pin a conversation sends no channel at all, and the
    // booth resolves the speaker's own line from k10r. A default id here was the client guessing.
    assert!(
        args.get("channel_id").is_none(),
        "the channel follows the speaker: {args}"
    );
    // WHO this client is: a uuid derived from the tag, and the same uuid on every run — a fresh id
    // per invocation would make every command a different agent. The shape is asserted, not a
    // value; `a_tag_speaks_as_an_agent_derived_from_its_name` pins the value's stability.
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

/// THE TAG IS THE CALLER'S WHOLE IDENTITY: `--tag` names the automation and its member id is
/// derived from that name, so the same tag is the same agent on every run — which is exactly what
/// makes a series of ONE-TURN invocations (a cron job, a CI step) accumulate into one history
/// instead of a new one each time. Nothing is stored: a fresh checkout cannot come up as a
/// different speaker.
#[test]
fn a_tag_speaks_as_an_agent_derived_from_its_name() {
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--tag", "drift check", "hi"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let args = submit_args(&seen);
    assert_eq!(args["member_name"], "drift check", "{args}");
    assert!(
        args.get("supervisor").is_none(),
        "the agent decides who supervises; the client does not carry one: {args}"
    );
    let first = args["member_id"].as_str().unwrap().to_string();
    assert_eq!(first.len(), 36, "a uuid-shaped identity: {first}");

    // The same tag again: the same identity, because the derivation is the name.
    let (again_url, again_seen) = venue(Mode::Happy);
    let again = run_cli(&again_url, &["--tag", "drift check", "hi"]);
    assert!(again.status.success(), "exit: {:?}", again.status);
    assert_eq!(submit_args(&again_seen)["member_id"], first.as_str());

    // A different tag: a different agent.
    let (other_url, other_seen) = venue(Mode::Happy);
    let other = run_cli(&other_url, &["--tag", "nightly", "hi"]);
    assert!(other.status.success(), "exit: {:?}", other.status);
    assert_ne!(submit_args(&other_seen)["member_id"], first.as_str());

    // A caller that will not name itself is refused before a socket is opened: an unnamed caller
    // has no line to continue, and guessing one would be the client inventing an identity.
    let (url, seen) = venue(Mode::Happy);
    let nameless = run_cli(&url, &["hi"]);
    assert_eq!(nameless.status.code(), Some(2), "usage failure");
    assert!(seen.lock().unwrap().is_empty(), "nothing was sent");
    let stderr = String::from_utf8(nameless.stderr).unwrap();
    assert!(stderr.contains("--tag"), "the refusal names the flag: {stderr}");
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

/// `--session` PINS the conversation (absent = the speaker's own line): a pinned session rides the
/// call, and one that is not a channel id is refused before anything is sent.
#[test]
fn the_conversation_and_the_speaker_ride_the_call() {
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--tag", "nightly", "--session", "9", "hi"]);
    assert!(out.status.success(), "exit: {:?}", out.status);
    let frames = seen.lock().unwrap().clone();
    let submit = frames
        .iter()
        .find(|f| f["ev"] == "ui:turn.submit")
        .expect("one submit");
    assert_eq!(submit["args"]["channel_id"], 9, "{submit}");
    assert_eq!(submit["args"]["member_name"], "nightly", "{submit}");
    assert_eq!(
        submit["args"]["member_id"].as_str().unwrap().len(),
        36,
        "the speaker is derived from the tag, not typed: {submit}"
    );

    // A session that is not a channel id never reaches the socket: usage failure, exit 2.
    let (url, seen) = venue(Mode::Happy);
    let out = run_cli(&url, &["--tag", "smoke", "--session", "not-a-number", "hi"]);
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
    let out = run_cli(&url, &["--tag", "smoke", "hi"]);
    assert_eq!(out.status.code(), Some(4));
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert!(stderr.contains("the provider died"), "stderr: {stderr}");
}

/// Neither login nor signup accepted = exit 3.
#[test]
fn a_refused_auth_exits_3() {
    let (url, _seen) = venue(Mode::AuthRefused);
    let out = run_cli(&url, &["--tag", "smoke", "hi"]);
    assert_eq!(out.status.code(), Some(3));
}

/// The cancel path sends the venue's own verb (Ctrl-C / --cancel-after-ms),
/// and a cancelled turn is not a completed one: exit 4 with the venue's word.
#[test]
fn cancel_sends_the_venue_verb_and_ends_the_turn() {
    let (url, seen) = venue(Mode::SlowTurn);
    let out = run_cli(&url, &["--tag", "smoke", "--cancel-after-ms", "100", "hi"]);
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
    let out = run_cli(&format!("ws://{addr}"), &["--tag", "smoke", "hi"]);
    assert_eq!(out.status.code(), Some(5));
}