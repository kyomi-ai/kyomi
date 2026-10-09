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
#
# KYO-679: also pins DISPOSAL_BASELINE_FILE to a scratch path that is never
# created, so every Rule B match in these fixtures is deterministically
# "not in the baseline" and therefore reported as ERROR:B, never WARN:B —
# these tests exist to pin Rule B's pattern-matching (what fires and what
# doesn't), not its baseline status, and forcing an always-empty baseline
# here decouples that from whatever scripts/lint/disposal-safety-baseline.txt
# happens to contain (it would otherwise default to the real checked-in
# baseline, which never matches these /tmp fixture paths anyway — but
# relying on that non-overlap by accident is worse than making it explicit).
run_test() {
    local name="$1" expected_a="$2" expected_b="$3"
    local fixture="$tmpdir/$name.rs"

    cat > "$fixture"

    # Point the linter at our temp dir, and at a baseline that never exists.
    local output
    output="$(DISPOSAL_LINT_DIR="$tmpdir" DISPOSAL_BASELINE_FILE="$tmpdir/.no-baseline.txt" "$LINTER" "$fixture" 2>&1)" || true

    local got_a got_b
    got_a="$(echo "$output" | grep -c ":A " || true)"
    got_b="$(echo "$output" | grep -c ":ERROR:B " || true)"

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

# ─── KYO-414 regression tests ────────────────────────────────────────────
#
# These pin the false-positive/false-negative bugs KYO-414 fixed. Each one
# is a real pattern this repo has actually hit — see the ticket and the
# script header for where.

# ─── Test 17: this is the ticket's own reproduction — a source-text
# assertion (the `SRC.find("...")` self-inspection pattern this codebase
# uses, see datasources/tests/) quoting the EXACT trigger + bare .get() text
# as a string literal. Must not fire itself, AND — since an unstripped
# string containing "Signal::derive(" with no `{` on the same line is
# exactly what used to arm the KYO-414 flag-leak (Test 19) — must not leak
# into the unrelated closure that follows either. Both assertions only mean
# something together: a string-unaware matcher that merely failed to find
# the pattern for some other coincidental reason would still show 0 here.
run_test "literal_get_in_string_no_fire" 0 0 <<'RUST'
fn checks_source_for_marker() {
    let found = SRC
        .find("let x = Signal::derive(move || connection_auth_modes_unavailable.get());")
        .is_some();
    assert!(found);

    if some_condition {
        is_admin.get();
    }
}
RUST

# ─── Test 18: same as above but via a raw string, the other literal form
# this codebase uses (e.g. JSON placeholder text) — should also not fire,
# and must not leak into the (multi-line, so it stays open long enough for
# a leaked flag to attach to it) block that follows either.
run_test "literal_get_in_raw_string_no_fire" 0 0 <<'RUST'
fn checks_source_for_marker() {
    let found = SRC.find(r#"Signal::derive(move || flag.get())"#).is_some();

    if some_condition {
        is_admin.get();
    }
}
RUST

# ─── Test 19: THE core KYO-414 regression. A single-line Signal::derive
# with no block body of its own (`Signal::derive(move || x.get()...)`,
# closing on the same line) must still fire Rule B for itself — but must
# NOT leak into the ordinary <Show>/interpolation closures that follow,
# which is exactly the false-positive pattern reported against
# datasources.rs (KYO-414 comment: <Show when=move || is_admin.get()> and
# {move || auth_mode_description(...)} both flagged, neither anywhere near
# a derive). Expect exactly one B — for the derive line only.
run_test "single_line_derive_does_not_leak_into_show" 0 1 <<'RUST'
fn my_component() {
    let is_edit_mode = Signal::derive(move || datasource_id.get().is_some());

    view! {
        <Show when=move || is_admin.get()>
            <AdminPanel/>
        </Show>
        {move || format_description(&auth_modes.get())}
    }
}
RUST

# ─── Test 20: mutation-proof companion to Test 19 — a <Show> closure with
# no preceding single-line derive at all must also not fire on its own.
run_test "show_closure_alone_no_fire" 0 0 <<'RUST'
fn my_component() {
    view! {
        <Show when=move || is_admin.get()>
            <AdminPanel/>
        </Show>
    }
}
RUST

# ─── Test 21: a `//` inside a string (a URL) must not be treated as a line
# comment and truncate the rest of the line — that would silently hide a
# real violation appearing after it on the same line.
run_test "url_slash_slash_does_not_hide_later_violation" 1 0 <<'RUST'
fn my_component() {
    spawn_local(async move {
        let url = "https://kyomi.ai"; set_value.set(result);
    });
}
RUST

# ─── Test 22: `mod tests;` (an external submodule declaration, KYO-455
# split-test-module style — no block body, ever) must not arm the test-
# module skip and swallow every line after it waiting for a `{` that will
# never belong to it. A real violation appearing later in the same file
# must still be caught.
run_test "mod_tests_semicolon_does_not_swallow_rest_of_file" 1 0 <<'RUST'
fn production_code_before() {}

#[cfg(test)]
mod tests;

fn production_code_after() {
    spawn_local(async move {
        set_value.set(42);
    });
}
RUST

# ─── Test 23: a Signal::derive whose own block is on a LATER line than the
# call (`Signal::derive(\n  move || {\n ...`) must still be recognized as
# block-form and fire Rule B for a bare .get() inside it — proves multi-
# line trigger-to-block detection isn't limited to spawn/Timeout/set_timeout
# (Test 16); it works the same way for derives.
run_test "multiline_open_derive_still_detected" 0 1 <<'RUST'
fn my_component() {
    let filtered = Signal::derive(
        move || {
            store_signal.get()
        }
    );
}
RUST

# ─── Test 24: KYO-414 follow-up regression. A Leptos signal read is ALWAYS
# `.get()` with no argument. `.get(key)` inside a Signal::derive/Memo block
# is some other type's accessor entirely -- serde_json::Value::get(key),
# HashMap::get(&k), a slice/Vec .get(idx) -- and can never be the disposal
# hazard Rule B exists to catch, even though it sits lexically inside a
# derive. Real reproduction: datasources.rs:7976 is
# `v.get("client_email")?.as_str()...` on a serde_json::Value. Must not
# fire on its own.
run_test "get_with_arg_inside_derive_is_not_a_signal_read" 0 0 <<'RUST'
fn my_component() {
    let email = Signal::derive(move || {
        config.get("client_email").cloned().unwrap_or_default()
    });
}
RUST

# ─── Test 25: mutation-proof companion to Test 24 -- a genuine bare .get()
# sitting in the SAME derive block as a .get(key) call must still fire
# exactly once; the arg-taking accessor must neither be miscounted as a
# violation nor suppress detection of the real one next to it.
run_test "bare_get_still_fires_next_to_get_with_arg" 0 1 <<'RUST'
fn my_component() {
    let combined = Signal::derive(move || {
        let email = config.get("client_email").cloned().unwrap_or_default();
        let flag = enabled_signal.get();
        format!("{email}-{flag}")
    });
}
RUST


# ─── KYO-558 regression tests ────────────────────────────────────────────
#
# The test-module skip used to arm only on the exact literal text
# `#[cfg(test)]`. These pin the fix that broadens it to also recognize
# `test` wrapped in `all(...)`/`any(...)`, in any position — this crate's
# standard `#[cfg(all(test, feature = "ssr"))]` ssr-gated-test-module shape
# — without also arming on `not(test)` or a `test`-prefixed feature name
# that is not actually the `test` predicate.

# ─── Test 26: the ticket's exact shape — `#[cfg(all(test, feature =
# "ssr"))] mod disposal_scope_tests { ... }` (KYO-548) containing BOTH a
# Rule A trigger and a Rule B trigger. Must be fully skipped: A=0 B=0.
run_test "cfg_all_test_ssr_module_skipped" 0 0 <<'RUST'
fn production_code() {}

#[cfg(all(test, feature = "ssr"))]
mod disposal_scope_tests {
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

# ─── Test 27: same idea via the `any(...)` form — must also be fully
# skipped.
run_test "cfg_any_test_module_skipped" 0 0 <<'RUST'
fn production_code() {}

#[cfg(any(test, feature = "x"))]
mod misc_tests {
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

# ─── Test 28: `test` in a non-first position inside `all(...)` — must
# still be fully skipped.
run_test "cfg_all_test_non_first_position_skipped" 0 0 <<'RUST'
fn production_code() {}

#[cfg(all(feature = "ssr", test))]
mod ssr_first_tests {
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

# ─── Test 29: negative — `#[cfg(not(test))]` must NOT arm the skip. This
# is the guard that stops the fix from blanket-disabling the lint: a
# module that is explicitly excluded FROM test builds is still production
# code the lint must check.
run_test "cfg_not_test_still_fires" 1 0 <<'RUST'
fn production_code() {}

#[cfg(not(test))]
mod prod_only {
    fn real_code() {
        spawn_local(async move {
            set_value.set(42);
        });
    }
}
RUST

# ─── Test 30: negative — `test` as a prefix of a longer feature name
# (`test_helpers`, not the bare `test` predicate) must NOT arm the skip.
run_test "cfg_test_prefix_feature_still_fires" 1 0 <<'RUST'
fn production_code() {}

#[cfg(feature = "test_helpers")]
mod helpers {
    fn real_code() {
        spawn_local(async move {
            set_value.set(42);
        });
    }
}
RUST

# ─── Test 31: CRITICAL-1 regression — `#[cfg(not(all(test, feature =
# "x")))]` is a predicate that is TRUE in essentially every production
# build (it only excludes the narrow case where BOTH test AND feature=x
# hold), yet a token-anywhere search for `test` (the pre-fix behavior)
# matched the `test` inside it anyway and wrongly treated the module as
# test-only. cfg_predicate_is_test() only strips a DIRECT `not(test)`
# wrap; this predicate isn't that shape, isn't bare `test`, and isn't a
# flat `all(...)`/`any(...)`, so it must fail closed (not recognized as
# test) and the skip must NOT arm — Rule A must still fire.
run_test "cfg_not_all_test_wrapped_still_fires" 1 0 <<'RUST'
fn production_code() {}

#[cfg(not(all(test, feature = "x")))]
mod weird_prod_module {
    fn real_code() {
        spawn_local(async move {
            set_value.set(42);
        });
    }
}
RUST

# ─── Test 32: CRITICAL-2 regression — a semicolon-form module
# declaration named something OTHER than exactly `tests` (KYO-455 split-
# test-module style, no body ever), reached via the KYO-558-broadened cfg
# match, must not arm in_test_module and swallow the rest of the file.
# Before the fix, `in_test_module` only reset on a `{` that a semicolon
# declaration never produces — so a Rule A trigger appearing anywhere
# after this declaration would go unscanned. Must still fire.
run_test "semicolon_mod_other_name_does_not_swallow_rest_of_file" 1 0 <<'RUST'
fn production_code() {}

#[cfg(all(test, feature = "ssr"))]
mod some_tests;

fn after_semicolon_decl() {
    spawn_local(async move {
        set_value.set(42);
    });
}
RUST

# ─── Test 33: CRITICAL-1 cycle-2 regression — `#[cfg(any(not(test),
# test))]` is a tautology (`NOT test OR test` is always true), so the
# item it wraps compiles in PRODUCTION too, not just under `cfg(test)`.
# The unanchored `gsub` this fix replaces stripped `not(test)` from
# anywhere in the string, including this nested position, disguising the
# predicate as the flat `any( , test)` before the flatness check ran —
# which then wrongly armed the skip and silently disabled the BLOCKING
# Rule A on shipping code. `not(...)` can only ever appear as a nested
# paren group, so with no substitution step the flat-shape regex
# (`[^()]*`, no parens allowed inside the wrapper) rejects this predicate
# outright: fails closed, Rule A must still fire.
run_test "cfg_any_not_test_test_tautology_still_fires" 1 0 <<'RUST'
fn production_code() {}

#[cfg(any(not(test), test))]
mod the_module {
    fn f() {
        spawn_local(async move {
            set_value.set(1);
        });
    }
}
RUST

# ─── Test 34: CRITICAL-1 cycle-2 regression, `all(...)` variant —
# `#[cfg(all(not(test), test))]` is unsatisfiable (`NOT test AND test` is
# always false), so the item it wraps is dead code in every build, test
# or production — no build ever activates it. The old unanchored `gsub`
# still stripped `not(test)` out of this nested position too, disguising
# it as the flat `all( , test)` and wrongly arming the skip so Rule A
# went silent over dead code that should still be linted as ordinary
# production code (nothing about `cfg` shape entitles code to a Rule A
# exemption merely because it never builds). Fails closed the same way as
# test 33: Rule A must still fire.
run_test "cfg_all_not_test_test_unsatisfiable_still_fires" 1 0 <<'RUST'
fn production_code() {}

#[cfg(all(not(test), test))]
mod the_module {
    fn f() {
        spawn_local(async move {
            set_value.set(1);
        });
    }
}
RUST

# ─── KYO-679 regression tests: the Rule B baseline ratchet ─────────────────
#
# Everything above exercises check-disposal-safety.sh's pattern-matching via
# raw A/B counts. These tests exercise the layer KYO-679 added on top of
# Rule B: scripts/lint/disposal-safety-baseline.txt, and the exit-code /
# ERROR:B behaviour that baseline gates. Every test below overrides BOTH
# DISPOSAL_LINT_DIR (as above) AND DISPOSAL_BASELINE_FILE, so none of them
# ever reads or writes the real checked-in baseline.
echo
echo "Running Rule B baseline ratchet tests (KYO-679)..."
echo

kyo679dir="$tmpdir/kyo679"
mkdir -p "$kyo679dir"

# Runs the linter with the given hermetic DISPOSAL_LINT_DIR / baseline file,
# capturing both output and exit code. A plain `out="$(cmd)"` assignment
# would trip this script's own `set -e` on a non-zero exit (that failure
# happens outside any tested context) — bracket it in set +e/-e instead of
# masking the exit code entirely the way the run_test() helper above does
# with `|| true`, since these tests need the real exit code.
run_linter() {
    local lintdir="$1" baseline="$2"
    shift 2
    set +e
    BL_OUTPUT="$(DISPOSAL_LINT_DIR="$lintdir" DISPOSAL_BASELINE_FILE="$baseline" "$LINTER" "$@" 2>&1)"
    BL_RC=$?
    set -e
}

assert_rc() {
    local desc="$1" expected="$2" actual="$3"
    if [ "$actual" -eq "$expected" ]; then
        printf "  ✓ %s (rc=%d)\n" "$desc" "$actual"
        PASS=$((PASS + 1))
    else
        printf "  ✗ %s — expected rc=%d, got rc=%d\n" "$desc" "$expected" "$actual"
        echo "    output:"
        echo "$BL_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_contains() {
    local desc="$1" needle="$2"
    if printf '%s' "$BL_OUTPUT" | grep -qF -- "$needle"; then
        printf "  ✓ %s\n" "$desc"
        PASS=$((PASS + 1))
    else
        printf "  ✗ %s — expected output to contain: %s\n" "$desc" "$needle"
        echo "    output:"
        echo "$BL_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    fi
}

assert_not_contains() {
    local desc="$1" needle="$2"
    if printf '%s' "$BL_OUTPUT" | grep -qF -- "$needle"; then
        printf "  ✗ %s — expected output NOT to contain: %s\n" "$desc" "$needle"
        echo "    output:"
        echo "$BL_OUTPUT" | sed 's/^/    | /'
        FAIL=$((FAIL + 1))
    else
        printf "  ✓ %s\n" "$desc"
        PASS=$((PASS + 1))
    fi
}

assert_line_count() {
    local desc="$1" file="$2" expected="$3" actual
    if [ -f "$file" ]; then
        actual="$(wc -l < "$file" | tr -d ' ')"
    else
        actual=0
    fi
    if [ "$actual" -eq "$expected" ]; then
        printf "  ✓ %s (lines=%d)\n" "$desc" "$actual"
        PASS=$((PASS + 1))
    else
        printf "  ✗ %s — expected %d lines, got %d\n" "$desc" "$expected" "$actual"
        FAIL=$((FAIL + 1))
    fi
}

assert_files_identical() {
    local desc="$1" f1="$2" f2="$3"
    if diff -q "$f1" "$f2" >/dev/null 2>&1; then
        printf "  ✓ %s\n" "$desc"
        PASS=$((PASS + 1))
    else
        printf "  ✗ %s — files differ\n" "$desc"
        FAIL=$((FAIL + 1))
    fi
}

# ─── Criterion 1: a newly introduced bare .get() in a Signal::derive, in a
# file not previously baselined, exits non-zero — both in full-tree mode
# (no file args, what CI runs) and file-scoped mode (what pre-commit runs).
t1="$kyo679dir/t1"
mkdir -p "$t1"
cat > "$t1/new_site.rs" <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.get();
        items
    });
}
RUST
# No baseline file at all (not even empty) — must be treated as zero entries.
run_linter "$t1" "$t1/baseline.txt"
assert_rc "criterion 1: brand-new site, full-tree, no baseline file" 1 "$BL_RC"
assert_contains "criterion 1: full-tree reports ERROR:B" "ERROR:B"

t1b="$kyo679dir/t1b"
mkdir -p "$t1b"
cp "$t1/new_site.rs" "$t1b/new_site.rs"
: > "$t1b/baseline.txt"
t1b_abs="$(cd "$t1b" && pwd)/new_site.rs"
run_linter "$t1b" "$t1b/baseline.txt" "$t1b_abs"
assert_rc "criterion 1: brand-new site, file-scoped, empty baseline" 1 "$BL_RC"
assert_contains "criterion 1: file-scoped reports ERROR:B" "ERROR:B"

# ─── Criterion 2: existing baselined sites do not fail the build. Generates
# the baseline via --update-baseline (round-tripping the real mechanism
# rather than the test hand-computing its own hash) and reruns.
t2="$kyo679dir/t2"
mkdir -p "$t2"
cat > "$t2/existing_site.rs" <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.get();
        items
    });
}
RUST
: > "$t2/baseline.txt"
run_linter "$t2" "$t2/baseline.txt" --update-baseline
assert_rc "criterion 2: --update-baseline succeeds" 0 "$BL_RC"
assert_line_count "criterion 2: baseline has exactly one entry" "$t2/baseline.txt" 1

