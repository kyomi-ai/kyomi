# ---------------------------------------------------------------------------
# Kyomi Rust backend — production multi-stage build
# ---------------------------------------------------------------------------
# Build context must be the repo root:
#   docker build -f apps/server/Dockerfile .
#
# Prerequisites: The CI workflow builds the MCP chart app BEFORE this
# Docker build, so chart_app.html is available in the build context.
#
# Uses BuildKit cache mounts for fast incremental builds.
#
# Runtime: scratch with only the binary, runtime data, CA certs, and the
# minimal set of shared libraries detected by ldd (glibc, libstdc++, libssl
# — ring crypto needs libstdc++, webauthn-rs needs libssl/libcrypto).
# All outbound TLS uses rustls; openssl is only used locally by webauthn-rs
# for X.509 cert parsing.
# ---------------------------------------------------------------------------

# ===== Stage 0: Build Rust binary =====
FROM rust:1-bookworm AS builder

WORKDIR /build

# Copy Cargo workspace and crates
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY apps/server/ ./apps/server/
COPY enterprise/ ./enterprise/

# Remove desktop from workspace members (not needed for server Docker build)
RUN sed -i '/"apps\/desktop",/d' Cargo.toml

# Copy files needed for include_str!/include_bytes! paths
COPY data/ ./data/
COPY apps/mcp-chart-app/chart_app.html ./apps/mcp-chart-app/chart_app.html
COPY assets/kyomi_email_logo.png ./assets/kyomi_email_logo.png

# All SQL queries use runtime string-based sqlx::query() — no compile-time
# checking, no .sqlx/ cache, and no DATABASE_URL needed at build time.

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

# Runtime data files (chartml spec is read at runtime for the chat agent)
COPY data/constants.toml /app/data/constants.toml
COPY data/chartml-spec/ /app/data/chartml-spec/

# Writable /tmp
COPY --from=builder --chown=1000:1000 /tmp/scratch-tmp /tmp

# Tell the binary where to find runtime data and CA certs
# Note: ML model is embedded via include_bytes!() — no runtime model files needed
ENV SHARED_CONSTANTS_PATH=/app/data/constants.toml \
    CHARTML_SPEC_DIR=/app/data/chartml-spec \
    SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt \
    HOME=/tmp \
    TMPDIR=/tmp

USER 1000
ENTRYPOINT ["/app/kyomi"]
