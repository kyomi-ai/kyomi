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
#   1. target/release/kyomi                — server binary
#   2. target/release/runtime-libs/libs/   — shared libraries (ldd output)
#   3. target/release/runtime-libs/etc/    — ca-certificates.crt, nsswitch.conf
#   4. target/release/runtime-libs/tmp/    — writable /tmp placeholder
#   5. crates/kyomi-ui/dist/               — Leptos WASM frontend (trunk build --release)
#   6. apps/mcp-chart-app-wasm/chart_app.html — MCP chart viewer
#
# IMPORTANT: The server binary and WASM frontend MUST be compiled on the same
# host so that CARGO_MANIFEST_DIR is identical for both. Leptos server_fn hashes
# include this path — building one on the host and the other inside Docker
# produces mismatched hashes and a completely broken app.
#
# IMPORTANT: The shared libraries MUST come from the same host that compiled the
# binary, so that glibc versions match. Do NOT collect libs inside Docker from a
# different base image.
#
# Everything is embedded in the binary: frontend assets, ML model, constants,
# chartml spec. No runtime data files needed.
#
# Self-hosted quickstart:
#   docker run -v kyomi-data:/data -p 3000:3000 -e ANTHROPIC_API_KEY=sk-... kyomi
# ---------------------------------------------------------------------------

FROM scratch

# Dynamic linker + shared libraries (collected on the build host)
COPY target/release/runtime-libs/libs/ld-linux-x86-64.so.2 /lib64/ld-linux-x86-64.so.2
COPY target/release/runtime-libs/libs/ /lib/x86_64-linux-gnu/

# CA certificates (for outbound TLS to Anthropic, Stripe, HuggingFace, etc.)
COPY target/release/runtime-libs/etc/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt

# DNS config — glibc needs nsswitch.conf to know how to resolve hostnames
COPY target/release/runtime-libs/etc/nsswitch.conf /etc/nsswitch.conf

# Binary
COPY target/release/kyomi /app/kyomi

# Writable /tmp — scratch images have no directories
COPY --chown=1000:1000 target/release/runtime-libs/tmp/ /tmp/

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