run_linter "$t2" "$t2/baseline.txt"
assert_rc "criterion 2: baselined site does not fail a normal run" 0 "$BL_RC"
assert_not_contains "criterion 2: baselined site produces no ERROR:B" "ERROR:B"
assert_not_contains "criterion 2: baselined site produces no WARN:B (silent by default)" "WARN:B"

# --show-baselined smoke test: the covered finding should reappear, tagged.
run_linter "$t2" "$t2/baseline.txt" --show-baselined
assert_rc "--show-baselined: still exits 0" 0 "$BL_RC"
assert_contains "--show-baselined: prints the covered finding as BASELINED:B" "BASELINED:B"

# ─── Criterion 3: moving a baselined site within its file (line-number
# churn from unrelated lines inserted above it) does not trip the ratchet —
# the baseline hashes content, not line number.
t3="$kyo679dir/t3"
mkdir -p "$t3"
cat > "$t3/moved_site.rs" <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.get();
        items
    });
}
RUST
: > "$t3/baseline.txt"
run_linter "$t3" "$t3/baseline.txt" --update-baseline
assert_rc "criterion 3: baseline generated before the move" 0 "$BL_RC"

cat > "$t3/moved_site.rs" <<'RUST'
fn my_component() {
    // an unrelated comment
    // a second unrelated comment, pushing the flagged line further down
    let filtered = Signal::derive(move || {
        let items = store_signal.get();
        items
    });
}
RUST
run_linter "$t3" "$t3/baseline.txt"
assert_rc "criterion 3: same content at a new line number still passes" 0 "$BL_RC"
assert_not_contains "criterion 3: no ERROR:B after the move" "ERROR:B"

