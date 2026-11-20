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

/// The venue's admin base: the same listener that serves the gateway's WS, read as HTTP
/// (prism serves both on one port, and `/admin` is the operator's face — prism ADR-0017 §7).
fn admin_base(url: &str) -> Result<String, Failure> {
    if let Some(rest) = url.strip_prefix("ws://") {
        return Ok(format!("http://{rest}"));
    }
    Err(Failure::Usage(format!(
        "venue url `{url}` is not plain `ws://`: this client reads the mount table over HTTP and \
         has no TLS client, so a `wss://` venue must be reached through a local `ws://` one"
    )))
}

/// The venue's mount table, READ from the venue (prism ADR-0021) instead of carried in the config.
/// gravity keeps no copy: two copies of one venue's mounts drift, and the copy in a client's file
/// is the one nobody updates. It still rides the CALL (ADR-0001 §7) — the turn booth runs inside
/// the venue and cannot enumerate what the venue's own `/admin` face knows.
///
/// Ordering puts a CONNECTED mount first for each language: when two nodes serve `python`, the one
/// that is here is the one to address, and a job that picks the absent one spends a dispatch
/// learning what this page already said.
async fn venue_mounts(venue: &VenueConfig) -> Result<Value, Failure> {
    let base = admin_base(&venue.url)?;
    let page = http_get(&format!("{base}/admin/mounts")).await?;
    let parsed: Value = serde_json::from_str(&page)
        .map_err(|e| Failure::Link(format!("the venue's /admin/mounts is not JSON: {e}")))?;
    let mut entries: Vec<gravity_config::MountEntry> = parsed
        .get("mounts")
        .cloned()
        .map(|rows| serde_json::from_value(rows).unwrap_or_default())
        .unwrap_or_default();
    for line in parsed
        .get("refused")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
    {
        if let Some(text) = line.as_str() {
            eprintln!("venue: {text}");
        }
    }
    entries.retain(|m| m.registered);
    entries.sort_by(|a, b| {
        (&a.language, !a.node_connected, &a.type_name)
            .cmp(&(&b.language, !b.node_connected, &b.type_name))
    });
    Ok(Value::Array(
        entries
            .iter()
            .map(|m| json!({ "language": m.language, "type": m.type_name }))
            .collect(),
    ))
}

/// One `GET` of a small JSON page: `/admin` answers a fixed body and closes, so this needs no
/// client, no pool and no redirect handling — a request line, the headers, read to the end, split
/// the body off.
async fn http_get(url: &str) -> Result<String, Failure> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| Failure::Usage(format!("`{url}` is not an http:// URL")))?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let mut stream = tokio::net::TcpStream::connect(authority)
        .await
        .map_err(|e| Failure::Link(format!("connecting to {authority}: {e}")))?;
    let request =
        format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| Failure::Link(format!("sending to {authority}: {e}")))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|e| Failure::Link(format!("reading from {authority}: {e}")))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text
        .split_once("\r\n\r\n")
        .ok_or_else(|| Failure::Link(format!("{authority} answered without a body: {text}")))?;
    if !head.starts_with("HTTP/1.1 2") {
        return Err(Failure::Link(format!(
            "{authority} refused {}: {head}",
            url.trim_start_matches("http://")
        )));
    }
    Ok(body.to_string())
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
    // speaking, and `mounts` is the venue's own answer (read from `/admin/mounts`, not from this
    // client's config — ADR-0001 §7 as amended) — gravity never guesses a language→type match.
    // The `event` is the PLANE name for the chunks; the venue hands it back with the marker
    // stripped, which is the `turn.delta` read below.
    let mounts = venue_mounts(venue).await?;
    send(
        ws,
        &json!({
            "ev": "ui:turn.submit",
            "args": {
                "channel_id": cli.channel,
                "member_id": cli.member,
                "user_message": content,
                "event": "ui:turn.delta",
                "mounts": mounts,
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
                let reason = frame
                    .get("args")
                    .and_then(|args| args.get("reason"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if !cli.json {
                    let mut out = std::io::stdout();
                    out.write_all(b"\n").ok();
                    out.flush().ok();
                }
                // A CANCELLED TURN IS NOT A COMPLETED ONE (ADR-0008 point 4): the reply
                // that streamed never landed anywhere, so the exit status must not say it
                // did. The turn says why on the frame it ends on — the client's own
                // request is not the source of truth here, the venue's answer is.
                if cancelled || reason == "cancelled" {
                    return Err(Failure::Turn("turn cancelled".into()));
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
    // The venue's uplink shape, because that is what the venue reads: `dest` names the verb
    // and `data` is the payload. A cancel is NOT an event and gets NO ANSWER — the venue
    // holds authority over this connection, so it sets the turn's stop bit and interrupts
    // the call; the only thing this client can observe is the turn ending (`turn.done` with
    // `reason: "cancelled"`). Asking for a receipt is what made the old shape look like it
    // worked while the venue never saw it.
    send(ws, &json!({ "dest": "turn.cancel", "data": {} })).await
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