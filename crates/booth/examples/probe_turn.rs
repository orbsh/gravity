//! Probe: a minimal induction host that spawns gravity-turn, answers its
//! ctx iterate arms against the provider guest (also spawned here), and
//! drives the turn stream to done — reproducing the realm's wiring
//! without the engine.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unwrap_in_result, clippy::panic_in_result_fn)]

use induction_host::{spawn, Budgets, HostOps, InvokeCall, OpsFuture, Spec};
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

struct Ops {
    provider: Mutex<Option<induction_host::Process>>,
}

impl HostOps for Ops {
    fn invoke<'a>(&'a self, _call: &'a InvokeCall) -> OpsFuture<'a> {
        Box::pin(async { Err("no invoke".into()) })
    }
    fn iterate<'a>(&'a self, arm: induction_protocol::Arm, args: &'a Value) -> OpsFuture<'a> {
        use induction_protocol::{Arm, CallFrame, IterateKind};
        let args = args.clone();
        Box::pin(async move {
            let mut guard = self.provider.lock().await;
            let proc = guard.as_mut().unwrap();
            let kind = match arm {
                Arm::IterateStart => IterateKind::Start,
                Arm::IterateNext => IterateKind::Next,
                _ => IterateKind::Dispose,
            };
            let sid = args
                .get("stream_id")
                .and_then(Value::as_str)
                .unwrap_or("p1")
                .to_string();
            // The canonical booth-call shape: forward the INNER args to
            // the provider (the realm does the same unwrapping).
            let inner = args.get("args").cloned().unwrap_or_else(|| args.clone());
            let frame = CallFrame::iterate(1, kind, "stream", inner, sid);
            proc.session
                .writer
                .write_line(&frame.to_line(), std::time::Duration::from_secs(5))
                .await
                .then_some(())
                .ok_or("write stalled".to_string())?;
            let line = proc
                .session
                .reader
                .next_line(std::time::Duration::from_secs(5))
                .await
                .ok_or("no reply".to_string())?;
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            let result = v
                .get("result")
                .cloned()
                .ok_or_else(|| format!("no result: {v}"))?;
            // The host merges its own stream_id into a non-terminal first
            // reply (the realm's ruling).
            if arm == Arm::IterateStart && result.get("done").and_then(Value::as_bool) != Some(true)
            {
                let mut r = result.clone();
                if let Value::Object(map) = &mut r {
                    map.insert("stream_id".into(), json!("p1"));
                }
                return Ok(r);
            }
            Ok(result)
        })
    }
}

/// Read frames from the turn guest until a Result for `id` arrives,
/// answering any host arms along the way (the ctx iterate arms route
/// to the provider through `ops`).
async fn wait_result(
    turn: &mut induction_host::Process,
    ops: &Ops,
    id: u64,
) -> anyhow::Result<Value> {
    use induction_host::route_line;
    use induction_protocol::parse_guest_line;
    loop {
        let line = turn
            .session
            .reader
            .next_line(std::time::Duration::from_secs(5))
            .await
            .ok_or_else(|| anyhow::anyhow!("guest frame timeout"))?;
        match parse_guest_line(&line) {
            Some(induction_protocol::GuestFrame::Result(r)) => {
                if r.id == serde_json::json!(id) {
                    return Ok(r.result);
                }
                // A reply to an earlier frame — log and continue.
                eprintln!("probe: stray result: {}", r.result);
            }
            Some(_) => {
                // A host arm — serve it through the shared dispatcher.
                let routed = route_line(ops, &line).await;
                if let induction_host::Routed::Reply(reply) = routed {
                    turn.session
                        .writer
                        .write_line(&reply.to_line(), std::time::Duration::from_secs(5))
                        .await;
                }
            }
            None => anyhow::bail!("undecodable guest frame: {line}"),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let provider = spawn(&Spec::new(
        "p",
        ["target/debug/gravity-provider".to_string()],
    ))?;
    let ops = Arc::new(Ops {
        provider: Mutex::new(Some(provider)),
    });
    let ops_ref = ops.as_ref();

    let mut turn = spawn(&Spec::new("t", ["target/debug/gravity-turn".to_string()]))?;
    turn.session.handshake(Budgets::default().handshake).await;
    // Start the turn stream with real args.
    let args = json!({
        "round": 0,
        "store_url": "http://127.0.0.1:19993",
        "channel_id": 7, "member_id": 1, "user_message": "hi",
        "endpoint": "http://127.0.0.1:19994",
        "model": "m", "key_env": "GRAVITY_TEST_KEY",
    });
    let frame = induction_protocol::CallFrame::iterate(
        1,
        induction_protocol::IterateKind::Start,
        "turn",
        args,
        "t1".to_string(),
    );
    turn.session
        .writer
        .write_line(&frame.to_line(), Budgets::default().write)
        .await;
    let result = wait_result(&mut turn, ops_ref, 1).await?;
    println!("START: {result}");
    if result.get("done").and_then(Value::as_bool) == Some(true) {
        anyhow::bail!("turn failed to open its stream: {result}");
    }
    for round in 1..8 {
        let frame = induction_protocol::CallFrame::iterate(
            round + 1,
            induction_protocol::IterateKind::Next,
            "turn",
            json!({}),
            "t1".to_string(),
        );
        turn.session
            .writer
            .write_line(&frame.to_line(), Budgets::default().write)
            .await;
        let result = wait_result(&mut turn, ops_ref, round + 1).await?;
        println!("PULL {round}: {result}");
        if result.get("done").and_then(Value::as_bool) == Some(true) {
            break;
        }
    }
    turn.kill();
    Ok(())
}