# ─── Criterion 4: annotating a baselined site with the escape hatch removes
# it from the warning output entirely, and regenerating drops it from the
# baseline.
t4="$kyo679dir/t4"
mkdir -p "$t4"
cat > "$t4/escape_site.rs" <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.get();
        items
    });
}
RUST
: > "$t4/baseline.txt"
run_linter "$t4" "$t4/baseline.txt" --update-baseline
assert_line_count "criterion 4: baseline has one entry before the escape hatch" "$t4/baseline.txt" 1

cat > "$t4/escape_site.rs" <<'RUST'
fn my_component() {
    let filtered = Signal::derive(move || {
        let items = store_signal.get(); // lint-allow: disposal-safe=reads only this component's own scope
        items
    });
}
RUST
# File-scoped (the file is passed explicitly), not full-tree: a plain
# full-tree run here would correctly report this as a STALE baseline entry
# (the debt really did go down, and full-tree mode is where that becomes an
# error — see the header NOTE and the full-tree companion assertion under
# criterion 5 below). File-scoped mode is where "I just fixed this one
# file locally" is expected to pass cleanly.
t4_abs="$(cd "$t4" && pwd)/escape_site.rs"
run_linter "$t4" "$t4/baseline.txt" "$t4_abs"
assert_rc "criterion 4: escape-hatched site passes (file-scoped)" 0 "$BL_RC"
assert_not_contains "criterion 4: escape-hatched site produces no B finding at all" ":B "

