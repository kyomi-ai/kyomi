#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-client-eval-test.sh — self-test for check-client-eval.sh
#
# Plants synthetic client files in a tmp directory and runs the linter against
# each. Every fixture has a documented expected exit code; prints PASS/FAIL per
# fixture and exits non-zero if any assertion fails. Also proves the real tree
# is clean (the lint must start, and stay, at zero violations).
#
# Usage:
#   ./scripts/lint/check-client-eval-test.sh
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINTER="$SCRIPT_DIR/check-client-eval.sh"

if [ ! -x "$LINTER" ]; then
    echo "ERROR: linter not executable at $LINTER" >&2
    exit 2
fi

TMP="$(mktemp -d -t check-client-eval-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fails=0
checks=0

# assert_linter <label> <expected exit> <file>
assert_linter() {
    local label="$1" expected="$2" file="$3" output actual
    checks=$((checks + 1))
    set +e
    output="$("$LINTER" "$file" 2>&1)"
    actual=$?
    set -e
    if [ "$actual" -eq "$expected" ]; then
        printf 'PASS  %-46s  (exit %d as expected)\n' "$label" "$actual"
    else
        printf 'FAIL  %-46s  expected exit %d, got %d\n' "$label" "$expected" "$actual"
        printf '%s\n' "$output" | sed 's/^/        /'
        fails=$((fails + 1))
    fi
}

mk() { # mk <name> <content...>
    local name="$1"; shift
    printf '%s\n' "$@" > "$TMP/$name"
    printf '%s' "$TMP/$name"
}

# --- Rust: violations -------------------------------------------------------
assert_linter "rs: Function::new_no_args"            1 "$(mk a.rs 'let p = js_sys::Function::new_no_args("return 1");')"
assert_linter "rs: Function::new_with_args"          1 "$(mk b.rs 'let f = Function::new_with_args("a", "return a");')"
assert_linter "rs: js_sys::eval"                     1 "$(mk c.rs 'let _ = js_sys::eval("1+1");')"
assert_linter "rs: set_timeout_with_str"             1 "$(mk d.rs 'window.set_timeout_with_str_and_timeout_and_arguments_0("x()", 10);')"
assert_linter "rs: set_interval_with_str"            1 "$(mk e.rs 'window.set_interval_with_str("x()");')"

# --- Rust: clean ------------------------------------------------------------
assert_linter "rs: typed fetch is fine"              0 "$(mk f.rs 'let p = window.fetch_with_str_and_init("/api", &opts);')"
assert_linter "rs: callback timer is fine"           0 "$(mk g.rs 'window.set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 10);')"
assert_linter "rs: doc/line comment mention is fine" 0 "$(mk h.rs '/// Never use js_sys::Function::new_no_args here.' '// js_sys::eval is banned' 'let x = 1;')"

# --- Rust: escape hatch -----------------------------------------------------
assert_linter "rs: allow marker above (justified)"   0 "$(mk i.rs '// lint-allow: client-eval=server-only code path never in wasm' 'let p = js_sys::Function::new_no_args("x");')"
assert_linter "rs: allow marker same line"           0 "$(mk j.rs 'let p = js_sys::eval("1"); // lint-allow: client-eval=test-only helper behind cfg')"
assert_linter "rs: allow marker too short -> blocked" 1 "$(mk k.rs '// lint-allow: client-eval=ok' 'let p = js_sys::Function::new_no_args("x");')"
assert_linter "rs: allow marker empty -> blocked"    1 "$(mk l.rs '// lint-allow: client-eval=' 'let p = js_sys::Function::new_no_args("x");')"
assert_linter "rs: marker two lines up -> blocked"   1 "$(mk m.rs '// lint-allow: client-eval=some real reason here' '' 'let p = js_sys::Function::new_no_args("x");')"

# --- JS / HTML --------------------------------------------------------------
assert_linter "js: eval()"                           1 "$(mk a.js 'const r = eval("1+1");')"
assert_linter "js: new Function()"                   1 "$(mk b.js 'const f = new Function("return 1");')"
assert_linter "js: setTimeout with string"           1 "$(mk c.js 'setTimeout("go()", 10);')"
assert_linter "js: setInterval with string"          1 "$(mk d.js "setInterval('go()', 10);")"
assert_linter "html: inline eval()"                  1 "$(mk a.html '<script nonce="x">eval("1")</script>')"
assert_linter "js: setTimeout with callback is fine" 0 "$(mk e.js 'setTimeout(() => go(), 10);' 'setTimeout(go, 10);')"
assert_linter "js: method named evaluate is fine"    0 "$(mk f.js 'chart.evaluate(1); obj.eval_mode = 2; const retrieval = 1;')"
assert_linter "js: allow marker"                     0 "$(mk g.js '// lint-allow: client-eval=vendored polyfill, never loaded under CSP' 'const r = eval("1");')"
assert_linter "js: comment mention is fine"          0 "$(mk h.js '// eval( is banned here' 'const ok = 1;')"
assert_linter "other extension is ignored"           0 "$(mk notes.txt 'js_sys::eval("x")')"

# --- The real tree must be clean -------------------------------------------
checks=$((checks + 1))
set +e
tree_out="$("$LINTER" 2>&1)"
tree_rc=$?
set -e
if [ "$tree_rc" -eq 0 ]; then
    printf 'PASS  %-46s  (exit 0 as expected)\n' "real tree is clean"
else
    printf 'FAIL  %-46s  expected exit 0, got %d\n' "real tree is clean" "$tree_rc"
    printf '%s\n' "$tree_out" | sed 's/^/        /'
    fails=$((fails + 1))
fi

echo
if [ "$fails" -gt 0 ]; then
    echo "$fails of $checks checks FAILED"
    exit 1
fi
echo "all $checks checks passed"
