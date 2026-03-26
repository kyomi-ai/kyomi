#!/usr/bin/env bash
# rebuild-leptos.sh — Full Leptos frontend rebuild + server restart
#
# Runs in SaaS mode (Postgres + Redis) so the Leptos server shares the same
# database as the React server — required for E2E regression testing.
#
# Steps:
#   1. trunk build --release (compiles WASM frontend → crates/kyomi-ui/dist/)
#   2. cargo build --profile dev-server -p kyomi-server (release without LTO)
#   3. Kills old server on port 3000
#   4. Starts new server in SaaS mode on port 3000
#
# The dev-server profile inherits from release (opt-level=z, strip, panic=abort)
# but disables LTO and uses 16 codegen units for fast incremental builds.
# Use --release for production-identical builds.
#
# Usage:
#   bash scripts/dev/rebuild-leptos.sh                 # fast dev build (no LTO)
#   bash scripts/dev/rebuild-leptos.sh --release       # full production build
#   bash scripts/dev/rebuild-leptos.sh --skip-trunk    # skip trunk if only Rust changed
#   bash scripts/dev/rebuild-leptos.sh --skip-server   # skip server if only WASM changed

set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

SKIP_TRUNK=false
SKIP_SERVER=false
PROFILE="dev-server"
TARGET_DIR="target/dev-server"
for arg in "$@"; do
  case "$arg" in
    --skip-trunk) SKIP_TRUNK=true ;;
    --skip-server) SKIP_SERVER=true ;;
    --release) PROFILE="release"; TARGET_DIR="target/release" ;;
  esac
done

# Step 1: Build Leptos WASM frontend
if [ "$SKIP_TRUNK" = false ]; then
  echo "==> Step 1/4: trunk build --release"
  cd crates/kyomi-ui
  trunk build --release
  cd ../..
  echo "    Done."
else
  echo "==> Step 1/4: trunk build SKIPPED (--skip-trunk)"
fi

# Step 2: Build server binary
if [ "$SKIP_SERVER" = false ]; then
  echo "==> Step 2/4: cargo build --profile $PROFILE -p kyomi-server"
  cargo build --profile "$PROFILE" -p kyomi-server
  echo "    Done."
else
  echo "==> Step 2/4: cargo build SKIPPED (--skip-server)"
fi

# Step 3: Kill old server
echo "==> Step 3/4: Stopping old server on port 3000"
kill $(lsof -ti:3000) 2>/dev/null || true
sleep 1

# Step 4: Start new server in SaaS mode (Postgres + Redis)
echo "==> Step 4/4: Starting server (SaaS mode, profile=$PROFILE)"
set -a
source .env 2>/dev/null || true
set +a

PORT=3000 FRONTEND_URL=http://localhost:3000 \
  DATABASE_URL="${DATABASE_URL:-postgresql://kyomi:password@localhost:5433/kyomi}" \
  REDIS_URL="${REDIS_URL:-redis://localhost:6380/0}" \
  ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY:-}" \
  ./$TARGET_DIR/kyomi > /tmp/kyomi-leptos.log 2>&1 &

sleep 3
if grep -q "listening" /tmp/kyomi-leptos.log 2>/dev/null; then
  echo "==> Server started on http://localhost:3000 (SaaS mode)"
  grep -E "LLM:|Edition:|Database:" /tmp/kyomi-leptos.log 2>/dev/null || true
else
  echo "==> ERROR: Server failed to start. Check /tmp/kyomi-leptos.log"
  tail -10 /tmp/kyomi-leptos.log
  exit 1
fi
