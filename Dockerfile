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
#   1. target/release/kyomi             — server binary (cargo build --release -p kyomi-server)
#   2. crates/kyomi-ui/dist/            — Leptos WASM frontend (trunk build --release)
#   3. apps/mcp-chart-app-wasm/chart_app.html ��� MCP chart viewer (bash build.sh)
#
# IMPORTANT: The server binary and WASM frontend MUST be compiled on the same
# host so that CARGO_MANIFEST_DIR is identical for both. Leptos server_fn hashes
# include this path — building one on the host and the other inside Docker
# produces mismatched hashes and a completely broken app.
#
# Self-hosted quickstart:
#   docker run -v kyomi-data:/data -p 3000:3000 -e ANTHROPIC_API_KEY=sk-... kyomi
#
# Everything is embedded in the binary: frontend assets, ML model, constants,
# chartml spec. No runtime data files needed.
# ---------------------------------------------------------------------------

# ===== Stage 0: Collect runtime dependencies =====
FROM rust:1-bookworm AS deps

COPY target/release/kyomi /tmp/kyomi

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
COPY --from=deps /tmp/runtime-libs/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
COPY --from=deps /tmp/runtime-libs/ /lib/x86_64-linux-gnu/

# CA certificates (for outbound TLS to Anthropic, Stripe, HuggingFace, etc.)
COPY --from=deps /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# DNS config — glibc needs nsswitch.conf to know how to resolve hostnames
COPY --from=deps /etc/nsswitch.conf /etc/nsswitch.conf

# Binary
COPY --from=deps /tmp/kyomi /app/kyomi

# Writable /tmp
COPY --from=deps --chown=1000:1000 /tmp/scratch-tmp /tmp

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
