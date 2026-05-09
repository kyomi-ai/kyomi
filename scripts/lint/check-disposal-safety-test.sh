#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-disposal-safety-test.sh — self-test for check-disposal-safety.sh
#
# Creates synthetic Rust fixtures, runs the linter against them, and verifies
# the findings match expectations. Exit 0 = all pass, exit 1 = failures.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LINTER="$SCRIPT_DIR/check-disposal-safety.sh"
PASS=0
FAIL=0

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

# Helper: write a fixture, run the linter, check for expected violations.
# Usage: expect_violations <fixture_name> <expected_A_count> <expected_B_warn_count> <<'RUST'
#   ... Rust code ...
# RUST
run_test() {
    local name="$1" expected_a="$2" expected_b="$3"
    local fixture="$tmpdir/$name.rs"

    cat > "$fixture"

    # Point the linter at our temp dir.
    local output
    output="$(DISPOSAL_LINT_DIR="$tmpdir" "$LINTER" "$fixture" 2>&1)" || true

    local got_a got_b
    got_a="$(echo "$output" | grep -c ":A " || true)"
    got_b="$(echo "$output" | grep -c ":WARN:B " || true)"

    if [ "$got_a" -eq "$expected_a" ] && [ "$got_b" -eq "$expected_b" ]; then
        printf "  ✓ %s (A=%d B=%d)\n" "$name" "$got_a" "$got_b"
        PASS=$((PASS + 1))
    else
        printf "  ✗ %s — expected A=%d B=%d, got A=%d B=%d\n" "$name" "$expected_a" "$expected_b" "$got_a" "$got_b"
        if [ -n "$output" ]; then
            echo "    output:"
            echo "$output" | sed 's/^/    | /'
        fi
        FAIL=$((FAIL + 1))
    fi
}

echo "Running disposal safety lint tests..."
echo

# ─── Test 1: spawn_local with bare .set() — should flag ──────────────────
run_test "spawn_local_bare_set" 2 0 <<'RUST'
fn my_component() {
    let (value, set_value) = signal(0);
    spawn_local(async move {
        let result = fetch_data().await;
        set_value.set(result);
        set_loading.set(false);
    });
}
RUST

# ─── Test 2: spawn_local with .try_set() — should pass ──────────────────
run_test "spawn_local_try_set" 0 0 <<'RUST'
fn my_component() {
    let (value, set_value) = signal(0);
    spawn_local(async move {
        let result = fetch_data().await;
        set_value.try_set(result);
        set_loading.try_set(false);
    });
}
RUST

# ─── Test 3: synchronous .set() outside spawn_local — should pass ───────
run_test "sync_set_ok" 0 0 <<'RUST'
fn my_component() {
    let (value, set_value) = signal(0);
    let on_click = move |_| {
        set_value.set(42);
    };
}
RUST

# ─── Test 4: Signal::derive with .get() — should warn (Rule B) ──────────
run_test "derive_bare_get" 0 1 <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.get();
        items
    });
}
RUST

# ─── Test 5: Signal::derive with .try_get() — should pass ───────────────
run_test "derive_try_get" 0 0 <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.try_get();
        items
    });
}
RUST

# ─── Test 6: escape hatch — should pass ─────────────────────────────────
run_test "escape_hatch" 0 0 <<'RUST'
fn my_component() {
    spawn_local(async move {
        set_value.set(result); // lint-allow: disposal-safe=this signal outlives the component
    });
}
RUST

# ─── Test 7: escape hatch too short — should flag + warn ────────────────
run_test "escape_hatch_short" 1 0 <<'RUST'
fn my_component() {
    spawn_local(async move {
        set_value.set(result); // lint-allow: disposal-safe=ok
    });
}
RUST

# ─── Test 8: test module — should skip ──────────────────────────────────
run_test "test_module_skip" 0 0 <<'RUST'
fn production_code() {}

#[cfg(test)]
mod tests {
    fn test_something() {
        spawn_local(async move {
            set_value.set(42);
        });
        let x = Signal::derive(move || {
            val.get()
        });
    }
}
RUST

# ─── Test 9: Timeout callback with .set() — should flag ─────────────────
run_test "timeout_bare_set" 1 0 <<'RUST'
fn my_component() {
    let handle = gloo_timers::callback::Timeout::new(300, move || {
        set_is_searching.set(true);
    });
}
RUST

# ─── Test 10: .update() inside spawn_local — should flag ────────────────
run_test "spawn_local_bare_update" 1 0 <<'RUST'
fn my_component() {
    spawn_local(async move {
        counter.update(|v| *v += 1);
    });
}
RUST

# ─── Test 11: .update_value() inside spawn_local — should pass ──────────
run_test "spawn_local_update_value_ok" 0 0 <<'RUST'
fn my_component() {
    spawn_local(async move {
        stored.update_value(|v| *v += 1);
    });
}
RUST

# ─── Test 12: nested spawn_local — inner .set() should flag ─────────────
run_test "nested_spawn_local" 1 0 <<'RUST'
fn my_component() {
    spawn_local(async move {
        let result = fetch().await;
        set_outer.try_set(result);
        spawn_local(async move {
            set_inner.set(true);
        });
    });
}
RUST

# ─── Test 13: Memo::new with .get() — should warn (Rule B) ──────────────
run_test "memo_new_bare_get" 0 1 <<'RUST'
fn my_component() {
    let filtered = Memo::new(move |_| {
        data.get()
    });
}
RUST

# ─── Test 14: .get_untracked() inside Timeout — should flag ──────────────
run_test "timeout_bare_get_untracked" 1 0 <<'RUST'
fn my_component() {
    let timeout = Timeout::new(1_000, move || {
        let val = signal.get_untracked();
    });
}
RUST

# ─── Test 15: .try_get_untracked() inside Timeout — should pass ─────────
run_test "timeout_try_get_untracked" 0 0 <<'RUST'
fn my_component() {
    let timeout = Timeout::new(1_000, move || {
        let val = signal.try_get_untracked();
    });
}
RUST

# ─── Test 16: set_timeout (leptos) with .update() — should flag ─────────
run_test "leptos_set_timeout_update" 1 0 <<'RUST'
fn my_component() {
    set_timeout(
        move || {
            state.toasts.update(|t| t.clear());
        },
        std::time::Duration::from_millis(3000),
    );
}
RUST

echo
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
