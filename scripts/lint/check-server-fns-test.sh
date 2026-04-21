#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-server-fns-test.sh — self-test for check-server-fns.sh
#
# Plants synthetic server_fn files in a tmp directory and invokes the linter
# against each one. Each fixture has a documented expected exit code; the test
# prints PASS/FAIL per fixture and exits non-zero if any assertion fails.
#
# Usage:
#   ./scripts/lint/check-server-fns-test.sh
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINTER="$SCRIPT_DIR/check-server-fns.sh"

if [ ! -x "$LINTER" ]; then
    echo "ERROR: linter not executable at $LINTER" >&2
    exit 2
fi

TMP="$(mktemp -d -t check-server-fns-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

fails=0
checks=0

# ------------------------------------------------------------------------------
# assert_linter — invoke linter on a fixture, assert exit code matches.
#
# Args:
#   $1  fixture label (printed in PASS/FAIL line)
#   $2  expected exit code (0 = clean, 1 = violations)
#   $3  fixture file path
# ------------------------------------------------------------------------------
assert_linter() {
    local label="$1"
    local expected="$2"
    local file="$3"
    local output
    local actual

    checks=$((checks + 1))

    # Capture both stderr and exit code. Don't let `set -e` kill us on a
    # non-zero exit from the linter — that's exactly what we're testing.
    set +e
    output="$("$LINTER" "$file" 2>&1)"
    actual=$?
    set -e

    if [ "$actual" -eq "$expected" ]; then
        printf 'PASS  %-40s  (exit %d as expected)\n' "$label" "$actual"
    else
        printf 'FAIL  %-40s  expected exit %d, got %d\n' "$label" "$expected" "$actual"
        if [ -n "$output" ]; then
            printf '      linter output:\n'
            printf '        %s\n' "$output" | sed 's/^        /      /'
        fi
        fails=$((fails + 1))
    fi
}

# ------------------------------------------------------------------------------
# assert_contains — assert linter output contains a substring.
# ------------------------------------------------------------------------------
assert_contains() {
    local label="$1"
    local needle="$2"
    local file="$3"
    local output

    checks=$((checks + 1))

    set +e
    output="$("$LINTER" "$file" 2>&1)"
    set -e

    if printf '%s' "$output" | grep -qF "$needle"; then
        printf 'PASS  %-40s  (found %q)\n' "$label" "$needle"
    else
        printf 'FAIL  %-40s  expected output to contain %q\n' "$label" "$needle"
        printf '      got:\n'
        printf '        %s\n' "$output" | sed 's/^        /      /'
        fails=$((fails + 1))
    fi
}

# ------------------------------------------------------------------------------
# Fixtures live under $TMP/crates/kyomi-ui/src/server_fns/ so the linter's
# path-prefix filter accepts them. The linter resolves the script-root via
# its own location, so we symlink the fixture dir into the real server_fns
# path? Simpler: skip the filter by naming the files with `.rs` extension and
# passing an absolute path that starts with the linter's SERVER_FN_DIR.
#
# ...but that would contaminate the real tree. Better: create a mock tree
# under $TMP and invoke the linter with --no-path-filter behavior. The
# linter has no such flag. So we construct the expected directory layout
# under $TMP and point the linter at those files — but the path filter
# rejects them because they don't live under the real server_fns.
#
# Workaround: the linter filters paths to the REAL server_fns dir. Since
# we want to test synthetic files, we must either (a) add a test mode to
# the linter, or (b) drop fixtures into the real server_fns dir
# temporarily. (b) is racy; (a) pollutes production for test.
#
# Cleanest: run the linter pointed at a fixture *file* and skip the prefix
# filter by checking the `*.rs` suffix only when the file lives outside the
# known server_fns dir. The linter already skips unknown dirs silently —
# which means test fixtures would pass (exit 0) vacuously and not prove
# anything.
#
# Correct solution: invoke awk directly on our fixtures, using the same awk
# program the linter uses. We extract the awk program body by sourcing the
# linter's variables (ALLOW_CSV, MAX) and inlining the awk block. This is
# brittle — so instead, we make the linter accept a test-mode sentinel
# via an environment variable that widens the path filter. See SERVER_FN_LINT_DIR
# in check-server-fns.sh.
# ------------------------------------------------------------------------------

# Writable fixture dir acts as the linter's target. The linter consults
# $SERVER_FN_LINT_DIR if set, falling back to the real server_fns path.
export SERVER_FN_LINT_DIR="$TMP/server_fns"
mkdir -p "$SERVER_FN_LINT_DIR"

