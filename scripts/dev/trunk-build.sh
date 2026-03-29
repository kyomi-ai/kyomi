#!/bin/bash
# Build trunk to a staging directory, then swap atomically.
# Prevents the live server from serving half-written WASM during builds.

set -e

KYOMI_UI_DIR="$(cd "$(dirname "$0")/../../crates/kyomi-ui" && pwd)"
DIST_DIR="$KYOMI_UI_DIR/dist"
STAGING_DIR="$KYOMI_UI_DIR/dist-staging"

cd "$KYOMI_UI_DIR"

# Build to staging directory
echo "Building to staging directory..."
trunk build --dist "$STAGING_DIR" "$@"

# Atomic swap: rename current dist, move staging in, remove old
echo "Swapping dist..."
if [ -d "$DIST_DIR" ]; then
    mv "$DIST_DIR" "${DIST_DIR}-old"
fi
mv "$STAGING_DIR" "$DIST_DIR"
rm -rf "${DIST_DIR}-old"

echo "Done."