run_linter "$t4" "$t4/baseline.txt" --update-baseline
assert_line_count "criterion 4: regenerating drops the escape-hatched site from the baseline" "$t4/baseline.txt" 0

# ─── Criterion 5 (the trap most likely to regress): file-scoped mode must
# NOT report an unscanned file's baseline entry as stale — including the
# adversarial case where that file's underlying content is genuinely gone,
# which looks identical to "fixed" from the baseline's point of view.
t5="$kyo679dir/t5"
mkdir -p "$t5"
cat > "$t5/file_a.rs" <<'RUST'
fn a() {
    let x = Signal::derive(move || sig_a.get());
}
RUST
cat > "$t5/file_b.rs" <<'RUST'
fn b() {
    let x = Signal::derive(move || sig_b.get());
}
RUST
: > "$t5/baseline.txt"
run_linter "$t5" "$t5/baseline.txt" --update-baseline
assert_line_count "criterion 5: baseline covers both files before the scoped run" "$t5/baseline.txt" 2

rm "$t5/file_b.rs"
t5a_abs="$(cd "$t5" && pwd)/file_a.rs"
run_linter "$t5" "$t5/baseline.txt" "$t5a_abs"
assert_rc "criterion 5: file-scoped run over file_a only does not fail on file_b's now-vanished entry" 0 "$BL_RC"
assert_not_contains "criterion 5: no stale-baseline ERROR for the unscanned file" "ERROR:"

