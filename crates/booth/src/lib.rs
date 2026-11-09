//! gravity-booth — the aura booth binding (PLAN Phase 4, re-landed on
//! induction): the turn execution as a Rust guest process speaking the induction
//! pipe protocol (`~/world/induction`).
//!
//! ONE guest: `gravity-turn`, the turn booth whose handler (`turn`) the venue
//! reaches by subscription (`ui:turn.submit`). It fetches the session projection
//! from mem-server (HTTP — the booth's own code decides its access method,
//! ADR-0031), opens the LLM stream ITSELF (`gravity_core::llm::live`), emits every
//! chunk on the venue wire as it arrives, writes its rows to the store as it runs,
//! and closes the turn. There is no provider booth: a stream is a value on a
//! thread, so two streams — the answer and a compression branch, or two devices'
//! turns — are two threads, not two processes to key and evict.
//!
//! Nothing pulls the turn: a turn is one handler call, and its state lives on one
//! call stack. Rounds OVERLAP IN THIS ONE PROCESS — the type declares `unbounded`
//! (aura ADR-0046) and the guest runs each request on its own thread, so a
//! device's next message is served while the previous turn is still streaming —
//! and several turns in one process is induction's business (request/reply ids).
//!
//! Custody: the LLM key is read from the env var named in the args; the store URL
//! rides the args the same way (a deployment fact the caller supplies per call —
//! the booth holds no connection state).

/// The fixed tool list (ADR-0004) — the affordances the turn offers, their
/// schemas, and the door each one reaches.
pub mod tasks;
pub mod tools;

/// The turn booth's type name.
pub const TURN_TYPE: &str = "gravity_turn";

/// The turn booth's default spawn spec.
const TURN_SPEC: &str = "gravity-turn";

/// The turn booth spawn spec (`GRAVITY_TURN_BIN` override, same rule).
pub fn turn_spec() -> String {
    std::env::var("GRAVITY_TURN_BIN").unwrap_or_else(|_| TURN_SPEC.into())
}

/// Register the turn booth on an engine. The spawn spec names the guest binary
/// (a deployment installs it on PATH or beside the engine's cwd — the spec is
/// argv, resolved by the carrier's spawn; `GRAVITY_TURN_BIN` overrides for
/// non-PATH installs and tests); the codec is the executor's declared one, CBOR
/// here (high-frequency small token frames — the self-delimited document form
/// skips the per-frame JSON quoting overhead).
pub async fn register(engine: &aura_engine::Engine) -> anyhow::Result<()> {
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
                // THE DECLARATION CARRIES ITS OWN GATE: this event is served only to a sender
                // with a bound user, and the flag rides the `on_as` that already names the event,
                // the key and the entry (prism ADR-0017 §2). Written once, here — the guest names
                // only the entry, so a table in the guest keyed by event would repeat this name.
                .gated()
                // WHERE A DEPLOYMENT IS CONFIGURED: the facts are rows in this type's ns, so the
                // door that writes them is an event too (auth: `ui:gravity-model.save`). A venue
                // never learns the word — this is the application's own vocabulary, as much as
                // `ui:turn.submit` is.
                // Rounds OVERLAP, in ONE process (ADR-0046): a turn runs as long as
                // the model takes, and a device's next message is served while it
                // does. The turn holds no state outside a call's own stack and its
                // own streams, and induction pairs every reply with its request id,
                // so the instance is `unbounded` — no queue, no second process.
                .unbounded()
                .on_as("ui:gravity-model.save", "device", "save")
                .gated()
                // THE CANCEL (ADR-0008): keyed by NOTHING — the singleton instance of this same
                // type. That is the whole point: a delivery to an instance that is running a turn
                // waits behind it, while the singleton is never busy, so it writes the marker and
                // returns. The turn (device-keyed) reads the marker at its next chunk boundary.
                // NOT gated at the route, and that is deliberate: a gate is a binding on the
                // ADDRESSED instance, and the singleton has no device and no user to bind. The
                // two halves move into code: the handler refuses a sender with no bound user, and
                // the TURN honours the marker only when it names its own member (so a stranger's
                // row can be written and still cannot stop anything).
                .on_as("ui:turn.cancel", "", "cancel")
                // And the SCREEN (ADR-0007 §4: management is a view). It draws forms whose buttons
                // emit the save event above, so the vocabulary is discoverable from the venue
                // itself and an AI drives exactly what a person clicks.
                .on_as("ui:gravity-model", "device", "screen")
                .gated()
                // The shell a renderer connects to: the layout is the application's answer, and
                // this booth is the application in this stack.
                .on_as("ui:connect", "device", "shell"),
        )
        .await?;
    Ok(())
}
