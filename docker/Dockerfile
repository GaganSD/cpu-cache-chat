# syntax=docker/dockerfile:1
# Build natively on ARM64 (OrbStack or Linux), or natively on x86-64 Linux.
# Multi-platform index digests verified with buildx imagetools inspect.
ARG RUST_IMAGE=rust:1.96.1-bookworm@sha256:a339861ae23e9abb272cea45dfafde21760d2ce6577a70f8a926153677902663
ARG RUNTIME_IMAGE=debian:bookworm-slim@sha256:99c0b25c46d0214d5a2091092e7908907dcaecd6a962cde0ac700acee222b6f3
FROM ${RUST_IMAGE} AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock rust-toolchain.toml rustfmt.toml ./
COPY crates ./crates
ARG VCS_REF=unknown
ENV CCC_BUILD_REVISION=${VCS_REF}
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    CARGO_BUILD_JOBS=2 cargo build --locked --release -p cpu-cache-chat -p ccc-game \
    && cp target/release/cpu-cache-chat target/release/ccc-verify-game /

FROM ${RUNTIME_IMAGE} AS runtime
ARG SOURCE_URL=https://github.com/GaganSD/cpu-cache-chat
ARG VCS_REF=unknown
LABEL org.opencontainers.image.source=${SOURCE_URL} \
      org.opencontainers.image.revision=${VCS_REF}
COPY --from=build /cpu-cache-chat /ccc-verify-game /usr/local/bin/
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
COPY agent/README.md agent/package.json /opt/ccc/agent/
COPY agent/src /opt/ccc/agent/src
# The base image does not include Node, Pi, model weights, or secrets.
USER 65532:65532
WORKDIR /tmp
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["--help"]