# ─── Companion to criterion 5: full-tree mode DOES treat the same situation
# as stale (this is the asymmetry the design deliberately introduces, and
# it needs its own coverage — a full-tree run sees file_b really is gone).
run_linter "$t5" "$t5/baseline.txt"
assert_rc "full-tree companion: a genuinely-fixed/vanished baselined site fails as stale" 1 "$BL_RC"
assert_contains "full-tree companion: mentions --update-baseline" "--update-baseline"

# ─── Criterion 6: --update-baseline combined with file arguments exits 2
# and does not write the baseline (a partial write would silently erase
# every unscanned file's recorded debt).
t6="$kyo679dir/t6"
mkdir -p "$t6"
cat > "$t6/f.rs" <<'RUST'
fn f() {
    let x = Signal::derive(move || sig.get());
}
RUST
printf 'sentinel-line-should-survive-untouched\n' > "$t6/baseline.txt"
cp "$t6/baseline.txt" "$t6/baseline.before"
t6_abs="$(cd "$t6" && pwd)/f.rs"
run_linter "$t6" "$t6/baseline.txt" --update-baseline "$t6_abs"
assert_rc "criterion 6: --update-baseline with file args is a usage error" 2 "$BL_RC"
assert_files_identical "criterion 6: baseline file is untouched" "$t6/baseline.txt" "$t6/baseline.before"

