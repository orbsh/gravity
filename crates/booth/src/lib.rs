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
//! - `gravity-turn` — the turn booth: ONE handler (`turn`) the venue reaches by
//!   subscription (`ui:turn.submit`). It fetches the session projection from
//!   mem-server (HTTP — the booth's own code decides its access method, ADR-0031),
//!   opens the provider stream through the ctx iterate arms (ADR-0034 — the
//!   provider is a sibling citizen, not gravity-internal plumbing), emits every
//!   chunk on the venue wire as it arrives, then appends the one-ts-two-subs
//!   exchange and returns. Nothing pulls it: a turn is one handler call, so its
//!   state lives on one call stack. The instance key is the envelope's own
//!   address, so one device runs one turn at a time.
//!
//! The guest process IS the turn's state — the SSE connection, the accumulated
//! reply and the live tail branches all sit on the handler's stack, which is
//! exactly the resident form the old in-process `Body::Rust` closure arm lacked,
//! with no pull face that has to stitch the rounds back together.
//!
//! Custody: the LLM key is read from the env var named in the args;
//! the store URL rides the args the same way (a deployment fact the
//! caller supplies per call — the booth holds no connection state).

/// The fixed tool list (ADR-0004) — the affordances the turn offers, their
/// schemas, and the door each one reaches.
pub mod tasks;
pub mod tools;

/// The turn booth's type name.
pub const TURN_TYPE: &str = "gravity_turn";

/// The provider booth's type name (the ctx.iterate target).
pub const PROVIDER_TYPE: &str = "llm_provider";

/// The provider handler the turn stream opens.
pub const PROVIDER_HANDLER: &str = "stream";

/// The provider booth's default spawn spec (the source IS the argv for
/// the `induction` language — ADR-0035 §2, the shellish-spawn contract).
const PROVIDER_SPEC: &str = "gravity-provider";

/// The turn booth's default spawn spec.
const TURN_SPEC: &str = "gravity-turn";

/// The provider booth spawn spec (the source IS the argv for the
/// `induction` language — ADR-0035 §2, the shellish-spawn contract).
/// Overridable per deployment via `GRAVITY_PROVIDER_BIN` (a test or a
/// non-PATH install points at the absolute binary).
pub fn provider_spec() -> String {
    std::env::var("GRAVITY_PROVIDER_BIN").unwrap_or_else(|_| PROVIDER_SPEC.into())
}

/// The turn booth spawn spec (`GRAVITY_TURN_BIN` override, same rule).
pub fn turn_spec() -> String {
    std::env::var("GRAVITY_TURN_BIN").unwrap_or_else(|_| TURN_SPEC.into())
}

/// Register both booth types on an engine. The spawn specs name the two
/// guest binaries (deployment installs them on PATH or beside the
/// engine's cwd — the spec is argv, resolved by the carrier's spawn;
/// env overrides for non-PATH installs and tests); the codec is the
/// session's declared one, CBOR here (high-frequency small token
/// frames — the self-delimited document form skips the per-frame JSON
/// quoting overhead; both guests read the same env).
pub async fn register(engine: &aura_engine::Engine) -> anyhow::Result<()> {
    engine
        .register(
            aura_booth::BoothType::script(PROVIDER_TYPE, "induction", provider_spec())
                .encoded(aura_booth::Encoding::Cbor),
        )
        .await?;
    engine
        .register(
            aura_booth::BoothType::script(TURN_TYPE, "induction", turn_spec())
                .encoded(aura_booth::Encoding::Cbor)
                // The venue's own word for "run a turn", declared here rather than special-cased
                // in the host: the entry is `turn` (the guest's handler), the key is the envelope's
                // address, so one device has one turn instance.
                // The instance key is the ASKING DEVICE (prism ADR-0018: the transport
                // knows devices and users; a channel is an application's business). One
                // device, one turn at a time.
                .on_as("ui:turn.submit", "device", "turn")
                // WHERE A DEPLOYMENT IS CONFIGURED: the facts are rows in this type's ns, so the
                // door that writes them is an event too (auth: `ui:gravity-model.save`). A venue
                // never learns the word — this is the application's own vocabulary, as much as
                // `ui:turn.submit` is.
                .on_as("ui:gravity-model.save", "device", "save"),
        )
        .await?;
    Ok(())
}
