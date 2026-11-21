//! gravity — GRAVITY'S OWN CLI, wearing a client's clothes. It is not prism's CLI: the venue is
//! the wire it has to pass through (ADR-0002 — there is no other door), not the thing it drives.
//! What it drives is the agent, and the agent is the one that decides what a task needs.
//!
//! Shape: kubectl-like, one command = one call, stdin/stdout, meaningful exit codes, no session
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
usage: gravity --tag <name> [--json] [--session <id>] [--venue <ws-url>] \
                [--cancel-after-ms <n>] <text>...

  <text>...           one turn per argument; `-` reads stdin as ONE turn
  --json              one JSON object per turn on stdout (scripts); the
                      readable form streams the deltas as they arrive
  --session <id>      PIN the conversation (k10r's channel id, a number). Left out, the
                      channel follows the SPEAKER: the line k10r records as theirs, opened
                      on first use — so a person's first message needs no id at all, and
                      two contexts never share a transcript. The client keeps no state
                      (ADR-0002 §3)
  --tag <name>        WHO CALLS, and the only identity there is: this invocation speaks as
                      an agent named <name>, and its member id is DERIVED from that name. The
                      same tag is the same speaker — and the same line — on every run, which is
                      what makes a cron job or a CI step accumulate one history instead of a
                      new one each time. Nothing on disk: the identity is a function of the name.
                      The tag is what the transcript shows. There is no speak-as-a-person
                      form: this client is not the user, and a person's voice comes from the
                      page they are looking at.
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
    /// The conversation, as the operator typed it (echoed in `--json` output). `None` = the
    /// channel follows the speaker, which is the shape that needs no ids: k10r knows which channel
    /// is somebody's own (`solo`), and this client never learns an id it did not need.
    session: Option<String>,
    /// The same value as a channel id: k10r's channel is a NUMBER, so a session that is not one is
    /// a usage failure, refused before a socket is opened.
    channel: Option<u64>,
    /// WHO CALLS: the name of the automation this invocation IS. Required, because a caller that
    /// does not say what it is has no line to continue — and the store, not the client, decides
    /// what a speaker reads.
    tag: String,
    venue: Option<String>,
    cancel_after_ms: Option<u64>,
    turns: Vec<String>,
}

