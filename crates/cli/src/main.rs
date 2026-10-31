//! gravity — the CLI is a CLIENT of the venue (ADR-0002): kubectl-shaped,
//! one command = one call, stdin/stdout, meaningful exit codes, no session
//! state of its own.
//!
//! It speaks the venue's ONE wire (prism ADR-0017): it EMITS `ui:turn.submit` and reads the
//! application's own speech — `turn.delta`*, `turn.done` / `turn.error` — with the `ui:` marker
//! already stripped by the venue. The name it types for its chunks is the PLANE name
//! (`ui:turn.delta`) because that is what the prefix route carries; what comes back is the plain
//! name, which is what this client reads. There is no second RPC surface and no CLI-specific verb.
//! The turn itself runs in the booth, in the venue: this binary holds no memory, no LLM and no
//! session.
//!
//! Configuration: `GRAVITY_CONFIG` (default `./gravity.json`). The file must
//! carry a `venue` node (`url`, `username`, `password_env`); the password is
//! read from the env var that field NAMES, never from the file.
//!
//! Exit codes (ADR-0002 §2): 0 turn done, 2 usage/config, 3 auth, 4 the turn
//! failed, 5 the venue could not be reached or the protocol broke.

use futures_util::{SinkExt, StreamExt};
use gravity_config::{GravityConfig, VenueConfig};
use serde_json::{json, Value};
use std::io::Write;
use std::time::Duration;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const USAGE: &str = "\
usage: gravity [--json] [--session <id>] [--venue <ws-url>] [--cancel-after-ms <n>] <text>...

  <text>...           one turn per argument; `-` reads stdin as ONE turn
  --json              one JSON object per turn on stdout (scripts); the
                      readable form streams the deltas as they arrive
  --session <id>      the conversation this call speaks in (k10r's channel id,
                      a number; default 1 — the client keeps no state, ADR-0002 §3)
  --member <id>       who is speaking in it (k10r's member id; default 1)
  --venue <ws-url>    override the configured venue URL
  --cancel-after-ms   send `ui:turn.cancel` after n ms (the automation form of
                      Ctrl-C; Ctrl-C does the same at any moment)

exit codes: 0 done, 2 usage/config, 3 auth, 4 turn failed, 5 venue unreachable";

#[tokio::main]
async fn main() {
    let cli = match parse_args(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(failure) => {
            eprintln!("gravity> {}", failure.message());
            eprintln!("{USAGE}");
            std::process::exit(failure.code());
        }
    };
    if let Err(failure) = run(cli).await {
        eprintln!("gravity> {}", failure.message());
        std::process::exit(failure.code());
    }
}

/// The four failure classes ADR-0002 gives exit codes to.
enum Failure {
    Usage(String),
    Link(String),
    Auth(String),
    Turn(String),
}

impl Failure {
    fn code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Link(_) => 5,
            Self::Auth(_) => 3,
            Self::Turn(_) => 4,
        }
    }

    fn message(&self) -> &str {
        match self {
            Self::Usage(message)
            | Self::Link(message)
            | Self::Auth(message)
            | Self::Turn(message) => message,
        }
    }
}

