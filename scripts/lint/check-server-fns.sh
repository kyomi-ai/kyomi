#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-server-fns.sh — Phase 3 enforcement (KYO-122)
#
# Static lint that blocks new divergence between Leptos `#[server]` functions
# and their REST handler counterparts. Runs in the pre-commit hook (staged
# `server_fns/*.rs` only) and in CI (full tree).
#
# Two rules:
#
#   Rule A — non-allowlisted context lookup
#     Any `use_context::<T>()` or `expect_context::<T>()` inside a server_fn
#     body where `T`'s base name is not in the allowlist. Rationale: the
#     KYO-115 failure mode was a server_fn looking up
#     `Arc<ConnectTokenService>` via a context that was never provided — a
#     REST route gets the same dependency through `AppState`. Non-allowlisted
#     lookups are almost always DI that should live in a shared service
#     both sides call.
#
#   Rule B — too many service-layer callouts
#     Count function-call invocations inside a single `#[server]` fn body to:
#       * `sqlx::query(`, `sqlx::query_as(`
#       * `kyomi_auth::`  (any path under)
#       * `kyomi_knowledge::` (any path under)
#       * `services::`     (any path under)
#       * `kyomi_core::db_execute!`, `db_fetch_one!`, `db_fetch_optional!`,
#         `db_fetch_all!`
#     If > SERVER_FN_CALLOUT_MAX (default 3), fail with file:line:fn. Rationale:
#     heavy-orchestration server_fns are the ones that drift from REST.
#
# Escape hatches (require non-empty justification, ≥5 chars after `=` trimmed):
#   `// lint-allow: server-fn-context=<why>`   on line before a `use_context`/
#                                              `expect_context` call
#   `// lint-allow: server-fn-callouts=<why>`  anywhere inside the fn body,
#                                              or on the `#[server]` attribute
#                                              line / fn signature line
#
# Usage:
#   check-server-fns.sh                 run against full tree
#   check-server-fns.sh <file>...       run against the listed files only
#   SERVER_FN_CALLOUT_MAX=N ...         override Rule B threshold
#
# Exit codes: 0 clean, 1 violations found, 2 usage error.
#
# Pure bash + awk. No Rust toolchain required.
# ------------------------------------------------------------------------------

set -euo pipefail

# ------------------------------------------------------------------------------
# Allowlist of permitted types for Rule A (use_context / expect_context).
#
# These are the types the Leptos+Axum runtime and the Kyomi server consistently
# provide. Anything else is suspect — see rationale block above.
#
# Extend by adding a line below. One name per line, base type only (strip
# module paths and generic wrappers: `Arc<ConnectTokenService>` → match on
# `ConnectTokenService`; we do NOT allow `ConnectTokenService` here).
# ------------------------------------------------------------------------------
ALLOWLIST_TYPES=(
    ServerContext
    AuthUser
    # Provided unconditionally by leptos_axum on every server_fn invocation.
    # The server_fn macro relies on this for HTTP status / header control.
    ResponseOptions
)

CALLOUT_MAX="${SERVER_FN_CALLOUT_MAX:-3}"

# ------------------------------------------------------------------------------
# Locate target files.
#
# The linter is normally anchored at `crates/kyomi-ui/src/server_fns/` (the
# only directory where `#[server]` functions live). For the self-test at
# `scripts/lint/check-server-fns-test.sh`, override via `SERVER_FN_LINT_DIR`
# — that env var points at an alternate directory of fixture files.
# ------------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SERVER_FN_DIR="${SERVER_FN_LINT_DIR:-$REPO_ROOT/crates/kyomi-ui/src/server_fns}"