fn parse_args(args: impl Iterator<Item = String>) -> Result<Cli, Failure> {
    let mut json = false;
    let mut session: Option<String> = None;
    let mut tag: Option<String> = None;
    let mut venue = None;
    let mut cancel_after_ms = None;
    let mut turns = Vec::new();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--session" => {
                session = Some(
                    args.next()
                        .ok_or_else(|| Failure::Usage("--session needs an id".into()))?,
                );
            }
            "--tag" => {
                tag = Some(
                    args.next()
                        .ok_or_else(|| Failure::Usage("--tag needs a name".into()))?,
                );
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
    let channel = match session.as_deref() {
        Some(raw) => Some(raw.parse::<u64>().map_err(|_| {
            Failure::Usage(format!(
                "--session: `{raw}` is not a channel id (k10r's channel is a number)"
            ))
        })?),
        None => None,
    };
    // A tag is not optional: this binary's caller is a script, a cron job or a CI step, and a
    // caller that will not say what it is has no line to continue — its turns would land in a new
    // one every run. The refusal is the feature.
    let tag = match tag {
        Some(tag) if tag.trim().is_empty() => {
            return Err(Failure::Usage("--tag: empty name".into()))
        }
        Some(tag) => tag,
        None => {
            return Err(Failure::Usage(
                "a caller names itself: --tag <name>. This client speaks as the agent that name \
                 describes — there is no speaking as a person (a person's voice comes from the \
                 page they are looking at)"
                    .into(),
            ))
        }
    };
    Ok(Cli {
        json,
        session,
        channel,
        tag,
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
    // THIS CLIENT IS AN AGENT NAMED BY ITS TAG, derived here and never stored: nothing on disk can
    // be lost, forgotten or checked out fresh into a different identity. The same tag is the same
    // speaker on every run, and the same speaker in the same channel IS the same conversation —
    // which is how one-turn invocations accumulate into a history.
    let member = tag_member(&cli.tag);
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
        let reply = submit(&mut ws, &cli, &member, text).await?;
        if cli.json {
            println!(
                "{}",
                json!({ "tag": cli.tag, "session": cli.session, "reply": reply })
            );
        }
    }
    let _ = ws.close(None).await;
    Ok(())
}

fn config_path() -> String {
    std::env::var("GRAVITY_CONFIG").unwrap_or_else(|_| "gravity.json".into())
}

/// A TAG's member id: derived from the tag's NAME.
///
/// Two properties, both required: the same tag is the same agent on every run — so a script's turns
/// accumulate into ONE conversation — and two tags are two agents without a file to keep them in.
/// It is not a security boundary: the venue's stamp is what protects a person's identity, and a
/// declared identity that k10r says is a person is not honoured.
///
/// FNV-1a twice with different seeds: 128 bits of stable text, shaped like the uuids around it.
fn tag_member(name: &str) -> String {
    fn fnv(bytes: &[u8], seed: u64) -> u64 {
        let mut hash = seed;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }
    let mut raw = [0u8; 16];
    raw[..8].copy_from_slice(&fnv(name.as_bytes(), 0xcbf2_9ce4_8422_2325).to_be_bytes());
    raw[8..].copy_from_slice(&fnv(name.as_bytes(), 0x9e37_79b9_7f4a_7c15).to_be_bytes());
    raw[6] = (raw[6] & 0x0f) | 0x70; // reads as a v7, like every member id around it
    raw[8] = (raw[8] & 0x3f) | 0x80;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

fn load_config() -> Result<GravityConfig, Failure> {
    let path = config_path();
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
///
/// The credential's field name is the VENUE's, not this config's: prism's auth
/// surface asks for `login_name`, and a client that sends its own word for it
/// (`username`) is refused with "auth `register` needs `login_name`" — which is
/// what the smoke script found, because the stub venue in this crate's tests
/// accepted anything. The config keeps `username` (it is a person, not a field);
/// the wire says what the venue reads.
async fn authenticate(ws: &mut Ws, username: &str, password: &str) -> Result<(), Failure> {
    send(
        ws,
        &json!({ "ev": "login", "args": { "login_name": username, "password": password } }),
    )
    .await?;
    let first = next_frame(ws).await?;
    match first.get("ev").and_then(Value::as_str) {
        Some("login.result") => Ok(()),
        Some("error") => {
            send(
                ws,
                &json!({ "ev": "signup", "args": { "login_name": username, "password": password } }),
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
async fn submit(ws: &mut Ws, cli: &Cli, member: &str, content: &str) -> Result<String, Failure> {
    // `channel_id` is the conversation the application keeps its memory in and `member_id` who is
    // speaking — nothing else. The mount table is NOT here any more: the booth reads it from the
    // venue it runs inside (ADR-0001 §7, amended 2026-11-20), so a page-driven turn and this one
    // see the same table instead of one client carrying a copy. The `event` is the PLANE name for
    // the chunks; the venue hands it back with the marker stripped, which is the `turn.delta`
    // read below.
    let mut args = json!({
        "member_id": member,
        // Only when the caller is a context: the name registers the identity in the store the
        // first time this context speaks, so the transcript can say "drift check".
        "member_name": cli.tag,
        "user_message": content,
        "event": "ui:turn.delta",
    });
    // ABSENT unless the operator pinned one, never `null`: the booth then resolves the speaker's
    // OWN line from k10r (`solo`), and that is the whole addressing a caller needs — a person's
    // first message carries no channel id, and the store opens their line for them.
    if let Some(channel) = cli.channel {
        args["channel_id"] = json!(channel);
    }
    send(ws, &json!({ "ev": "ui:turn.submit", "args": args })).await?;
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