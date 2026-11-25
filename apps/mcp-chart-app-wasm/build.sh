#!/bin/bash
# SPDX-License-Identifier: AGPL-3.0-or-later
#
# Build the MCP Chart App (pure Rust/WASM) and produce a single chart_app.html.
#
# Steps:
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

echo "==> Step 1: Build WASM via Trunk"
rm -rf dist
trunk build --release --filehash false

echo "==> Step 2: Inline into single HTML file"
python3 inline_chart_app.py dist "$OUTPUT_PATH"

echo "==> Done!"
