#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-client-eval.sh — no eval-style JavaScript in client code
#
# The frontend Content-Security-Policy deliberately omits 'unsafe-eval'
# (KYO-849; `script-src 'self' 'nonce-…' 'wasm-unsafe-eval' …`). Any code that
# makes the browser compile a JavaScript *string* — `new Function("…")`,
# `eval("…")`, `setTimeout("…")` — throws an uncatchable `EvalError` there and
# takes the whole WASM app down with it.
#
# That is exactly how `auth_refresh::try_refresh` broke: it built its `fetch`
# call with `js_sys::Function::new_no_args("return fetch(…)")`. It only ran on
# the rare "server could not renew the session" path, so the policy change
# that broke it shipped unnoticed (v2.7.1) and users hit a dead page instead of
# the login redirect. This lint is the missing "client code still works under
# the policy" check.
#
# Blocked in client code (Rust in crates/kyomi-ui*, handwritten JS/HTML there):
#   Rust:    js_sys::Function::new_no_args / new_with_args, js_sys::eval,
#            web_sys Window::set_timeout_with_str* / set_interval_with_str*
#   JS/HTML: eval(…), new Function(…), setTimeout("…") / setInterval("…")
#
# Use the typed APIs instead (web_sys::Window::fetch_with_str_and_init,
# Closure-based timers, js_sys::Reflect, …).
#
# Escape hatch (justification >= 5 chars), on the same line or the line above:
#   // lint-allow: client-eval=<why this can never run under the CSP>
#   (JS/HTML: the same marker inside a // or <!-- --> comment)
#
# Usage:
#   check-client-eval.sh                 run against the full tracked tree
#   check-client-eval.sh <file>...       run against the listed files only
#
# Exit codes: 0 clean, 1 violations found.
#
# Pure bash + POSIX awk (works with mawk and gawk). No Rust toolchain needed.
# ------------------------------------------------------------------------------

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

collect_full_tree() {
    git -C "$ROOT" ls-files \
        'crates/kyomi-ui/src/*.rs' 'crates/kyomi-ui-components/src/*.rs' \
        'crates/kyomi-ui/*.html' 'crates/kyomi-ui/*.js' \
        'crates/kyomi-ui/assets/*' 'crates/kyomi-ui/static/*' 2>/dev/null \
        | grep -E '\.(rs|js|html)$' \
        | grep -vE '(^|/)(dist|node_modules|target)/' \
        | sed "s#^#$ROOT/#" || true
}

FILES=()
if [ "$#" -gt 0 ]; then
    FILES=("$@")
else
    while IFS= read -r f; do FILES+=("$f"); done < <(collect_full_tree)
fi

violations=0

scan_file() {
    local file="$1" kind="$2"
    [ -f "$file" ] || return 0
    awk -v kind="$kind" -v file="$file" -v sq="'" '
        # True when s carries `lint-allow: client-eval=<>=5 chars>`.
        function allowed(s,   rest) {
            if (match(s, /lint-allow: *client-eval=/)) {
                rest = substr(s, RSTART + RLENGTH)
                sub(/[ \t]*-->.*$/, "", rest)
                gsub(/^[ \t]+|[ \t]+$/, "", rest)
                return length(rest) >= 5
            }
            return 0
        }
        {
            line = $0
            # Whole-line comments (incl. doc comments) may mention the APIs.
            if (line ~ /^[ \t]*(\/\/|\/\*|\*|<!--)/) { prev = line; next }

            hit = ""
            if (kind == "rs") {
                if (line ~ /Function::new_no_args|Function::new_with_args/) hit = "js_sys::Function constructor (compiles a JS string)"
                else if (line ~ /js_sys::eval/) hit = "js_sys::eval"
                else if (line ~ /set_timeout_with_str|set_interval_with_str/) hit = "string-argument timer (eval-like)"
            } else {
                if (line ~ /(^|[^A-Za-z0-9_$.])eval[ \t]*\(/) hit = "eval()"
                else if (line ~ /new[ \t]+Function[ \t]*\(/) hit = "new Function()"
                else if (match(line, /set(Timeout|Interval)[ \t]*\([ \t]*./)) {
                    c = substr(line, RSTART + RLENGTH - 1, 1)
                    if (c == "\"" || c == sq || c == "`") hit = "string-argument timer (eval-like)"
                }
            }

            if (hit != "" && !(allowed(line) || allowed(prev))) {
                printf "ERROR: %s:%d: %s -- blocked by the frontend CSP (no unsafe-eval, KYO-849); use a typed API or add `// lint-allow: client-eval=<why>`\n", file, NR, hit > "/dev/stderr"
                bad = 1
            }
            prev = line
        }
        END { exit bad ? 1 : 0 }
    ' "$file" || violations=$((violations + 1))
}

for f in "${FILES[@]}"; do
    case "$f" in
        *.rs) scan_file "$f" rs ;;
        *.js|*.html) scan_file "$f" js ;;
        *) ;;
    esac
done

if [ "$violations" -gt 0 ]; then
    echo "client-eval lint: $violations file(s) with eval-style JavaScript. See the header of scripts/lint/check-client-eval.sh." >&2
    exit 1
fi
echo "client-eval lint clean (${#FILES[@]} file(s) scanned)."
