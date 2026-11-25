#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-or-later
"""Fault tests for inline_chart_app.py (KYO-992).

The two faults this covers are the ones that could make build.sh succeed while
producing a chart_app.html that draws nothing:

1. wasm-bindgen's glue no longer contains the `module_or_path = fetch(...)`
   line the inliner patches, so the WASM is never embedded.
2. The inliner relies on the platform's default text encoding instead of UTF-8,
   which is the ANSI code page on Windows.

Every case runs the real inliner as a subprocess against a throwaway fixture
directory, never the app's own dist/ or chart_app.html.

Run with:
    python3 apps/mcp-chart-app-wasm/inline_chart_app_test.py
"""

import base64
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

INLINER = Path(__file__).with_name("inline_chart_app.py")
WASM_BYTES = b"\x00asm\x01\x00\x00\x00kyo-992-fixture"
FETCH_LINE = "    module_or_path = fetch(module_or_path);\n"
JS_WITH_FETCH = (
    "export function __wbg_init(module_or_path) {\n"
    + FETCH_LINE
    + "    return module_or_path;\n"
    "}\n"
)
JS_WITHOUT_FETCH = (
    "export function __wbg_init(module_or_path) {\n"
    "    // wasm-bindgen changed the shape of this function\n"
    "    return module_or_path;\n"
    "}\n"
)
NON_ASCII = "café — 日本"


def make_dist(root: Path, js: str, css: str = "body { color: #111; }\n") -> Path:
    dist = root / "dist"
    dist.mkdir()
    (dist / "mcp-chart-app-wasm_bg.wasm").write_bytes(WASM_BYTES)
    (dist / "mcp-chart-app-wasm.js").write_text(js, encoding="utf-8")
    (dist / "mcp-chart-app-wasm.css").write_text(css, encoding="utf-8")
    return dist


def run_inliner(dist: Path, output: Path) -> subprocess.CompletedProcess:
    """Run the inliner under a non-UTF-8 default encoding.

    Windows uses its ANSI code page for text I/O unless the code names UTF-8,
    which is exactly the platform difference fault 2 is about. PYTHONUTF8=0
    with a C locale reproduces that on Linux and macOS too.
    """
    env = dict(os.environ)
    env.update(
        {
            "PYTHONUTF8": "0",
            "PYTHONCOERCECLOCALE": "0",
            "LC_ALL": "C",
            "LANG": "C",
        }
    )
    return subprocess.run(
        [sys.executable, str(INLINER), str(dist), str(output)],
        capture_output=True,
        text=True,
        env=env,
    )


class InlinerFaultTests(unittest.TestCase):
    def test_missing_fetch_line_fails_with_clear_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "chart_app.html"
            result = run_inliner(make_dist(root, JS_WITHOUT_FETCH), output)

            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("expected exactly one", result.stderr)
            self.assertIn("found 0", result.stderr)
            self.assertFalse(
                output.exists(),
                "a failed inline must not leave a chart_app.html behind",
            )

    def test_duplicate_fetch_line_fails_with_clear_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "chart_app.html"
            js = (
                "export function __wbg_init(module_or_path) {\n"
                + FETCH_LINE
                + FETCH_LINE
                + "    return module_or_path;\n"
                "}\n"
            )
            result = run_inliner(make_dist(root, js), output)

            self.assertNotEqual(result.returncode, 0, result.stdout)
            self.assertIn("expected exactly one", result.stderr)
            self.assertIn("found 2", result.stderr)
            self.assertFalse(output.exists())

    def test_non_ascii_input_is_written_as_utf8(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "chart_app.html"
            dist = make_dist(
                root,
                JS_WITH_FETCH.replace("export", f'const label = "{NON_ASCII}";\nexport'),
                f'body::before {{ content: "{NON_ASCII}"; }}\n',
            )
            result = run_inliner(dist, output)

            self.assertEqual(result.returncode, 0, result.stderr)
            html_bytes = output.read_bytes()
            self.assertFalse(html_bytes.startswith(b"\xef\xbb\xbf"), "unexpected BOM")
            self.assertNotIn(b"\r\n", html_bytes, "output must not depend on the OS")
            html = html_bytes.decode("utf-8")
            self.assertIn(NON_ASCII, html)

    def test_successful_inline_is_self_contained(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            output = root / "chart_app.html"
            result = run_inliner(make_dist(root, JS_WITH_FETCH), output)

            self.assertEqual(result.returncode, 0, result.stderr)
            html = output.read_text(encoding="utf-8")
            expected_b64 = base64.b64encode(WASM_BYTES).decode("ascii")
            self.assertIn(expected_b64, html, "WASM must be embedded")
            self.assertNotIn("fetch(module_or_path)", html)
            self.assertIn("await __wbg_init();", html)
            self.assertNotIn("<script src=", html)
            self.assertNotIn("<link rel=", html)


if __name__ == "__main__":
    unittest.main()
