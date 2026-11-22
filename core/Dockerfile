# ── Build stage ──────────────────────────────────────────────────────────────
FROM rust:1-slim AS builder

# A release build fetches several hundred crates. The default is 3 retries with a short
# backoff, which a brief network blip outlasts and which fails the whole build. Ten retries
# and a longer per-request timeout ride out a flaky link; neither changes the output.
ENV CARGO_NET_RETRY=10 \
    CARGO_HTTP_TIMEOUT=120

WORKDIR /build

# Copy the whole workspace and build the core binary in release mode.
# Cache mounts keep the cargo registry and the target dir between local builds, so a code
# change recompiles only the touched crates. A cache mount is not part of the layer: the
# binary is copied out of it for the runtime stage.
COPY . .

# rust-toolchain.toml asks for channel "stable". This image ships a pinned release under its
# version number, not under the name "stable", so leaving the file in place makes rustup
# download the stable channel manifest from static.rust-lang.org on every build — a network
# dependency that has already failed builds. The image's own toolchain is that same stable
# release, so dropping the file selects it and touches the network for nothing.
RUN rm -f rust-toolchain.toml

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --bin otw-core \
 && cp target/release/otw-core /otw-core

# Pre-create the upload dir owned by the runtime user (65532). When the named
# volume is first mounted here it inherits this ownership, so the non-root
# process can write to it. Distroless has no shell to chown at runtime.
RUN mkdir -p /data/uploads && chown -R 65532:65532 /data

# ── Runtime stage ────────────────────────────────────────────────────────────
# Distroless: tiny, no shell, smaller attack surface (M11 security-by-design).
FROM gcr.io/distroless/cc-debian12 AS runtime

WORKDIR /app
COPY --from=builder /otw-core /app/otw-core
# Carry over the upload dir with its 65532 ownership for the volume mountpoint.
COPY --from=builder --chown=65532:65532 /data /data

# Defaults; overridden by the compose environment / .env.
ENV OTW_CORE_HOST=0.0.0.0 \
    OTW_CORE_PORT=8080 \
    OTW_LOG=info

EXPOSE 8080
USER 65532:65532

ENTRYPOINT ["/app/otw-core"]
