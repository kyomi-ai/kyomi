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

# Tooling versions needed to build the Leptos WASM frontend. Pinned so the
# image is reproducible — bump these intentionally when upgrading leptos.
#   wasm-bindgen-cli must match the wasm-bindgen library version in Cargo.lock.
ENV TRUNK_VERSION=0.21.14 \
    WASM_BINDGEN_VERSION=0.2.114 \
    TAILWIND_VERSION=v4.2.2

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates curl clang brotli gzip \
    && rm -rf /var/lib/apt/lists/*

# wasm target for the Leptos hydrate build.
RUN rustup target add wasm32-unknown-unknown

# Trunk — bundles the kyomi-ui crate into dist/ (runs the tailwindcss pre_build
# hook, compiles to wasm32, runs wasm-bindgen, fingerprints assets).
RUN curl -fsSL "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-x86_64-unknown-linux-gnu.tar.gz" \
    | tar -xz -C /usr/local/bin trunk

# wasm-bindgen-cli — must match Cargo.lock's wasm-bindgen library version.
RUN curl -fsSL "https://github.com/rustwasm/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
    | tar -xz --strip-components=1 -C /usr/local/bin \
        "wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen" \
        "wasm-bindgen-${WASM_BINDGEN_VERSION}-x86_64-unknown-linux-musl/wasm-bindgen-test-runner"

# Standalone tailwindcss binary — referenced by Trunk.toml's pre_build hook.
RUN curl -fsSL -o /usr/local/bin/tailwindcss \
        "https://github.com/tailwindlabs/tailwindcss/releases/download/${TAILWIND_VERSION}/tailwindcss-linux-x64" \
    && chmod +x /usr/local/bin/tailwindcss

# wasm-opt — pre-seeded at trunk's expected cache path. Trunk 0.21 invokes
# wasm-opt without `--enable-bulk-memory-opt` / `--enable-nontrapping-float-to-int`,
# and rejects input WASM that uses those instructions (which rustc + leptos
# routinely emit). Install binaryen's wasm-opt and wrap it so those flags
# are always passed through.
ENV BINARYEN_VERSION=version_123
RUN curl -fsSL "https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN_VERSION}/binaryen-${BINARYEN_VERSION}-x86_64-linux.tar.gz" \
        | tar -xz -C /tmp \
    && mkdir -p "/root/.cache/trunk/wasm-opt-${BINARYEN_VERSION}/bin" \
    && mv "/tmp/binaryen-${BINARYEN_VERSION}/bin/wasm-opt" \
          "/root/.cache/trunk/wasm-opt-${BINARYEN_VERSION}/bin/wasm-opt-real" \
    && printf '#!/bin/sh\nexec "%s" --enable-bulk-memory-opt --enable-nontrapping-float-to-int "$@"\n' \
        "/root/.cache/trunk/wasm-opt-${BINARYEN_VERSION}/bin/wasm-opt-real" \
        > "/root/.cache/trunk/wasm-opt-${BINARYEN_VERSION}/bin/wasm-opt" \
    && chmod +x "/root/.cache/trunk/wasm-opt-${BINARYEN_VERSION}/bin/wasm-opt" \
    && rm -rf "/tmp/binaryen-${BINARYEN_VERSION}"

WORKDIR /build

# Copy Cargo workspace and crates
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
COPY apps/server/ ./apps/server/
COPY enterprise/ ./enterprise/

# Remove desktop from workspace members (not needed for server Docker build)
RUN sed -i '/"apps\/desktop",/d' Cargo.toml

# Strip the local-dev [patch.crates-io] block — prod uses crates.io versions
# of chartml-* and kode-leptos. The block is delimited by marker comments in
# the workspace Cargo.toml.
RUN sed -i '/# BEGIN_LOCAL_DEV_PATCHES/,/# END_LOCAL_DEV_PATCHES/d' Cargo.toml

# Copy files needed for include_str!/include_bytes! paths
COPY data/ ./data/
COPY apps/mcp-chart-app/chart_app.html ./apps/mcp-chart-app/chart_app.html
COPY assets/kyomi_email_logo.png ./assets/kyomi_email_logo.png

# All SQL queries use runtime string-based sqlx::query() — no compile-time
# checking, no .sqlx/ cache, and no DATABASE_URL needed at build time.

# Build the Leptos WASM frontend first. rust-embed in apps/server/src/
# leptos_frontend.rs embeds crates/kyomi-ui/dist/ into the server binary
# at compile time, so dist/ must be populated before the cargo build below.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cd crates/kyomi-ui && trunk build --release

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