# ------------------------------------------------------------------------------
# Fixture: bad_context.rs — Rule A violation (non-allowlisted context type).
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/bad_context.rs" <<'EOF'
// Fixture: non-allowlisted context lookup inside a server_fn body.
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn bad_context_fn() -> Result<(), ServerFnError> {
    let svc = use_context::<std::sync::Arc<ConnectTokenService>>()
        .ok_or_else(|| ServerFnError::new("missing"))?;
    let _ = svc;
    Ok(())
}
EOF
assert_linter "bad_context.rs"                    1 "$SERVER_FN_LINT_DIR/bad_context.rs"
assert_contains "bad_context.rs reports Rule A"   "ConnectTokenService" "$SERVER_FN_LINT_DIR/bad_context.rs"

# ------------------------------------------------------------------------------
# Fixture: bad_callouts.rs — Rule B violation (5 distinct callouts, max=3).
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/bad_callouts.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn bad_callouts_fn() -> Result<(), ServerFnError> {
    let a = kyomi_auth::user_service::get_user(&pool).await?;
    let b = kyomi_auth::user_service::list_users(&pool).await?;
    let c = kyomi_knowledge::docs::find(&pool).await?;
    let d = sqlx::query("SELECT 1").fetch_one(&pool).await?;
    let e = kyomi_core::db_execute!(&pool, "UPDATE foo SET x=1")?;
    let _ = (a, b, c, d, e);
    Ok(())
}
EOF
assert_linter "bad_callouts.rs"                   1 "$SERVER_FN_LINT_DIR/bad_callouts.rs"
assert_contains "bad_callouts.rs reports Rule B"  "bad_callouts_fn" "$SERVER_FN_LINT_DIR/bad_callouts.rs"

# ------------------------------------------------------------------------------
# Fixture: good_allowlisted.rs — only uses allowlisted context types. Clean.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/good_allowlisted.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn good_fn() -> Result<(), ServerFnError> {
    let _ctx = use_context::<ServerContext>()
        .ok_or_else(|| ServerFnError::new("missing"))?;
    let _auth = expect_context::<AuthUser>();
    let _resp = expect_context::<leptos_axum::ResponseOptions>();
    Ok(())
}
EOF
assert_linter "good_allowlisted.rs"               0 "$SERVER_FN_LINT_DIR/good_allowlisted.rs"

# ------------------------------------------------------------------------------
# Fixture: escape_hatch_context.rs — same as bad_context but with a justified
# hatch on the preceding line.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/escape_hatch_context.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn hatched_context_fn() -> Result<(), ServerFnError> {
    // lint-allow: server-fn-context=legitimate test-only DI with documented reason
    let svc = use_context::<std::sync::Arc<ConnectTokenService>>()
        .ok_or_else(|| ServerFnError::new("missing"))?;
    let _ = svc;
    Ok(())
}
EOF
assert_linter "escape_hatch_context.rs"           0 "$SERVER_FN_LINT_DIR/escape_hatch_context.rs"

# ------------------------------------------------------------------------------
# Fixture: escape_hatch_callouts.rs — same as bad_callouts but with a
# justified hatch comment inside the fn body.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/escape_hatch_callouts.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn hatched_callouts_fn() -> Result<(), ServerFnError> {
    // lint-allow: server-fn-callouts=legitimate orchestration test justification
    let a = kyomi_auth::user_service::get_user(&pool).await?;
    let b = kyomi_auth::user_service::list_users(&pool).await?;
    let c = kyomi_knowledge::docs::find(&pool).await?;
    let d = sqlx::query("SELECT 1").fetch_one(&pool).await?;
    let e = kyomi_core::db_execute!(&pool, "UPDATE foo SET x=1")?;
    let _ = (a, b, c, d, e);
    Ok(())
}
EOF
assert_linter "escape_hatch_callouts.rs"          0 "$SERVER_FN_LINT_DIR/escape_hatch_callouts.rs"

# ------------------------------------------------------------------------------
# Fixture: empty_escape_hatch.rs — hatch comment with no justification. Must
# FAIL because empty justification is treated as if the hatch weren't there,
# plus emit a WARN about the empty hatch.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/empty_escape_hatch.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn empty_hatch_fn() -> Result<(), ServerFnError> {
    // lint-allow: server-fn-callouts=
    let a = kyomi_auth::user_service::get_user(&pool).await?;
    let b = kyomi_auth::user_service::list_users(&pool).await?;
    let c = kyomi_knowledge::docs::find(&pool).await?;
    let d = sqlx::query("SELECT 1").fetch_one(&pool).await?;
    let _ = (a, b, c, d);
    Ok(())
}
EOF
assert_linter "empty_escape_hatch.rs"             1 "$SERVER_FN_LINT_DIR/empty_escape_hatch.rs"
assert_contains "empty_escape_hatch.rs emits WARN" "WARN" "$SERVER_FN_LINT_DIR/empty_escape_hatch.rs"

