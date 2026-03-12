# ---------------------------------------------------------------------------
# Kyomi Rust backend — production multi-stage build
# ---------------------------------------------------------------------------
# Build context must be the repo root:
#   docker build -f apps/backend-rust/Dockerfile .
#
# Prerequisites: The CI workflow builds the MCP chart app BEFORE this
# Docker build, so chart_app.html is available in the build context.
#
# Uses BuildKit cache mounts for fast incremental builds.
#
# Runtime: scratch with only the binary, runtime data, CA certs, and the
# minimal set of shared libraries detected by ldd (glibc, libstdc++, libssl
# — ONNX Runtime needs libstdc++, webauthn-rs needs libssl/libcrypto).
# All outbound TLS uses rustls; openssl is only used locally by webauthn-rs
# for X.509 cert parsing.
# ---------------------------------------------------------------------------

# ===== Stage 0: Build frontend =====
FROM node:20-slim AS frontend-builder
WORKDIR /build
# Copy manifests only — cache npm ci unless deps change
COPY apps/frontend/package.json apps/frontend/package-lock.json ./apps/frontend/
COPY packages/chart-header/package.json ./packages/chart-header/package.json
COPY packages/chartml-transform/package.json ./packages/chartml-transform/package.json
RUN cd apps/frontend && npm ci
# Copy full source for the build
COPY apps/frontend/ ./apps/frontend/
COPY packages/chart-header/ ./packages/chart-header/
COPY packages/chartml-transform/ ./packages/chartml-transform/
RUN cd apps/frontend && npm run build

# ===== Stage 1: Build Rust binary =====
FROM rust:1-bookworm AS builder

WORKDIR /build

# Copy files needed for compilation (include_str! paths require repo layout)
COPY apps/backend-rust/ ./apps/backend-rust/
COPY data/chartml-spec/ ./data/chartml-spec/
COPY apps/mcp-chart-app/chart_app.html ./apps/mcp-chart-app/chart_app.html
COPY apps/marketing/public/kyomi_email_logo.png ./apps/marketing/public/kyomi_email_logo.png
COPY shared/constants.toml ./shared/constants.toml

# Frontend dist — rust-embed reads this at compile time to embed into the binary
COPY --from=frontend-builder /build/apps/frontend/dist ./apps/frontend/dist

# All SQL queries use runtime string-based sqlx::query() — no compile-time
# checking, no .sqlx/ cache, and no DATABASE_URL needed at build time.

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/apps/backend-rust/target \
    cd apps/backend-rust && cargo build --release && \
    cp target/release/kyomi /tmp/kyomi

# Collect the minimal shared libraries the binary needs at runtime.
# ONNX Runtime (via fastembed) requires libstdc++ and glibc.
# Also include glibc NSS modules — getaddrinfo() loads these via dlopen()
# at runtime, so ldd doesn't list them, but they're needed for DNS resolution.
RUN mkdir -p /tmp/runtime-libs && \
    ldd /tmp/kyomi | awk '/=>/ {print $3}' | while read lib; do cp "$lib" /tmp/runtime-libs/; done && \
    cp /lib64/ld-linux-x86-64.so.2 /tmp/runtime-libs/ && \
    cp /lib/x86_64-linux-gnu/libnss_dns.so.2 /tmp/runtime-libs/ && \
    cp /lib/x86_64-linux-gnu/libnss_files.so.2 /tmp/runtime-libs/ && \
    cp /lib/x86_64-linux-gnu/libresolv.so.2 /tmp/runtime-libs/

# Create writable /tmp for ONNX Runtime temp files.
RUN mkdir -p /tmp/scratch-tmp && touch /tmp/scratch-tmp/.keep

# ===== Stage 2: Scratch runtime =====
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

# Runtime data files (chartml spec is read at runtime for the chat agent)
COPY shared/constants.toml /app/shared/constants.toml
COPY data/chartml-spec/ /app/data/chartml-spec/

# Writable /tmp for ONNX Runtime temp files
COPY --from=builder --chown=1000:1000 /tmp/scratch-tmp /tmp

# Tell the binary where to find runtime data and CA certs
# Note: ONNX model is now embedded via include_bytes!() — no runtime cache needed
ENV SHARED_CONSTANTS_PATH=/app/shared/constants.toml \
    CHARTML_SPEC_DIR=/app/data/chartml-spec \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    HOME=/tmp \
    TMPDIR=/tmp

USER 1000
ENTRYPOINT ["/app/kyomi"]
