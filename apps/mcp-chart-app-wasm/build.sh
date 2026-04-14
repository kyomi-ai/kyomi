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
# Output: ../mcp-chart-app/chart_app.html (consumed by include_str! in mcp.rs)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

OUTPUT_PATH="../mcp-chart-app/chart_app.html"

echo "==> Step 1: Build WASM via Trunk"
rm -rf dist
trunk build --release --filehash false

echo "==> Step 2: Inline into single HTML file"
python3 - "$OUTPUT_PATH" << 'PYTHON_SCRIPT'
import base64, sys, re
from pathlib import Path

dist = Path("dist")
output_path = sys.argv[1]

# Read the built index.html
html = (dist / "index.html").read_text()

# Read and base64-encode the WASM binary
wasm_file = next(dist.glob("*.wasm"))
wasm_b64 = base64.b64encode(wasm_file.read_bytes()).decode()

# Read the JS glue file
js_file = next(dist.glob("mcp-chart-app-wasm.js"))
js_content = js_file.read_text()

# Read the CSS file(s)
css_parts = []
for css_file in dist.glob("*.css"):
    css_parts.append(css_file.read_text())
css_content = "\n".join(css_parts)

# Patch the JS glue to load WASM from inline base64 instead of fetch()
# wasm-bindgen's init function does:
#   module_or_path = fetch(module_or_path);
# We replace it to decode inline base64.
js_patched = re.sub(
    r"module_or_path\s*=\s*fetch\s*\(\s*module_or_path\s*\)\s*;",
    f"""module_or_path = (async () => {{
        const b64 = "{wasm_b64}";
        const binary = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
        return new Response(binary, {{ headers: {{ "content-type": "application/wasm" }} }});
    }})();""",
    js_content,
)

# Build the final single-file HTML
final_html = f"""<!DOCTYPE html>
<html>
<head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <meta name="color-scheme" content="light dark">
    <style>{css_content}</style>
</head>
<body>
    <div id="chart">
        <div style="display: flex; align-items: center; justify-content: center; height: 200px;">
            <svg width="32" height="32" viewBox="0 0 60 60" xmlns="http://www.w3.org/2000/svg"><defs><linearGradient id="ag" x1="0%" y1="0%" x2="100%" y2="100%"><stop offset="0%" style="stop-color:#d97706"/><stop offset="100%" style="stop-color:#b45309"/></linearGradient><style>.r{{fill:url(#ag);animation:s 2s ease-in-out infinite}}.r1{{animation-delay:.6s}}.r2{{animation-delay:1.3s}}.r3{{animation-delay:.2s}}.r4{{animation-delay:1.7s}}.r5{{animation-delay:.9s}}.r6{{animation-delay:.1s}}.r7{{animation-delay:1.4s}}.r8{{animation-delay:.4s}}.c{{fill:url(#ag);animation:p 2s ease-in-out infinite}}@keyframes s{{0%,100%{{opacity:.4;transform:scale(1)}}50%{{opacity:1;transform:scale(1.1)}}}}@keyframes p{{0%,100%{{opacity:.6;transform:scale(1)}}50%{{opacity:1;transform:scale(1.15)}}}}</style></defs><g transform="translate(30,30)"><polygon class="r r1" points="0,-28 4.5,-11 0,-6 -4.5,-11"/><polygon class="r r2" points="20,-20 11.5,-4 7,-7 11.5,-11.5"/><polygon class="r r3" points="28,0 11.5,4.5 7,0 11.5,-4.5"/><polygon class="r r4" points="20,20 4.5,11.5 0,7 4.5,11.5"/><polygon class="r r5" points="0,28 -4.5,11.5 0,7 4.5,11.5"/><polygon class="r r6" points="-20,20 -11.5,4.5 -7,7 -11.5,11.5"/><polygon class="r r7" points="-28,0 -11.5,-4.5 -7,0 -11.5,4.5"/><polygon class="r r8" points="-20,-20 -4.5,-11.5 0,-7 -4.5,-11.5"/><circle class="c" cx="0" cy="0" r="6"/></g></svg>
        </div>
    </div>
    <script type="module">{js_patched}</script>
</body>
</html>"""

Path(output_path).write_text(final_html)
wasm_size_kb = wasm_file.stat().st_size / 1024
b64_size_kb = len(wasm_b64) / 1024
html_size_kb = len(final_html) / 1024
print(f"==> WASM: {wasm_size_kb:.0f} KB, base64: {b64_size_kb:.0f} KB, final HTML: {html_size_kb:.0f} KB")
print(f"==> Output: {output_path}")
PYTHON_SCRIPT

echo "==> Done!"
