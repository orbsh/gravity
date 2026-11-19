//! The WIRE: how a finished frame's CONTENT is encoded on its way to the guest.
//!
//! gravity has TWO content modes — text, and accrete frames — and TWO encodings: JSON
//! (readable by eye) and CBOR (compact, one document). Encoding is a TRANSPORT axis,
//! orthogonal to the content: two modes × two encodings = four combinations, **not three
//! output modes** (ADR-0013).
//!
//! WHOSE ANSWER IS IT? The BOOTH's, declared once. A booth declares the codec it can
//! produce when it enters — the spawn spec's `encoding`
//! (`gravity_booth::register`'s `.encoded(…)`, [`DECLARED`] here) — and the carrier hands
//! that same declaration back to this process as `INDUCTION_ENCODING`. So the emitter needs
//! no negotiation and no per-request context: it produces what it said it would produce,
//! and the VENUE (prism) transcodes at its own boundary for whatever each client
//! negotiated. Codec is per LEG at every hop, never end-to-end.
//!
//! `GRAVITY_WIRE` is therefore not a switch any more, only a FALLBACK: [`service_default`]
//! answers when nothing declared one (a guest run outside a carrier — a test, a `cargo run`,
//! a probe), so the JSON path stays byte-identical for anyone who never declared anything.

use serde_json::Value;

/// The environment variable that names the SERVICE DEFAULT encoding. Absent, unparsable, or
/// empty is JSON: the default is the debuggable one, and a typo must not silently switch a
/// deployment to a wire its operator cannot read.
///
/// A fallback, not the switch: a spawned guest always knows what it declared (the carrier
/// sets `INDUCTION_ENCODING`), so this variable only decides the case of no declaration.
pub const ENV: &str = "GRAVITY_WIRE";

/// WHAT THIS BOOTH SPEAKS — the declaration, in one place, read twice: [`register`] turns it
/// into the spawn spec's `encoding` (which the carrier passes to the guest), and the venue
/// (`prism`) reads it to know what a frame from this leg will be.
///
/// Rust has `ciborium`, so the answer is binary; a booth in a language without a CBOR codec
/// declares JSON there and the whole path degrades to today's readable wire. A booth that
/// declares NOTHING is JSON — a wrong guess in the other direction produces frames nobody
/// can decode, which is a garbage-on-the-wire failure rather than an error.
///
/// [`register`]: crate::register
pub const DECLARED: Codec = Codec::Cbor;

/// The content encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Codec {
    /// The envelope as JSON-typed `data` — what every deployment sent before this axis
    /// existed, and what the debug story (fluxen's `stage console`, the walkthrough) reads.
    #[default]
    Json,
    /// The same value, one CBOR document. Shorter on the wire; unreadable by eye.
    Cbor,
}

impl Codec {
    /// The wire spelling. Lower case only: these strings are a contract between a spawn
    /// spec, a carrier's environment and this code — not a place to be forgiving.
    pub fn as_str(self) -> &'static str {
        match self {
            Codec::Json => "json",
            Codec::Cbor => "cbor",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "json" => Some(Codec::Json),
            "cbor" => Some(Codec::Cbor),
            _ => None,
        }
    }

    /// The declared service default, or [`Codec::default`] with a line saying what was
    /// ignored. A value nobody recognizes is stated rather than obeyed: a deployment that
    /// meant CBOR and finds it running JSON needs the log to say why.
    pub fn from_env() -> Self {
        match std::env::var(ENV) {
            Ok(v) if v.trim().is_empty() => Self::default(),
            Ok(v) => match Self::parse(v.trim()) {
                Some(codec) => codec,
                None => {
                    eprintln!(
                        "gravity: {ENV}=`{v}` is not `json` or `cbor` — keeping {}",
                        Self::default().as_str()
                    );
                    Self::default()
                }
            },
            Err(_) => Self::default(),
        }
    }

    /// One value, encoded. `Json` is the `data` a caller would have written; `Cbor` is the
    /// document `ctx.emit_bytes` carries.
    pub fn encode(self, value: &Value) -> anyhow::Result<Vec<u8>> {
        match self {
            Codec::Json => Ok(serde_json::to_vec(value)?),
            Codec::Cbor => {
                let mut buf = Vec::new();
                ciborium::into_writer(value, &mut buf)?;
                Ok(buf)
            }
        }
    }
}

/// The service's ONE fallback decision, read once from [`ENV`].
///
/// Memoized because the environment is a boot fact. It answers only when the booth declared
/// nothing (see [`declared`]): a deployment-wide pin still has a use — a booth started by
/// hand — but it is a DEFAULT, never an override of the declaration.
pub fn service_default() -> Codec {
    static DECIDED: std::sync::OnceLock<Codec> = std::sync::OnceLock::new();
    *DECIDED.get_or_init(Codec::from_env)
}

