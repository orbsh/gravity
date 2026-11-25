# The guest image: `gravity-turn` plus the `gravity` CLI.
# (`gravity-provider` is gone: gravity 4d61f2c made the provider a stream on a thread inside
# the turn, so a second guest binary would be a second lifetime nobody needs.)
#
# gravity has no daemon — one turn is one process, and prism spawns it (ADR-0002
# for the client, ADR-0004 for the tool list). So this image is not "the gravity
# server": it is the pair of binaries every turn needs, published under one tag that
# prism's image copies from. Nothing here listens on a port.
#
# Build context is workspace-shaped: this repo at `gravity/`, its path dependencies
# beside it (memory-stack/build.sh assembles the tree).
FROM rust:1-bookworm AS build

WORKDIR /src
COPY aura/ aura/
COPY effector/ effector/
COPY induction/ induction/
COPY krystallizer/ krystallizer/
# a PATH dependency of gravity's booth: the manifest resolves it before any build step
COPY schematic/ schematic/
COPY gravity/ gravity/

# One guest binary: `gravity-turn` speaks the turn protocol on stdio and opens the model
# endpoint itself. The provider used to be a second process so a stalled inference could not
# stall the turn's pipes; it is a thread inside the turn now, which is why only one binary is
# copied here.
# Copied out of `target/` in the same step: a build harness may mount that directory
# as an incremental cache (memory-stack/build.sh does) and a mounted directory is not
# part of the stage's filesystem, so `COPY --from` would come up empty.
RUN cargo build --release --locked \
      --manifest-path gravity/Cargo.toml \
      -p gravity-booth --bins -p gravity-cli \
 && mkdir -p /out \
 && cp gravity/target/release/gravity-turn gravity/target/release/gravity /out/

FROM debian:bookworm-slim

# ca-certificates is not optional here: the provider opens the model endpoint over
# TLS, and a guest image without a trust store turns every turn into a certificate
# error that looks like a provider outage.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /out/gravity-turn /usr/local/bin/gravity-turn
COPY --from=build /out/gravity /usr/local/bin/gravity

# /usr/local/bin, not /bin: the guests are looked up by name through PATH
# (`GRAVITY_TURN_BIN` / `GRAVITY_PROVIDER_BIN` point at them), and every other image
# in the fleet keeps hand-installed binaries in the same place.
CMD ["/usr/local/bin/gravity", "--help"]