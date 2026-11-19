# 0012 — The data path is not the venue's

> **Languages:** [English](0012-the-data-path-is-not-the-venues.md) (primary) · [中文](0012-the-data-path-is-not-the-venues.zh-CN.md)

**Status:** Accepted (2026-11-18)
**Date:** 2026-11-18

## Context

A turn produces more than text: a generated report, a scraped dataset, a built image, a file that has to travel from the machine that produced it to the machine that asked for it. The venue is the one place every client and every node already has a connection to, which makes it the obvious thing to route those bytes through — and that is exactly the mistake this record names.

Two facts decide it. The venue's connection plane terminates device connections and node mounts; it is sized for instructions, not for payloads. And a turn's real work happens on a node, next to the data it produces, in an environment that may already hold the file.

## Decision

**RULING — the control plane carries instructions and result summaries; artifacts move on the node; what a node can reach is the skill's declaration, not the venue's.**

1. **A result stays message-sized.** What comes back to the plane is the answer, not the artifacts the answer is about. A tool's result names what it produced — a path, a reference, a count — the way a command prints a location rather than a file.
2. **The operation moves its own artifacts.** A step that produces a large thing writes it where that thing belongs: the node's local filesystem, the deployment's own transfer tooling, or a transport operation the skill declares. Gravity's arguments are instruction semantics — paths, options, fragments — and are bounded by being instructions.
3. **Reachability is the skill's property.** Whether a node can reach a target at all (a direct connection, the same VPN, a mount that exists on that machine) is declared with the skill on gravity's side. The venue neither knows nor governs it: it routes an event to a device, and an event to a mount, and has no opinion about what those two can do to each other.
4. **An inbound cap on the plane guards anomalies only.** A size limit at the venue's door exists so a malformed or hostile frame cannot exhaust it. It is not a data-plane design, and no threshold chosen there may be read as a statement about how large a result may be.

## Considered and rejected

- **Relay artifacts through the venue, so everything travels one channel.** It makes the plane the bandwidth of every artifact, and that cost grows with the size of the work rather than with the plane's actual job (delivering instructions to booths and nodes). The same rejection appears on the resource path, where material is minted by the plane and spent by the node directly (effector ADR-0003 §8) — the property that matters is "nothing relays", not "nothing crosses".
- **Put reachability in the venue's routing table.** The venue holds devices, connections and mounts. It does not know a deployment's network topology, and a second copy of that knowledge in the plane would drift from the machines it describes.
- **Carry the bytes inside the tool call.** The call is the instruction; bytes inside it turn the control plane into the data path by accident, and every hop that repeats the call repeats the payload.
- **Give the delivered code the transfer credentials and let it push.** It would put a data-plane credential inside the environment that runs unreviewed code — the thing the resource contract exists to prevent.

## Consequences

- A skill that moves data declares the transport it needs; the turn passes paths and options. Nothing about the transfer is modelled in the venue.
- A data-path failure is the operation's failure, reported as a tool result the model reads. The venue's own health says nothing about it.
- The venue's throughput is never measured by artifact size, so the numbers that matter for it (connections, dispatch latency) stay meaningful.
- Related: effector ADR-0003 §8–§9 (material minted by the plane, spent by the node, never relayed; `EFFECTOR_RESOURCES_ROOT`), and this repo's ADR-0001 §7 (a mount is reached by the manifest, not by a name the model invents).