# ─── Criterion 7: adding a duplicate of an already-baselined identical line
# (exceeding the recorded occurrence count) fails, even though the hash
# alone would still match an entry in the baseline.
t7="$kyo679dir/t7"
mkdir -p "$t7"
# Both flagged lines must be textually IDENTICAL (same hash) for this test —
# matching the real crates/kyomi-ui/src/cache/store.rs case this criterion
# is modeled on, where all 6 flagged lines are the literal same text. A
# per-line-unique binding name would hash differently and defeat the point.
cat > "$t7/dup.rs" <<'RUST'
fn f() {
    Signal::derive(move || sig.get());
    Signal::derive(move || sig.get());
}
RUST
: > "$t7/baseline.txt"
run_linter "$t7" "$t7/baseline.txt" --update-baseline
if grep -q ':2$' "$t7/baseline.txt"; then
    printf "  ✓ %s\n" "criterion 7: baseline records count=2 for the duplicated line"
    PASS=$((PASS + 1))
else
    printf "  ✗ %s — baseline contents:\n" "criterion 7: baseline records count=2 for the duplicated line"
    sed 's/^/    | /' "$t7/baseline.txt"
    FAIL=$((FAIL + 1))
fi

run_linter "$t7" "$t7/baseline.txt"
assert_rc "criterion 7: exactly 2 copies matches the baselined count" 0 "$BL_RC"

cat > "$t7/dup.rs" <<'RUST'
fn f() {
    Signal::derive(move || sig.get());
    Signal::derive(move || sig.get());
    Signal::derive(move || sig.get());
}
RUST
run_linter "$t7" "$t7/baseline.txt"
assert_rc "criterion 7: a 3rd copy exceeds the baselined count of 2" 1 "$BL_RC"
assert_contains "criterion 7: 3rd copy reports ERROR:B" "ERROR:B"

echo
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
