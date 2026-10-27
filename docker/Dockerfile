# syntax=docker/dockerfile:1
# Native Linux image. Build on ARM64 for Apple Silicon / OrbStack.
# This is container isolation, not a microVM.

ARG RUST_IMAGE=rust:1.86-bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim

FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY Cargo.toml rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
RUN cargo build --release -p cpu-cache-chat

FROM ${RUNTIME_IMAGE} AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 65532 --create-home ccc
COPY --from=build /src/target/release/cpu-cache-chat /usr/local/bin/cpu-cache-chat
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
COPY agent /opt/ccc/agent
RUN chmod +x /usr/local/bin/entrypoint.sh \
    && mkdir -p /tmp/ccc \
    && chown -R ccc:ccc /tmp/ccc /opt/ccc
USER 65532:65532
WORKDIR /home/ccc
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--help"]
