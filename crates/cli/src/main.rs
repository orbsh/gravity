//! gravity — the CLI driver (Milestone A's local form): a for-loop over
//! the turn function. Local mode = the loop itself (the stateless-agent
//! architecture's founding observation); no server, no aura — the same
//! `turn()` function the booth binding wraps later.
//!
//! Config comes from a JSON file (Phase 0's minimal surface; the KDL
//! discipline arrives when a second subsystem needs a node) — path from
//! `GRAVITY_CONFIG` or the default `./gravity.json`.

use anyhow::Context;
use gravity_config::GravityConfig;
use std::io::{BufRead, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let path = std::env::var("GRAVITY_CONFIG").unwrap_or_else(|_| "gravity.json".into());
    let raw = std::fs::read_to_string(&path).with_context(|| format!("reading config {path}"))?;
    let config: GravityConfig =
        serde_json::from_str(&raw).with_context(|| format!("parsing config {path}"))?;

    // Memory surface: store_url (HTTP, the cross-process form) wins
    // over store_path (in-process) when both are present — explicit
    // precedence, not an error, so one config file serves both bring-up
    // and the deployed shape.
    //
    // The memory representation is validated HERE, before the first turn:
    // `struct` returns the fact data for a caller that assembles the graph
    // itself, which this executor never does (ADR-0010 §5).
    let format = gravity_core::surface::MemoryFormat::from_config(config.memory_format.as_deref())
        .context("memory_format")?;
    let surface: std::sync::Arc<dyn gravity_core::surface::MemorySurface> = if let Some(url) =
        &config.store_url
    {
        std::sync::Arc::new(gravity_core::surface::HttpSurface::new(url).with_format(format))
    } else if let Some(path) = &config.store_path {
        std::sync::Arc::new(
            gravity_core::surface::InProcessSurface::open(std::path::Path::new(path))
                .map_err(|e| anyhow::anyhow!("opening store at {path}: {e}"))?
                .with_format(format),
        )
    } else {
        anyhow::bail!("config needs either store_path (in-process) or store_url (mem-server HTTP)");
    };

    // One store for the process lifetime (the CLI is the loop); the
    // channel is this session's conversation log.
    let client = gravity_core::llm::LlmClient::from_config(&config)?;

    let channel_id = 1; // Phase 0: one local conversation
                        // mem-core's single-member default, re-exported at the surface seam
                        // (the CLI needs no mem-core dep for one constant).
    let member_id = gravity_core::surface::DEFAULT_MEMBER;
    println!("gravity ready — channel {channel_id}, empty line quits");

    let stdin = std::io::stdin();
    loop {
        print!("you> ");
        std::io::stdout().flush()?;
        let mut line = String::new();
        stdin.lock().read_line(&mut line)?;
        let user_message = line.trim().to_string();
        if user_message.is_empty() {
            break;
        }

        let input = gravity_core::TurnInput {
            channel_id,
            member_id,
            user_message,
        };
        match gravity_core::turn::turn(surface.as_ref(), &client, input).await {
            Ok(out) => println!("assistant> {}\n", out.reply),
            Err(e) => {
                // Error values, never a crash: the loop continues, the
                // failed turn left no trace in the log.
                eprintln!("error> {e:#}\n");
            }
        }
    }
    Ok(())
}
