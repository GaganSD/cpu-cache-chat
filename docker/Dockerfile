# syntax=docker/dockerfile:1
# Build natively on ARM64 (OrbStack or Linux), or natively on x86-64 Linux.
ARG RUST_IMAGE=rust:bookworm
ARG RUNTIME_IMAGE=debian:bookworm-slim
FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
RUN cargo build --locked --release -p cpu-cache-chat

FROM ${RUNTIME_IMAGE} AS runtime
COPY --from=build /src/target/release/cpu-cache-chat /usr/local/bin/cpu-cache-chat
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
COPY agent/README.md agent/package.json /opt/ccc/agent/
COPY agent/src /opt/ccc/agent/src
# The agent is a stub: a later optional image can provision Node, Pi and a
# container-local llama.cpp runtime and model separately; none are included here.
USER 65532:65532
WORKDIR /tmp
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--help"]
