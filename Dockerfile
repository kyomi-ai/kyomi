# ---------------------------------------------------------------------------
# Kyomi — unified Docker image
# ---------------------------------------------------------------------------
# One image for both self-hosted (Community Edition) and SaaS (app.kyomi.ai).
# Mode is determined at runtime by environment variables (DATABASE_URL,
# REDIS_URL, AUTH_METHOD, etc.) — the compiled binary is identical.
#
# Build context must be the repo root:
#   docker build -t kyomi .
#
# Prerequisites: the CI workflow (or manual build) must produce these
# artifacts BEFORE running docker build:
#   1. crates/kyomi-ui/dist/         — Leptos WASM frontend (trunk build --release)
#   2. apps/mcp-chart-app/chart_app.html — MCP chart viewer (npm run build)
#
# Self-hosted quickstart:
#   docker run -v kyomi-data:/data -p 3000:3000 -e ANTHROPIC_API_KEY=sk-... kyomi
#
# Everything is embedded in the binary: frontend assets, ML model, constants,
# chartml spec. No runtime data files needed.
# ---------------------------------------------------------------------------

# ===== Stage 0: Build Rust binary =====
FROM rust:1-bookworm AS builder

WORKDIR /build

# Copy Cargo workspace and crates
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY apps/server/ ./apps/server/
COPY enterprise/ ./enterprise/

# Remove desktop from workspace members (not built in server Docker image)
RUN sed -i '/"apps\/desktop",/d' Cargo.toml

# Strip the local-dev [patch.crates-io] block — prod uses crates.io versions.
RUN sed -i '/# BEGIN_LOCAL_DEV_PATCHES/,/# END_LOCAL_DEV_PATCHES/d' Cargo.toml

# Copy files needed for include_str!/include_bytes! paths. The Leptos WASM
# frontend (crates/kyomi-ui/dist/) is pre-built on the host and COPY'd in —
# keeps this Dockerfile cross-arch friendly since the heavy trunk/wasm-bindgen
# tooling runs on the native amd64 host, not inside QEMU-emulated containers.
COPY data/ ./data/
COPY apps/mcp-chart-app/chart_app.html ./apps/mcp-chart-app/chart_app.html
COPY assets/kyomi_email_logo.png ./assets/kyomi_email_logo.png
COPY crates/kyomi-ui/dist/ ./crates/kyomi-ui/dist/

# All SQL queries use runtime string-based sqlx::query() — no compile-time
# checking, no .sqlx/ cache, and no DATABASE_URL needed at build time.

# Build the binary. The BGE-small-en-v1.5 model is downloaded by build.rs
# and embedded via include_bytes!() — no runtime model files needed.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release -p kyomi-server && \
    cp target/release/kyomi /tmp/kyomi

# Collect the minimal shared libraries the binary needs at runtime.
# ring crypto requires libstdc++ and glibc.
# Also include glibc NSS modules — getaddrinfo() loads these via dlopen()
# at runtime, so ldd doesn't list them, but they're needed for DNS resolution.
RUN mkdir -p /tmp/runtime-libs && \
    ldd /tmp/kyomi | awk '/=>/ {print $3}' | while read lib; do cp "$lib" /tmp/runtime-libs/; done && \
    cp /lib64/ld-linux-x86-64.so.2 /tmp/runtime-libs/ && \
    cp /lib/x86_64-linux-gnu/libnss_dns.so.2 /tmp/runtime-libs/ && \
    cp /lib/x86_64-linux-gnu/libnss_files.so.2 /tmp/runtime-libs/ && \
    cp /lib/x86_64-linux-gnu/libresolv.so.2 /tmp/runtime-libs/

# Create writable /tmp for runtime temp files.
RUN mkdir -p /tmp/scratch-tmp && touch /tmp/scratch-tmp/.keep

# ===== Stage 1: Scratch runtime =====
FROM scratch

# Dynamic linker + shared libraries
COPY --from=builder /tmp/runtime-libs/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
COPY --from=builder /tmp/runtime-libs/ /lib/x86_64-linux-gnu/

# CA certificates (for outbound TLS to Anthropic, Stripe, HuggingFace, etc.)
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# DNS config — glibc needs nsswitch.conf to know how to resolve hostnames
COPY --from=builder /etc/nsswitch.conf /etc/nsswitch.conf

# Binary
COPY --from=builder /tmp/kyomi /app/kyomi

# Writable /tmp
COPY --from=builder --chown=1000:1000 /tmp/scratch-tmp /tmp

ENV SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    HOME=/tmp \
    TMPDIR=/tmp

# Data directory — SQLite database and auto-generated secrets live here.
# k8s deployments override the mount; standalone users need the volume.
ENV DATA_DIR=/data
VOLUME /data

EXPOSE 3000

# Health check using the built-in subcommand (no curl needed on scratch)
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/kyomi", "health"]

USER 1000

ENTRYPOINT ["/app/kyomi"]