# ------------------------------------------------------------------------------
# Fixture: multiple_fns.rs — exercises state-machine boundaries: hatch on one
# fn must NOT leak to the next.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/multiple_fns.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn first_fn() -> Result<(), ServerFnError> {
    // lint-allow: server-fn-callouts=first-fn-specific documented reason here
    let a = kyomi_auth::user_service::get_user(&pool).await?;
    let b = kyomi_auth::user_service::list_users(&pool).await?;
    let c = kyomi_knowledge::docs::find(&pool).await?;
    let d = sqlx::query("SELECT 1").fetch_one(&pool).await?;
    let _ = (a, b, c, d);
    Ok(())
}

#[server(prefix = "/leptos-api")]
pub async fn second_fn() -> Result<(), ServerFnError> {
    // No hatch here — Rule B should fire.
    let a = kyomi_auth::user_service::get_user(&pool).await?;
    let b = kyomi_auth::user_service::list_users(&pool).await?;
    let c = kyomi_knowledge::docs::find(&pool).await?;
    let d = sqlx::query("SELECT 1").fetch_one(&pool).await?;
    let _ = (a, b, c, d);
    Ok(())
}
EOF
assert_linter "multiple_fns.rs (hatch doesn't leak)" 1 "$SERVER_FN_LINT_DIR/multiple_fns.rs"
assert_contains "multiple_fns.rs reports second_fn"  "second_fn"       "$SERVER_FN_LINT_DIR/multiple_fns.rs"

# Also assert first_fn is NOT flagged (only second_fn).
output="$("$LINTER" "$SERVER_FN_LINT_DIR/multiple_fns.rs" 2>&1 || true)"
if printf '%s' "$output" | grep -qF "first_fn"; then
    echo 'FAIL  multiple_fns.rs                          first_fn should NOT be flagged'
    echo "      got: $output"
    fails=$((fails + 1))
else
    echo 'PASS  multiple_fns.rs leak check              (first_fn not flagged)'
fi
checks=$((checks + 1))

# ------------------------------------------------------------------------------
# Fixture: env_override.rs — Rule B threshold overridable via env var. Same
# body as bad_callouts (5 callouts), but SERVER_FN_CALLOUT_MAX=10 should
# accept it.
# ------------------------------------------------------------------------------
cp "$SERVER_FN_LINT_DIR/bad_callouts.rs" "$SERVER_FN_LINT_DIR/env_override.rs"
checks=$((checks + 1))
set +e
out="$(SERVER_FN_CALLOUT_MAX=10 "$LINTER" "$SERVER_FN_LINT_DIR/env_override.rs" 2>&1)"
rc=$?
set -e
if [ "$rc" -eq 0 ]; then
    echo 'PASS  env_override SERVER_FN_CALLOUT_MAX=10   (exit 0)'
else
    echo "FAIL  env_override SERVER_FN_CALLOUT_MAX=10   expected exit 0 got $rc"
    echo "      output: $out"
    fails=$((fails + 1))
fi

# ------------------------------------------------------------------------------
# Fixture: non_server_fn.rs — functions not marked `#[server]` are ignored,
# even if they reference kyomi_auth heavily.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/non_server_fn.rs" <<'EOF'
use leptos::prelude::*;

pub async fn plain_fn() -> Result<(), ServerFnError> {
    let a = kyomi_auth::user_service::get_user(&pool).await?;
    let b = kyomi_auth::user_service::list_users(&pool).await?;
    let c = kyomi_knowledge::docs::find(&pool).await?;
    let d = sqlx::query("SELECT 1").fetch_one(&pool).await?;
    let e = kyomi_core::db_execute!(&pool, "UPDATE foo SET x=1")?;
    let _ = (a, b, c, d, e);
    Ok(())
}
EOF
assert_linter "non_server_fn.rs"                  0 "$SERVER_FN_LINT_DIR/non_server_fn.rs"

# ------------------------------------------------------------------------------
# Fixture: comment_content.rs — use_context in a // comment must not trigger.
# ------------------------------------------------------------------------------
cat > "$SERVER_FN_LINT_DIR/comment_content.rs" <<'EOF'
use leptos::prelude::*;

#[server(prefix = "/leptos-api")]
pub async fn comment_fn() -> Result<(), ServerFnError> {
    // Historically this called use_context::<ConnectTokenService>() — fixed.
    let _ctx = use_context::<ServerContext>()
        .ok_or_else(|| ServerFnError::new("missing"))?;
    Ok(())
}
EOF
assert_linter "comment_content.rs"                0 "$SERVER_FN_LINT_DIR/comment_content.rs"

# ------------------------------------------------------------------------------
# Summary.
# ------------------------------------------------------------------------------
printf '\n%d checks, %d failed\n' "$checks" "$fails"
if [ "$fails" -eq 0 ]; then
    echo 'All fixtures passed.'
    exit 0
fi
exit 1
