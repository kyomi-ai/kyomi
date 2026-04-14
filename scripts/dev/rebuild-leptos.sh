#!/usr/bin/env bash
# rebuild-leptos.sh — Build and serve the Leptos frontend
#
# DEFAULT: Debug builds (fast compile, slow runtime). This is what you use
# for development. Do NOT change this default without explicit approval.
#
# Usage:
#   bash scripts/dev/rebuild-leptos.sh                 # debug build (DEFAULT)
#   bash scripts/dev/rebuild-leptos.sh --release       # production build (slow, rare)
#   bash scripts/dev/rebuild-leptos.sh --skip-trunk    # skip WASM if only server Rust changed
#   bash scripts/dev/rebuild-leptos.sh --skip-server   # skip server if only frontend changed

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SKIP_TRUNK=false
SKIP_SERVER=false
RELEASE=false
for arg in "$@"; do
  case "$arg" in
    --skip-trunk) SKIP_TRUNK=true ;;
    --skip-server) SKIP_SERVER=true ;;
    --release) RELEASE=true ;;
  esac
done

# Step 1: Build WASM frontend
if [ "$SKIP_TRUNK" = false ]; then
  cd crates/kyomi-ui
  if [ "$RELEASE" = true ]; then
    echo "==> Step 1/4: trunk build --release (with build-std, slow)"
    CARGO_UNSTABLE_BUILD_STD="std,panic_abort,core,alloc" \
    RUSTFLAGS="--cfg=has_std -Zunstable-options -Cpanic=immediate-abort" \
    RUSTUP_TOOLCHAIN=nightly trunk build --release
  else
    echo "==> Step 1/4: trunk build (debug)"
    trunk build
  fi
  cd ../..
  echo "    Done."
else
  echo "==> Step 1/4: trunk build SKIPPED (--skip-trunk)"
fi

# Step 2: Build server binary
if [ "$SKIP_SERVER" = false ]; then
  if [ "$RELEASE" = true ]; then
    echo "==> Step 2/4: cargo build --profile dev-server -p kyomi-server"
    cargo build --profile dev-server -p kyomi-server
  else
    echo "==> Step 2/4: cargo build -p kyomi-server (debug)"
    cargo build -p kyomi-server
  fi
  echo "    Done."
else
  echo "==> Step 2/4: cargo build SKIPPED (--skip-server)"
fi

# Determine binary path
if [ "$RELEASE" = true ]; then
  BINARY="target/dev-server/kyomi"
else
  BINARY="target/debug/kyomi"
fi

# Step 3: Kill old server
echo "==> Step 3/4: Stopping old server on port 3000"
kill $(lsof -ti:3000) 2>/dev/null || true
sleep 1

# Step 4: Start new server in SaaS mode
echo "==> Step 4/4: Starting server (SaaS mode)"
set -a
source .env 2>/dev/null || true
set +a

PORT=3000 FRONTEND_URL=https://dev.kyomi.ai \
  DATABASE_URL="${DATABASE_URL:-postgresql://kyomi:password@localhost:5433/kyomi}" \
  REDIS_URL="${REDIS_URL:-redis://localhost:6380/0}" \
  ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}" \
  ./$BINARY > /tmp/kyomi-leptos.log 2>&1 &

sleep 3
if grep -q "listening" /tmp/kyomi-leptos.log 2>/dev/null; then
  echo "==> Server started on https://dev.kyomi.ai (SaaS mode)"
  grep -E "LLM:|Edition:|Database:" /tmp/kyomi-leptos.log 2>/dev/null || true
else
  echo "==> ERROR: Server failed to start. Check /tmp/kyomi-leptos.log"
  tail -10 /tmp/kyomi-leptos.log
  exit 1
fi
