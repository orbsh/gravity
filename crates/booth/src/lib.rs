//! gravity-booth — the aura booth binding (PLAN Phase 4, re-landed on
//! induction): the turn execution as Rust guest processes speaking the
//! induction pipe protocol (`~/world/induction`).
//!
//! Two guests:
//!
//! - `gravity-provider` — the LLM booth: SSE from an OpenAI-compatible
//!   endpoint parsed into an iterate producer; one envelope round per
//!   pull, `done: true` closes. Key custody: the key is read from the
//!   env var NAMED in the args (the value never in the module or args).
//! - `gravity-turn` — the turn booth: a `turn` PRODUCER that fetches the
//!   session projection from mem-server (HTTP — the booth's own code
//!   decides its access method, ADR-0031), opens the provider stream
//!   through the ctx iterate arms (ADR-0034's pull loop — the provider
//!   is a sibling citizen, not gravity-internal plumbing), yields
//!   `{"delta": ...}` per envelope item while the stream is live, then
//!   appends the one-ts-two-subs exchange and returns. The instance key
//!   IS the session (partition = session_id per aura PLAN Phase 6).
//!
//! The guest process IS the stream state — the process holds the SSE
//! connection and the accumulated reply across pull rounds, which is
//! exactly the resident state the old in-process `Body::Rust` closure
//! arm lacked. prism's `turn.delta` contract rides the envelopes
//! unchanged.
//!
//! Custody: the LLM key is read from the env var named in the args;
//! the store URL rides the args the same way (a deployment fact the
//! caller supplies per call — the booth holds no connection state).

/// The turn booth's type name.
pub const TURN_TYPE: &str = "gravity_turn";

/// The provider booth's type name (the ctx.iterate target).
pub const PROVIDER_TYPE: &str = "llm_provider";

/// The provider handler the turn stream opens.
pub const PROVIDER_HANDLER: &str = "stream";

/// The provider booth spawn spec (the source IS the argv for the
/// `induction` language — ADR-0035 §2, the shellish-spawn contract).
pub const PROVIDER_SPEC: &str = "gravity-provider";

/// The turn booth spawn spec.
pub const TURN_SPEC: &str = "gravity-turn";

/// Register both booth types on an engine. The spawn specs name the two
/// guest binaries (deployment installs them on PATH or beside the
/// engine's cwd — the spec is argv, resolved by the carrier's spawn);
/// the codec is the session's declared one, CBOR here (high-frequency
/// small token frames — the self-delimited document form skips the
/// per-frame JSON quoting overhead; both guests read the same env).
pub async fn register(engine: &aura_engine::Engine) -> anyhow::Result<()> {
    engine
        .register(
            aura_booth::BoothType::script(PROVIDER_TYPE, "induction", PROVIDER_SPEC)
                .encoded(aura_booth::Encoding::Cbor),
        )
        .await?;
    engine
        .register(
            aura_booth::BoothType::script(TURN_TYPE, "induction", TURN_SPEC)
                .encoded(aura_booth::Encoding::Cbor),
        )
        .await?;
    Ok(())
}
