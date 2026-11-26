#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build the MCP Chart App (pure Rust/WASM) and produce a single chart_app.html.
#
# Steps:
# 0. Verify Cargo.lock is current
# 1. Build WASM via Trunk
# 2. Inline WASM + JS + CSS into a single HTML file
#
# No Node.js, npm, or esbuild required.
#
# Output: chart_app.html (consumed by include_str! in mcp.rs)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

OUTPUT_PATH="chart_app.html"

# Fail before building anything if Cargo.lock would need to change, so the app
# shipped inside Kyomi comes from the reviewed lock and never from a fresh
# resolution of the manifest. Keeping the check here rather than in a workflow
# means every caller inherits it: this script is the chart app's only build
# entry point, so there is one copy rather than one per caller.
#
# Trunk's own `--locked` cannot do this. Trunk 0.21.14 first resolves the
# project with `cargo metadata` (src/config/manifest.rs → MetadataCommand)
# without forwarding the flag; that call rewrites a stale lock, and the
# `cargo build --locked` Trunk runs afterwards then finds the lock it just
# wrote, so the build succeeds and the fetched versions are never reviewed.
# Verified against this pinned Trunk: `trunk build --locked` exits 0 and
# rewrites Cargo.lock, while `cargo metadata --locked` fails with Cargo's lock
# error. Run the same check Cargo does, before Trunk can touch the lock.
#
# A real dependency version change updates and commits the lock in the same
# change; the check reports a stale lock rather than rewriting it behind an
# unreviewed dependency. A local `[patch.crates-io]` override also changes
# resolution, so this check fails under one until the lock is regenerated; that
# lock change is temporary and is discarded before committing (see CLAUDE.md,
# "External Crate Dependencies").
echo "==> Step 0: Verify Cargo.lock is current"
cargo metadata --locked --format-version 1 > /dev/null

echo "==> Step 1: Build WASM via Trunk"
rm -rf dist
trunk build --release --filehash false

echo "==> Step 2: Inline into single HTML file"
python3 inline_chart_app.py dist "$OUTPUT_PATH"

echo "==> Done!"
