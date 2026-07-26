#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-disposal-safety.sh — Leptos signal disposal safety
#
# Introduced by PR #34 (commit 9c20d8f0). That commit's message cites
# "KYO-289", an identifier from an earlier issue-numbering scheme that no
# longer resolves — cite the PR, not that ID. Enforcement status of this and
# the four sibling patterns it does NOT cover is documented in
# docs/CODING_STANDARDS.md § "Enforcement status" (KYO-199).
#
# Static lint that blocks two patterns known to cause "reactive value already
# disposed" WASM panics in Leptos:
#
# NOTE: the two rules are NOT equally strict, despite reading symmetrically
# below. Rule A FAILS the build; Rule B only WARNS and exits 0 (see the
# `*:WARN*)` case in the reporting loop near the bottom of this file). Rule B
# cannot distinguish a genuinely mixed-lifetime derive from a same-scope one,
# so gating on it would fail every build — it is advisory by design. Do not
# "fix" that asymmetry without addressing the false-positive rate first.
#
#   Rule A — bare .set() / .update() inside spawn_local or deferred callbacks
#     [BLOCKING — sets exit status 1]
#     spawn_local spawns a detached future that outlives the component. If
#     the user navigates away before it completes, .set() on a disposed
#     signal panics. Use .try_set() / .try_update() instead.
#
#     Deferred contexts also include gloo_timers Timeout/Interval callbacks.
#
#   Rule B — bare .get() inside Signal::derive / Memo::new closures
#     [ADVISORY — prints WARN:B, does NOT affect exit status]
#     A derive that subscribes to Layout-scoped signals (SyncStore) AND
#     reads page-scoped signals via .get() will panic when the page is
#     disposed and a sync update re-evaluates the derive. Use .try_get()
#     instead.
#
# Escape hatch (require non-empty justification, ≥5 chars after `=` trimmed):
#   `// lint-allow: disposal-safe=<why>`  on the same line as the violation
#
# Usage:
#   check-disposal-safety.sh                 run against full tree
#   check-disposal-safety.sh <file>...       run against the listed files only
#
# Exit codes: 0 no Rule A violations (Rule B warnings do not affect this),
#             1 Rule A violations found, 2 usage error.
#
# Pure bash + awk. No Rust toolchain required.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LINT_DIR="${DISPOSAL_LINT_DIR:-$REPO_ROOT/crates/kyomi-ui/src}"