struct Cli {
    json: bool,
    /// The conversation, as the operator typed it (echoed in `--json` output).
    session: String,
    /// The same value as a channel id: k10r's channel is a NUMBER, so a session that is not one is
    /// a usage failure, refused before a socket is opened.
    channel: u64,
    member: u64,
    venue: Option<String>,
    cancel_after_ms: Option<u64>,
    turns: Vec<String>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, Failure> {
    let mut json = false;
    let mut session = "1".to_string();
    let mut member = 1u64;
    let mut venue = None;
    let mut cancel_after_ms = None;
    let mut turns = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--session" => {
                session = args
                    .next()
                    .ok_or_else(|| Failure::Usage("--session needs an id".into()))?;
            }
            "--member" => {
                let raw = args
                    .next()
                    .ok_or_else(|| Failure::Usage("--member needs an id".into()))?;
                member = raw
                    .parse()
                    .map_err(|_| Failure::Usage(format!("--member: `{raw}` is not a number")))?;
            }
            "--venue" => {
                venue = Some(
                    args.next()
                        .ok_or_else(|| Failure::Usage("--venue needs a ws:// URL".into()))?,
                );
            }
            "--cancel-after-ms" => {
                let raw = args
                    .next()
                    .ok_or_else(|| Failure::Usage("--cancel-after-ms needs a number".into()))?;
                cancel_after_ms = Some(raw.parse().map_err(|_| {
                    Failure::Usage(format!("--cancel-after-ms: `{raw}` is not a number"))
                })?);
            }
            "-" => turns.push(read_stdin()?),
            other if other.starts_with("--") => {
                return Err(Failure::Usage(format!("unknown option `{other}`")));
            }
            other => turns.push(other.to_string()),
        }
    }
    if turns.is_empty() {
        return Err(Failure::Usage(
            "no input: pass text, or `-` to read stdin".into(),
        ));
    }
    let channel: u64 = session.parse().map_err(|_| {
        Failure::Usage(format!(
            "--session: `{session}` is not a channel id (k10r's channel is a number)"
        ))
    })?;
    Ok(Cli {
        json,
        session,
        channel,
        member,
        venue,
        cancel_after_ms,
        turns,
    })
}

/// `-` reads the whole of stdin as ONE turn: a pipeline is one prompt, not one
/// turn per line (a file of prose is not a session).
fn read_stdin() -> Result<String, Failure> {
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|e| Failure::Usage(format!("reading stdin: {e}")))?;
    let text = text.trim().to_string();
    if text.is_empty() {
        return Err(Failure::Usage("stdin was empty".into()));
    }
    Ok(text)
}

async fn run(cli: Cli) -> Result<(), Failure> {
    let config = load_config()?;
    let venue = resolve_venue(&config, cli.venue.as_deref())?;
    let password = std::env::var(&venue.password_env).map_err(|_| {
        Failure::Auth(format!(
            "env var {} is not set (the venue password)",
            venue.password_env
        ))
    })?;

    let url = format!("{}/ws?protocol=json", venue.url.trim_end_matches('/'));
    let (mut ws, _) = connect_async(&url)
        .await
        .map_err(|e| Failure::Link(format!("connecting to {url}: {e}")))?;
    expect(&mut ws, "connected").await?;
    authenticate(&mut ws, &venue.username, &password).await?;

    for text in &cli.turns {
        let reply = submit(&mut ws, &cli, &venue, text).await?;
        if cli.json {
            println!("{}", json!({ "session": cli.session, "reply": reply }));
        }
    }
    let _ = ws.close(None).await;
    Ok(())
}

fn load_config() -> Result<GravityConfig, Failure> {
    let path = std::env::var("GRAVITY_CONFIG").unwrap_or_else(|_| "gravity.json".into());
    let raw = std::fs::read_to_string(&path)
        .map_err(|e| Failure::Usage(format!("reading config {path}: {e}")))?;
    serde_json::from_str(&raw).map_err(|e| Failure::Usage(format!("parsing config {path}: {e}")))
}

/// The venue is the CLI's whole reason to exist (ADR-0002), so a config
/// without one is a config for something else — saying that beats guessing a
/// localhost default.
fn resolve_venue(
    config: &GravityConfig,
    override_url: Option<&str>,
) -> Result<VenueConfig, Failure> {
    let mut venue = config.venue.clone().ok_or_else(|| {
        Failure::Usage(
            "config has no `venue` node — the CLI submits turns to a venue (ADR-0002); \
             add { \"url\", \"username\", \"password_env\" }"
                .into(),
        )
    })?;
    if let Some(url) = override_url {
        venue.url = url.to_string();
    }
    Ok(venue)
}