/// What THIS process declared it produces: the carrier's echo of the spawn spec's
/// `encoding` (`INDUCTION_ENCODING`), else the service default.
///
/// The declaration is written once ([`DECLARED`]) and travels as an environment variable
/// because that is how the carrier states it to a guest — never as an argv, which would
/// break a spawn spec's arity. A value we cannot read does not fail the link (a booth must
/// keep serving); it falls back to the service default and says so on stderr.
pub fn declared() -> Codec {
    match std::env::var(induction_guest::ENCODING_ENV) {
        Ok(v) => match Codec::parse(v.trim()) {
            Some(codec) => codec,
            None => {
                eprintln!(
                    "gravity: {}={v:?} is not `json` or `cbor` — keeping the service default {}",
                    induction_guest::ENCODING_ENV,
                    service_default().as_str()
                );
                service_default()
            }
        },
        Err(_) => service_default(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;
    use serde_json::json;

    /// One finished frame, in the shape the funnel hands over: the venue envelope around
    /// an accrete operation list. Deliberately not flat — a switch that only worked on
    /// scalars would pass a weaker test.
    fn envelope() -> Value {
        json!({
            "name": "ui:turn.done",
            "island": "gravity-model",
            "turn": "7",
            "user": "u-1",
            "data": [{
                "action": "append",
                "slot": "messages",
                "data": {
                    "id": "7_1",
                    "type": "frame",
                    "attrs": { "selector": "assistant" },
                    "children": [{ "type": "text", "source": { "default": "there" } }],
                },
            }],
        })
    }

    /// The equivalence this axis rests on: one value, both encodings, both DECODED, and
    /// the two decoded values EQUAL. Not "each parses" — equal, because a codec that
    /// parses into a slightly different value is the failure a parse assertion hides.
    #[test]
    fn both_encodings_decode_to_the_same_value() {
        let frame = envelope();
        let via_json: Value =
            serde_json::from_slice(&Codec::Json.encode(&frame).unwrap()).expect("json decodes");
        let via_cbor: Value = ciborium::from_reader(
            Codec::Cbor.encode(&frame).unwrap().as_slice(),
        )
        .expect("cbor decodes");
        assert_eq!(via_json, via_cbor);
        assert_eq!(via_json, frame, "and both are the frame that went in");
    }

    /// The same equality ACROSS the guest boundary, which is where the two paths actually
    /// diverge: the JSON path hands over the envelope; the CBOR path hands over
    /// `{"$cbor": …}`. What the receiver gets back must be the same value.
    #[test]
    fn the_cbor_payload_decodes_to_what_the_json_path_sent() {
        let frame = envelope();
        let sent_json = frame.clone();
        let sent_cbor = induction_guest::cbor_payload(&Codec::Cbor.encode(&frame).unwrap());
        // The receiver's two steps: unwrap, then decode (prism does exactly this).
        let bytes = induction_guest::payload_bytes(&sent_cbor).expect("a tagged payload");
        let received: Value = ciborium::from_reader(bytes.as_slice()).expect("decode");
        assert_eq!(received, sent_json);
        assert!(
            induction_guest::payload_bytes(&sent_json).is_none(),
            "the JSON path must not look tagged"
        );
    }

    /// The two spellings, the default, and a refusal that does not guess.
    #[test]
    fn the_names_are_json_or_cbor_and_default_to_json() {
        assert_eq!(Codec::parse("json"), Some(Codec::Json));
        assert_eq!(Codec::parse("cbor"), Some(Codec::Cbor));
        assert_eq!(Codec::parse("CBOR"), None, "the spelling is part of the contract");
        assert_eq!(Codec::parse("msgpack"), None);
        assert_eq!(Codec::default(), Codec::Json, "readable stays the default");
        for codec in [Codec::Json, Codec::Cbor] {
            assert_eq!(Codec::parse(codec.as_str()), Some(codec));
        }
    }

    /// CBOR mode is not a rounding of JSON: it is smaller on a frame the size of a real
    /// answer, which is the only reason to carry the second codec at all.
    #[test]
    fn cbor_is_smaller_than_json_for_a_real_frame() {
        let frame = envelope();
        let json = Codec::Json.encode(&frame).unwrap();
        let cbor = Codec::Cbor.encode(&frame).unwrap();
        assert!(
            cbor.len() < json.len(),
            "cbor {} bytes vs json {}",
            cbor.len(),
            json.len()
        );
    }

    /// The declaration is one word, and the emitter's view of it is the carrier's echo:
    /// with nothing declared (a test process, a `cargo run`) it is the service default,
    /// which is JSON unless an operator said otherwise.
    #[test]
    fn with_no_declaration_the_emitter_falls_back_to_the_service_default() {
        // The test process sets neither variable; assert the shape of the rule rather than
        // the ambient value, since another test binary may run beside this one.
        if std::env::var(induction_guest::ENCODING_ENV).is_err() {
            assert_eq!(declared(), service_default());
        }
        assert_eq!(DECLARED, Codec::Cbor, "gravity declares binary at registration");
    }
}