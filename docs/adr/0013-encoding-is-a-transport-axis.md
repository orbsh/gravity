# 0013 — Encoding is a transport axis: two content modes × two codecs

> **Languages:** [English](0013-encoding-is-a-transport-axis.md) (primary) · [中文](0013-encoding-is-a-transport-axis.zh-CN.md)

**Status:** Accepted (2026-11-19). It fixes how the content of a frame is ENCODED on its way out; it adds no third output mode.
**Date:** 2026-11-19

## Context

A turn says one of two things, chosen by its call (ADR-0009): text, or accrete frames. That is the CONTENT axis, and it is fixed by what the caller asked for.

Compactness is not a kind of content. The same frame is the same frame whether it crosses a wire as text or as CBOR — what changes is how many bytes it takes and whether a person can read it without a decoder. Treating "CBOR" as a third `protocol` value would be the tempting mistake: it would give two content modes to JSON and one to CBOR, and every consumer would start branching on a distinction that has nothing to do with what the turn drew.

## Decision

**RULING — encoding is orthogonal to content: two content modes × two encodings = four combinations, not three output modes.**

1. **Each leg answers for itself; nothing propagates.** The codec is a property of a CONNECTION, not of a request, so it is settled at the handshake and never rides in the payload. A client may ask prism for `accrete-json` or `accrete-cbor` (subprotocol first, `?codec=` as the simplified entry; absent means `json`, an unrecognized value is a refused handshake rather than a silent fallback, and `PRISM_CODECS` can pin a deployment to one codec). A booth declares its own encoding when it is started. prism transcodes at the seam between the two when it must. The client's choice does not travel down the chain, and the booth's declaration does not travel up it.

2. **A booth declares what it can speak.** `Body::Script::encoding` (`json` | `cbor`) is set at spawn and reaches the guest as `INDUCTION_ENCODING`; gravity declares `cbor`. Declaring nothing means `json` — the wrong guess here does not produce an error, it produces garbage, so the safe default is the readable one.

3. **JSON is the default on the client leg, and stays the default.** The whole debug story — fluxen's `stage console`, `stage tojson <dump>`, the walkthrough — depends on frames a person can read. Compaction is opt-in per connection.

4. **The payload says which it is, by its shape.** On the JSON wire the content is `data = {"$cbor": "<base64 of the CBOR document>"}`: one reserved key, so a receiver discriminates without out-of-band configuration. On the CBOR wire the content is a real CBOR byte string — `induction-protocol`'s `payload` module splits it on the way in and reassembles it on the way out, so a `Value`-typed reader still sees the mapping while the bytes stay bytes.

5. **Equivalence is tested, not assumed.** The two encodings must decode to the SAME value: induction-guest proves it over a real CBOR pipe (both `emit` paths, one content value), gravity proves it across the guest boundary (one frame, both encodings, both decoded), and prism proves it at its single decode funnel (`ui::decode_content`). A parse assertion would hide a codec that quietly changes a value; an equality assertion cannot.

## Measured

The same ten accrete frames, on this machine (2026-11-19), by gravity's `the_four_payload_sizes`:

| spelling | bytes |
|---|---|
| pretty JSON | 18 257 |
| compact JSON | 6 182 |
| `{"$cbor": base64}` (the frame as it exists at aura's realm seam) | 6 168 |
| raw CBOR with a byte string (the frame on the pipe) | 4 618 |

Two answers come out of that table and both are needed. Binary is worth **25%** over compact JSON — but only where the bytes stay bytes. At the realm seam, where `Handler = Fn(&Ctx, Value)` forces a `serde_json::Value`, the same frame arrives as 6 168 B, a 1.3% improvement on plain JSON that would not justify a second code path by itself.

## Consequences

- **"End-to-end binary" is not true yet, and the record says where it stops.** The `Value`-typed dispatch payload (`HostFn`, `Realm::emit`, `aura_booth::Handler`) re-spells bytes as base64; removing that is a change to aura's payload type, not to the codec. Until then the byte path pays off on the pipe and is washed out at the seam.
- **The pipe codec is independent of the content codec.** Switching one does not switch the other, and a JSON-wire link carries a `$cbor` payload correctly — that is what "axis" means here.
- **A CBOR frame is not plannable by eye.** An operator debugging it decodes first; the frame-dump test does exactly that (`GRAVITY_FRAMES_OUT` in CBOR mode, decoded to JSON, then handed to `stage tojson` unchanged).
- **The receiver carries both shapes forever.** There is no version marker: the marker's presence is the discriminator on the JSON wire, and a receiver that drops that branch silently starts drawing nothing, which the equality test catches.
- **Adding a content mode stays cheap.** A third mode costs one more `emit` caller, not a codec matrix: the funnel already answers the encoding question once for all of them.
- **The negotiation is per connection, so a deployment cannot mix.** Two codecs on one connection is a broken connection, which is why the choice is made at the handshake rather than per message.

## Related records

ADR-0009 (one turn, two renderings — the content axis), ADR-0002 (the CLI is a client of the venue), ADR-0012 (a failure is a value), effector ADR-0004 (the carrier is an interface), induction's `induction-protocol::payload` (`$cbor` and the byte-string split), prism's `ui::decode_content` (the one decode site).