type Ws = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// The venue's mount table as a turn carries it (ADR-0001 §7): operation language → the mount
/// type that serves it. It rides the CALL because a client cannot enumerate a venue's mounts and
/// gravity refuses to guess the match (a language no mount serves is refused by name).
fn mounts_of(venue: &VenueConfig) -> Value {
    let entries: Vec<Value> = venue
        .mounts
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|entry| json!({ "language": entry.language, "type": entry.type_name }))
        .collect();
    Value::Array(entries)
}

/// Read one frame and require its `ev`: the handshake and the auth answer are
/// strictly ordered, so a mismatched frame is a protocol break, not a race.
async fn expect(ws: &mut Ws, ev: &str) -> Result<Value, Failure> {
    let frame = next_frame(ws).await?;
    if frame.get("ev").and_then(Value::as_str) != Some(ev) {
        return Err(Failure::Link(format!(
            "expected `{ev}`, the venue answered {frame}"
        )));
    }
    Ok(frame.get("args").cloned().unwrap_or(Value::Null))
}

/// Login, falling back to signup once: a fresh venue has no account, and
/// asking the operator to run a second command for that buys nothing (the
/// password is the same secret either way). A signup that also fails is the
/// auth failure.
async fn authenticate(ws: &mut Ws, username: &str, password: &str) -> Result<(), Failure> {
    send(
        ws,
        &json!({ "ev": "login", "args": { "username": username, "password": password } }),
    )
    .await?;
    let first = next_frame(ws).await?;
    match first.get("ev").and_then(Value::as_str) {
        Some("login.result") => Ok(()),
        Some("error") => {
            send(
                ws,
                &json!({ "ev": "signup", "args": { "username": username, "password": password } }),
            )
            .await?;
            let second = next_frame(ws).await?;
            if second.get("ev").and_then(Value::as_str) == Some("signup.result") {
                return Ok(());
            }
            Err(Failure::Auth(format!(
                "neither login nor signup was accepted: {} ({first})",
                second
                    .get("args")
                    .and_then(|args| args.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("no message")
            )))
        }
        _ => Err(Failure::Auth(format!("unexpected auth answer: {first}"))),
    }
}

/// One turn: submit as an EVENT, stream, read the end. The venue's word for a turn is an emit —
/// `ui:turn.submit` — so there is no answer to read; the end arrives as `turn.done` (a booth that
/// is reached by an event says its end, gravity ADR-0009's Update), and the reply is the deltas
/// this client already assembled.
async fn submit(
    ws: &mut Ws,
    cli: &Cli,
    venue: &VenueConfig,
    content: &str,
) -> Result<String, Failure> {
    // `channel_id` is the conversation the application keeps its memory in, `member_id` who is
    // speaking, and `mounts` is the venue's mount table as its manifest declares it (ADR-0001 §7) —
    // gravity never guesses a language→type match. The `event` is the PLANE name for the chunks;
    // the venue hands it back with the marker stripped, which is the `turn.delta` read below.
    send(
        ws,
        &json!({
            "ev": "ui:turn.submit",
            "args": {
                "channel_id": cli.channel,
                "member_id": cli.member,
                "user_message": content,
                "event": "ui:turn.delta",
                "mounts": mounts_of(venue),
            }
        }),
    )
    .await?;
    let mut seen = String::new();
    let mut cancelled = false;
    let mut deadline =
        cli.cancel_after_ms
            .map(|ms| Box::pin(tokio::time::sleep(Duration::from_millis(ms))));
    loop {
        let frame = tokio::select! {
            message = ws.next() => {
                let message = message
                    .ok_or_else(|| Failure::Link("the venue closed the connection".into()))?
                    .map_err(|e| Failure::Link(format!("websocket: {e}")))?;
                match message {
                    Message::Text(text) => parse_frame(text.as_str())?,
                    Message::Binary(bytes) => {
                        let text = std::str::from_utf8(&bytes)
                            .map_err(|e| Failure::Link(format!("non-utf8 frame: {e}")))?;
                        parse_frame(text)?
                    }
                    Message::Close(_) => {
                        return Err(Failure::Link("the venue closed the connection".into()))
                    }
                    _ => continue,
                }
            }
            // Ctrl-C cancels the LIVE turn through the venue's own verb
            // instead of leaving it running behind a dead client.
            _ = tokio::signal::ctrl_c() => {
                cancel(ws).await?;
                cancelled = true;
                deadline = None;
                continue;
            }
            _ = async {
                match deadline.as_mut() {
                    Some(sleep) => sleep.as_mut().await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                cancel(ws).await?;
                cancelled = true;
                deadline = None;
                continue;
            }
        };
        match frame.get("ev").and_then(Value::as_str) {
            Some("turn.delta") => {
                let delta = frame
                    .get("args")
                    .and_then(|args| args.get("delta"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                seen.push_str(delta);
                if !cli.json {
                    let mut out = std::io::stdout();
                    out.write_all(delta.as_bytes()).ok();
                    out.flush().ok();
                }
            }
            Some("turn.done") => {
                // A turn reached by an EVENT has no return value to read (gravity ADR-0009's
                // Update): the end is said, and the reply is the deltas already assembled here.
                // `value.reply` is kept for the direct-caller shape, which does answer.
                let reply = frame
                    .get("args")
                    .and_then(|args| args.get("value"))
                    .and_then(|value| value.get("reply"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or(seen);
                if !cli.json {
                    let mut out = std::io::stdout();
                    out.write_all(b"\n").ok();
                    out.flush().ok();
                    if cancelled {
                        eprintln!("gravity> turn cancelled");
                    }
                }
                return Ok(reply);
            }
            Some("turn.error") => {
                // gravity's own end says `detail`; the venue's refusal value says `message`.
                let message = frame
                    .get("args")
                    .and_then(|args| args.get("detail").or_else(|| args.get("message")))
                    .and_then(Value::as_str)
                    .unwrap_or("the turn failed");
                return Err(Failure::Turn(message.to_string()));
            }
            Some("error") => {
                let message = frame
                    .get("args")
                    .and_then(|args| args.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("the venue refused the turn");
                return Err(if message.contains("authentication required") {
                    Failure::Auth(message.to_string())
                } else {
                    Failure::Turn(message.to_string())
                });
            }
            // Anything else on this connection is not ours to interpret.
            _ => continue,
        }
    }
}

async fn cancel(ws: &mut Ws) -> Result<(), Failure> {
    // The marker on cancel is gravity ADR-0008's OWED PART: the event travels like any other,
    // and nothing reads it yet, so an operator sees the client stop rather than a cancelled turn.
    send(ws, &json!({ "ev": "ui:turn.cancel", "args": {} })).await
}

async fn send(ws: &mut Ws, frame: &Value) -> Result<(), Failure> {
    ws.send(Message::Text(frame.to_string()))
        .await
        .map_err(|e| Failure::Link(format!("sending to the venue: {e}")))
}

async fn next_frame(ws: &mut Ws) -> Result<Value, Failure> {
    loop {
        let message = ws
            .next()
            .await
            .ok_or_else(|| Failure::Link("the venue closed the connection".into()))?
            .map_err(|e| Failure::Link(format!("websocket: {e}")))?;
        match message {
            Message::Text(text) => return parse_frame(text.as_str()),
            Message::Binary(bytes) => {
                let text = std::str::from_utf8(&bytes)
                    .map_err(|e| Failure::Link(format!("non-utf8 frame: {e}")))?;
                return parse_frame(text);
            }
            Message::Close(_) => {
                return Err(Failure::Link("the venue closed the connection".into()))
            }
            _ => continue,
        }
    }
}

fn parse_frame(text: &str) -> Result<Value, Failure> {
    serde_json::from_str(text).map_err(|e| Failure::Link(format!("unparsable frame: {e}")))
}