declare -a TARGETS=()
if [ "$#" -gt 0 ]; then
    for f in "$@"; do
        if [ -f "$f" ]; then
            abs="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
        elif [ -f "$REPO_ROOT/$f" ]; then
            abs="$REPO_ROOT/$f"
        else
            continue
        fi
        case "$abs" in
            "$LINT_DIR"/*.rs) TARGETS+=("$abs") ;;
            *) ;;
        esac
    done
else
    while IFS= read -r -d '' f; do
        TARGETS+=("$f")
    done < <(find "$LINT_DIR" -name '*.rs' -type f -print0 | sort -z)
fi

if [ "${#TARGETS[@]}" -eq 0 ]; then
    exit 0
fi

awk_program='
BEGIN {
    depth = 0
    spawn_stack_size = 0
    derive_stack_size = 0
    entering_spawn = 0
    entering_derive = 0
    in_test_module = 0
    test_module_depth = 0
}

function trim(s,  t) {
    t = s
    sub(/^[[:space:]]+/, "", t)
    sub(/[[:space:]]+$/, "", t)
    return t
}

function strip_comment(s,  idx) {
    idx = index(s, "//")
    if (idx > 0) return substr(s, 1, idx - 1)
    return s
}

function has_escape_hatch(line,  idx, tail, eqidx, just) {
    idx = index(line, "// lint-allow: disposal-safe=")
    if (idx == 0) return 0
    tail = substr(line, idx + length("// lint-allow: disposal-safe="))
    just = trim(tail)
    if (length(just) < 5) {
        printf "%s:%d:WARN empty-or-short escape-hatch justification (need ≥5 chars)\n",
            FILENAME, FNR
        return 0
    }
    return 1
}

function update_stacks(code,  i, c) {
    for (i = 1; i <= length(code); i++) {
        c = substr(code, i, 1)
        if (c == "{") {
            depth++

            if (entering_spawn) {
                spawn_stack_size++
                spawn_stack[spawn_stack_size] = depth
                entering_spawn = 0
            }
            if (entering_derive) {
                derive_stack_size++
                derive_stack[derive_stack_size] = depth
                entering_derive = 0
            }
            if (in_test_module == 1 && test_module_depth == 0) {
                test_module_depth = depth
            }
        } else if (c == "}") {
            if (spawn_stack_size > 0 && depth == spawn_stack[spawn_stack_size]) {
                spawn_stack_size--
            }
            if (derive_stack_size > 0 && depth == derive_stack[derive_stack_size]) {
                derive_stack_size--
            }
            if (in_test_module && depth == test_module_depth) {
                in_test_module = 0
                test_module_depth = 0
            }
            depth--
        }
    }
}

BEGINFILE {
    depth = 0
    spawn_stack_size = 0
    derive_stack_size = 0
    entering_spawn = 0
    entering_derive = 0
    in_test_module = 0
    test_module_depth = 0
}

{
    raw = $0
    code = strip_comment(raw)

    # Skip test modules entirely.
    if (code ~ /^[[:space:]]*#\[cfg\(test\)\]/ || code ~ /^[[:space:]]*mod tests/) {
        in_test_module = 1
    }

    # Detect deferred context entry: spawn_local, Timeout, TimeoutFuture, set_timeout
    if (code ~ /spawn_local[[:space:]]*\(/ ||
        code ~ /Timeout::new[[:space:]]*\(/ ||
        code ~ /TimeoutFuture::new[[:space:]]*\(/ ||
        code ~ /set_timeout[[:space:]]*\(/) {
        entering_spawn = 1
    }

    # Detect derive context entry: Signal::derive, Memo::new
    if (code ~ /Signal::derive[[:space:]]*\(/ ||
        code ~ /Memo::new[[:space:]]*\(/) {
        entering_derive = 1
    }

    # Update brace depth and context stacks
    update_stacks(code)

    # Skip lines in test modules
    if (in_test_module) next

    # Skip lines with escape hatch
    if (has_escape_hatch(raw)) next

    # Rule A: bare signal access inside spawn_local / deferred context
    if (spawn_stack_size > 0) {
        # Match .set( but not .try_set( or .set_untracked(
        if (match(code, /\.[[:space:]]*set[[:space:]]*\(/) &&
            code !~ /\.[[:space:]]*try_set[[:space:]]*\(/ &&
            code !~ /\.[[:space:]]*set_untracked[[:space:]]*\(/) {
            printf "%s:%d:A bare .set() inside deferred context — use .try_set() to avoid disposal panic\n",
                FILENAME, FNR
        }
        # Match .update( but not .try_update( or .update_value(
        if (match(code, /\.[[:space:]]*update[[:space:]]*\(/) &&
            code !~ /\.[[:space:]]*try_update[[:space:]]*\(/ &&
            code !~ /\.[[:space:]]*update_value[[:space:]]*\(/ &&
            code !~ /\.[[:space:]]*update_untracked[[:space:]]*\(/) {
            printf "%s:%d:A bare .update() inside deferred context — use .try_update() to avoid disposal panic\n",
                FILENAME, FNR
        }
        # Match .get_untracked( but not .try_get_untracked(
        if (match(code, /\.[[:space:]]*get_untracked[[:space:]]*\(/) &&
            code !~ /\.[[:space:]]*try_get_untracked[[:space:]]*\(/) {
            printf "%s:%d:A bare .get_untracked() inside deferred context — use .try_get_untracked() to avoid disposal panic\n",
                FILENAME, FNR
        }
    }

    # Rule B: bare .get() inside Signal::derive / Memo::new (WARN only)
    # This is a warning because not all derives mix signal lifetimes.
    # The pattern is only dangerous when Layout-scoped signals (SyncStore)
    # coexist with page-scoped signals in the same closure.
    if (derive_stack_size > 0) {
        if (match(code, /\.[[:space:]]*get[[:space:]]*\(/) &&
            code !~ /\.[[:space:]]*try_get[[:space:]]*\(/ &&
            code !~ /\.[[:space:]]*get_untracked[[:space:]]*\(/ &&
            code !~ /\.[[:space:]]*try_get_untracked[[:space:]]*\(/ &&
            code !~ /\.[[:space:]]*get_value[[:space:]]*\(/) {
            printf "%s:%d:WARN:B bare .get() inside Signal::derive/Memo — consider .try_get() if this derive mixes Layout-scoped and page-scoped signals\n",
                FILENAME, FNR
        }
    }
}

ENDFILE {
    if (spawn_stack_size > 0) {
        printf "%s:1:PARSE deferred context did not close before EOF (linter parse error)\n",
            FILENAME
    }
    if (derive_stack_size > 0) {
        printf "%s:1:PARSE derive context did not close before EOF (linter parse error)\n",
            FILENAME
    }
}
'

findings="$(awk "$awk_program" "${TARGETS[@]}" | LC_ALL=C sort -t: -k1,1 -k2,2n -k3,3)"

if [ -z "$findings" ]; then
    exit 0
fi

status=0
while IFS= read -r line; do
    printf '%s\n' "$line" >&2
    case "$line" in
        *:WARN*) ;;
        *:PARSE*) ;;
        *)
            status=1
            ;;
    esac
done <<< "$findings"

exit "$status"