declare -a TARGETS=()
if [ "$#" -gt 0 ]; then
    # Caller-supplied files. Filter to only those that exist and live under
    # `crates/kyomi-ui/src/server_fns/` — the pre-commit hook passes every
    # staged `.rs` file but the linter only checks server_fns.
    for f in "$@"; do
        # Normalize to an absolute path so filename comparisons are stable.
        if [ -f "$f" ]; then
            abs="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
        elif [ -f "$REPO_ROOT/$f" ]; then
            abs="$REPO_ROOT/$f"
        else
            # File doesn't exist (e.g. deleted by the commit). Skip silently —
            # a deleted file can't introduce new violations.
            continue
        fi
        case "$abs" in
            "$SERVER_FN_DIR"/*.rs) TARGETS+=("$abs") ;;
            *) ;;
        esac
    done
else
    # Full-tree mode (used by CI). Enumerate every `.rs` under server_fns.
    while IFS= read -r -d '' f; do
        TARGETS+=("$f")
    done < <(find "$SERVER_FN_DIR" -name '*.rs' -type f -print0 | sort -z)
fi

if [ "${#TARGETS[@]}" -eq 0 ]; then
    # Nothing to check — clean exit.
    exit 0
fi

# ------------------------------------------------------------------------------
# Pass each target through a single awk program that tokenizes the file,
# tracks server_fn boundaries, and emits `path:line:rule:message` findings
# on stderr (via awk's print to /dev/stderr? no — we collect to stdout of
# the awk child, then the shell sorts and re-emits to stderr so ordering
# is deterministic across all targets).
# ------------------------------------------------------------------------------
awk_program='
BEGIN {
    # Build allowlist set from $ALLOW_CSV (comma-separated).
    n = split(ALLOW_CSV, parts, ",")
    for (i = 1; i <= n; i++) allow[parts[i]] = 1
    max_callouts = MAX + 0
}

function reset_fn_state() {
    in_server_attr = 0
    in_fn_sig = 0
    in_fn_body = 0
    fn_depth = 0
    fn_start_line = 0
    fn_name = ""
    fn_callouts = 0
    fn_callouts_allowed = 0
    # Reset per-line allow marker — it is a single-shot flag set by a
    # preceding comment and consumed by the next use_context/expect_context
    # line.
    next_line_context_allowed = 0
}

# Trim leading/trailing whitespace
function trim(s,  t) {
    t = s
    sub(/^[[:space:]]+/, "", t)
    sub(/[[:space:]]+$/, "", t)
    return t
}

# Strip // line comments from a Rust line so matches in comment text do not
# count. We preserve the original $0 for escape-hatch detection.
function strip_comment(s,  idx) {
    idx = index(s, "//")
    if (idx > 0) return substr(s, 1, idx - 1)
    return s
}

# Count `{` and `}` in a code slice and update fn_depth. Returns 1 if the
# balance carried the fn body closed (depth returned to zero after having
# entered the body).
function update_depth(code,  i, c, closed) {
    closed = 0
    for (i = 1; i <= length(code); i++) {
        c = substr(code, i, 1)
        if (c == "{") {
            fn_depth++
            if (!in_fn_body && in_fn_sig) {
                in_fn_body = 1
                in_fn_sig = 0
            }
        } else if (c == "}") {
            fn_depth--
            if (in_fn_body && fn_depth <= 0) {
                closed = 1
            }
        }
    }
    return closed
}

# Extract base type name from a use_context::<...>() / expect_context::<...>()
# invocation. Strips module paths and single-arg wrappers iteratively so
# `std::sync::Arc<kyomi_auth::ConnectTokenService>` reduces to
# `ConnectTokenService`. Returns empty string if extraction fails.
#
# Wrapper types peeled: Arc, Rc, Box, Option, Vec, Pin, RefCell, Cell,
# Mutex, RwLock. Each may appear with or without a module prefix.
#
# NOTE: awks `match()` sets global RSTART/RLENGTH, so we must carefully
# save positions from the outer match before calling any inner match.
# Every mutable variable is declared in the local-var section of the
# function signature.
function base_type(generic,  t, wrappers, stripped,
                   outer_rstart, outer_rlength, head, last,
                   rest, depth, end, i, ch) {
    t = trim(generic)
    wrappers = "Arc|Rc|Box|Option|Vec|Pin|RefCell|Cell|Mutex|RwLock"

    # Loop: if t begins with `SomeWrapper<...>`, peel it; else stop.
    stripped = 1
    while (stripped) {
        stripped = 0
        # Strip leading module path segments on the outer type BEFORE
        # wrapper detection: `std::sync::Arc<...>` → `Arc<...>`.
        if (match(t, /^[A-Za-z_][A-Za-z0-9_]*(::[A-Za-z_][A-Za-z0-9_]*)*[[:space:]]*</)) {
            outer_rstart = RSTART
            outer_rlength = RLENGTH
            head = substr(t, outer_rstart, outer_rlength)
            # Drop trailing `<` and any whitespace before it.
            sub(/[[:space:]]*<[[:space:]]*$/, "", head)
            # head is now "std::sync::Arc" or "Arc" etc.
            # Extract last path segment — this call clobbers RSTART/RLENGTH,
            # but we no longer need the outer matchs positions so thats fine.
            last = head
            if (match(last, /[^:]+$/)) last = substr(last, RSTART, RLENGTH)
            # Check if the last segment is a known wrapper. If so, peel.
            if (match(last, "^(" wrappers ")$")) {
                # Find the matching `>` using the SAVED outer positions.
                rest = substr(t, outer_rstart + outer_rlength)
                depth = 1
                end = 0
                for (i = 1; i <= length(rest); i++) {
                    ch = substr(rest, i, 1)
                    if (ch == "<") depth++
                    else if (ch == ">") {
                        depth--
                        if (depth == 0) { end = i; break }
                    }
                }
                if (end > 0) {
                    t = trim(substr(rest, 1, end - 1))
                    stripped = 1
                    continue
                }
            }
        }
    }

    # Remaining t is the base type with an optional module path. Strip
    # trailing `<...>` (e.g. HashMap<K, V> → HashMap) and leading module
    # path (e.g. leptos_axum::ResponseOptions → ResponseOptions).
    sub(/[[:space:]]*<.*$/, "", t)
    if (match(t, /[^:]+$/)) t = substr(t, RSTART, RLENGTH)
    return trim(t)
}

# Count function-call invocations against Rule B patterns on a single
# code line (line already stripped of // comments). Multiple hits per line
# count each time.
function count_callouts(code,  cnt, tmp, re) {
    cnt = 0

    # kyomi_auth::, kyomi_knowledge::, services::  —  any qualified
    # path followed by `(` (with optional whitespace).
    tmp = code
    re = "(kyomi_auth|kyomi_knowledge|services)::[A-Za-z_][A-Za-z0-9_:]*[[:space:]]*\\("
    while (match(tmp, re)) {
        cnt++
        tmp = substr(tmp, RSTART + RLENGTH)
    }

    # sqlx::query( and sqlx::query_as(
    tmp = code
    re = "sqlx::query(_as)?[[:space:]]*\\("
    while (match(tmp, re)) {
        cnt++
        tmp = substr(tmp, RSTART + RLENGTH)
    }

    # kyomi_core::db_execute!/db_fetch_*! macro invocations.
    tmp = code
    re = "kyomi_core::db_(execute|fetch_one|fetch_optional|fetch_all)![[:space:]]*[({[]"
    while (match(tmp, re)) {
        cnt++
        tmp = substr(tmp, RSTART + RLENGTH)
    }

    return cnt
}

# Validate an escape-hatch justification. Returns 1 if ≥5 non-whitespace
# chars after `=`. Emits a warning finding if the hatch is present but
# empty.
function justified(raw,  j) {
    # raw is the text AFTER the `=`.
    j = trim(raw)
    if (length(j) < 5) {
        printf "%s:%d:WARN empty-or-short escape-hatch justification (need ≥5 chars)\n",
            FILENAME, FNR
        return 0
    }
    return 1
}

# Look for lint-allow comments on this line. Sets flags according to kind.
# kind: "server-fn-context" or "server-fn-callouts"
function check_hatch(line, kind,  idx, tail, eqidx, just) {
    idx = index(line, "// lint-allow:")
    if (idx == 0) return 0
    tail = substr(line, idx + length("// lint-allow:"))
    tail = trim(tail)
    # Expect: `<kind>=<justification>`
    if (substr(tail, 1, length(kind) + 1) != kind "=") return 0
    just = substr(tail, length(kind) + 2)
    return justified(just)
}

BEGINFILE {
    reset_fn_state()
    # NR is global across all files; we want per-file line numbers for
    # findings. FNR resets at BEGINFILE, so use FNR everywhere via this
    # alias kept in sync on every record.
}

{
    raw = $0
    code = strip_comment(raw)

    # -- Escape-hatch detection on this line, before any rule fires. --------
    if (check_hatch(raw, "server-fn-context")) {
        next_line_context_allowed = 1
    }

    # -- State transitions ---------------------------------------------------
    if (!in_server_attr && !in_fn_sig && !in_fn_body) {
        if (raw ~ /^[[:space:]]*#\[server[[:space:](]/) {
            in_server_attr = 1
        }
    }

    # A callouts hatch applies to the current fn. Honor the hatch only when
    # we are already inside `#[server]` attribute, the signature, or the body
    # — this prevents stray hatches scattered elsewhere in the file from
    # silently suppressing the next unrelated fn.
    if ((in_server_attr || in_fn_sig || in_fn_body) \
        && check_hatch(raw, "server-fn-callouts")) {
        fn_callouts_allowed = 1
    }

    if (in_server_attr && !in_fn_sig && !in_fn_body) {
        # Detect the `pub async fn NAME(` or `async fn NAME(` that closes
        # the attribute. The server_fn attribute itself may span multiple
        # lines (arg list), so we only leave in_server_attr when we see the
        # fn keyword.
        if (match(raw, /(^|[[:space:]])(pub[[:space:]]+)?async[[:space:]]+fn[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/)) {
            piece = substr(raw, RSTART, RLENGTH)
            if (match(piece, /fn[[:space:]]+[A-Za-z_][A-Za-z0-9_]*/)) {
                name = substr(piece, RSTART, RLENGTH)
                sub(/^fn[[:space:]]+/, "", name)
                fn_name = name
            }
            in_server_attr = 0
            in_fn_sig = 1
            fn_start_line = FNR
            fn_callouts = 0
        }
    }

    # The fn body begins at the first `{` after the signature.
    if (in_fn_sig || in_fn_body) {
        closed = update_depth(code)
    }

    # -- Rule A: non-allowlisted context lookup. Only inside fn body. -------
    #
    # We locate each `use_context::<...>(` or `expect_context::<...>(` call
    # and extract the generic arg. The arg may contain nested generics like
    # `Arc<ConnectTokenService>`, so a flat `/<[^>]*>/` regex is not enough
    # — we find the opening `<` after `::` and walk forward counting `<`/
    # `>` until balance.
    if (in_fn_body) {
        probe = code
        while (match(probe, /(use_context|expect_context)[[:space:]]*::[[:space:]]*</)) {
            head_start = RSTART
            head_len = RLENGTH
            # Position of the opening `<` (1-based, relative to `probe`).
            lt_pos = head_start + head_len - 1
            # Walk forward from `lt_pos` to find the matching `>`.
            depth_lt = 0
            end_pos = 0
            for (i = lt_pos; i <= length(probe); i++) {
                ch = substr(probe, i, 1)
                if (ch == "<") depth_lt++
                else if (ch == ">") {
                    depth_lt--
                    if (depth_lt == 0) { end_pos = i; break }
                }
            }
            if (end_pos > 0) {
                # `(` must follow after optional whitespace to be a call.
                tail_start = end_pos + 1
                tail = substr(probe, tail_start)
                if (match(tail, /^[[:space:]]*\(/)) {
                    gen = substr(probe, lt_pos + 1, end_pos - lt_pos - 1)
                    b = base_type(gen)
                    if (b != "" && !(b in allow)) {
                        if (next_line_context_allowed) {
                            # Consume the per-call allow flag.
                            next_line_context_allowed = 0
                        } else {
                            printf "%s:%d:A non-allowlisted context lookup: %s — move DI into ServerContext or add // lint-allow: server-fn-context=<why>\n",
                                FILENAME, FNR, b
                        }
                    }
                }
                probe = substr(probe, end_pos + 1)
            } else {
                # Unbalanced `<` — stop scanning this line to avoid infinite
                # loop. Likely a multi-line generic that continues next line,
                # which the Kyomi codebase does not currently use.
                break
            }
        }

        # Rule B accumulator.
        if (!fn_callouts_allowed) {
            fn_callouts += count_callouts(code)
        }
    }

    # -- End of fn body: emit Rule B finding if over threshold. -------------
    if (in_fn_body && closed) {
        if (!fn_callouts_allowed && fn_callouts > max_callouts) {
            printf "%s:%d:B too many service-layer callouts in `%s` (%d > %d) — extract shared orchestration into a services::/kyomi_auth:: function or add // lint-allow: server-fn-callouts=<why>\n",
                FILENAME, fn_start_line, fn_name, fn_callouts, max_callouts
        }
        reset_fn_state()
        # Reset next_line_context_allowed too — it resets per fn already via
        # reset_fn_state, but outside fns we also want a stale flag cleared.
        next_line_context_allowed = 0
    }

    # A line without use_context/expect_context clears the one-shot context
    # hatch so a stale hatch comment cannot silently suppress a later lookup.
    if (!(code ~ /(use_context|expect_context)[[:space:]]*::/)) {
        # Only clear if this line was not itself the hatch comment.
        if (!check_hatch(raw, "server-fn-context")) {
            next_line_context_allowed = 0
        }
    }
}

ENDFILE {
    # Any fn left open at EOF is a parse failure — report but do not crash.
    if (in_fn_body) {
        printf "%s:%d:PARSE server_fn `%s` did not close before EOF (linter parse error, please investigate)\n",
            FILENAME, fn_start_line, fn_name
    }
    reset_fn_state()
}
'

# Run awk once over all targets. Collect findings, sort, emit to stderr.
# Separating WARN from violation is done after the sort.
findings="$(awk -v ALLOW_CSV="$(IFS=,; echo "${ALLOWLIST_TYPES[*]}")" \
               -v MAX="$CALLOUT_MAX" \
               "$awk_program" "${TARGETS[@]}" | LC_ALL=C sort -t: -k1,1 -k2,2n -k3,3)"

if [ -z "$findings" ]; then
    exit 0
fi

# Any non-WARN finding = violation. WARN lines are printed but do not
# change the exit code.
status=0
while IFS= read -r line; do
    printf '%s\n' "$line" >&2
    case "$line" in
        *:WARN*) ;;  # warning only
        *)
            status=1
            ;;
    esac
done <<< "$findings"

exit "$status"
