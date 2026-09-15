#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/preflight-clippy.sh — run CI's three clippy passes locally, with
# byte-identical flags (KYO-629)
#
# WHY THIS EXISTS
#
# .github/workflows/ci.yml's `clippy` job runs THREE separate `cargo clippy`
# invocations ("cargo clippy (workspace, excluding desktop)", "cargo clippy
# (all targets, incl. tests)", and "cargo clippy (kyomi-ui, wasm32)"). The
# /backlog-fast and /backlog skills document a pre-PR gate that only
# approximates the FIRST of the three, so every lint that lives inside a
# `#[cfg(test)]` module — exactly what the second pass alone catches — is
# invisible to that gate and is only discovered after CI runs. Confirmed
# instance: PR #480 added tests to
# crates/kyomi-auth/src/datasource_service.rs and CI failed with three
# clippy::cloned_ref_to_slice_refs errors the documented gate had passed
# clean over.
#
# This script is the single, tested, executable place these three flag
# strings live, so the skill documents (and any other caller) invoke it
# instead of retyping the flags — the same consolidation KYO-422 applied to
# scripts/check-ticket-in-flight.sh. Self-test: scripts/preflight-clippy-test.sh,
# whose most important assertion parses the three `run: cargo clippy` lines
# back out of ci.yml itself and diffs them against what this script actually
# runs, so the two cannot silently drift apart again without a test noticing.
#
# `-A clippy::unwrap_used` ON PASS 2 IS LOAD-BEARING, NOT COSMETIC
#
# This repo's root `Cargo.toml` sets `unwrap_used = "warn"` in its
# `[workspace.lints.clippy]` table, workspace-wide, and there is no
# `clippy.toml` in this repo at all (verified by hand: `ls clippy.toml`
# finds nothing), so there is no `allow-unwrap-in-tests` escape hatch either.
# `-D warnings` promotes every `warn`-level lint to a hard error, and
# `--all-targets` is what makes test code visible to clippy in the first
# place — so without `-A clippy::unwrap_used`, pass 2 fails on every
# legitimate `.unwrap()` a test author ever wrote. ci.yml's own comment above
# that step says as much: "a panic on unwrap() in a test IS the assertion".
# Do not drop this flag to "tighten" the gate — it does not make anything
# safer, it just makes the all-targets pass permanently red on pre-existing,
# correct test code.
#
# WHY THE MOLD-LINKER STRIP IS DELIBERATELY NOT REPLICATED HERE
#
# ci.yml's clippy job has a "Strip local-dev mold linker config" step
# (`sed -i '/# BEGIN_LOCAL_DEV_LINKER/,/# END_LOCAL_DEV_LINKER/d'
# .cargo/config.toml`) immediately before its clippy steps. That step exists
# ONLY because GitHub's hosted runners don't have mold installed, so leaving
# that block in place would break every build on CI. Locally the opposite is
# true: mold IS installed and IS wanted — that is the entire reason the
# BEGIN_LOCAL_DEV_LINKER/END_LOCAL_DEV_LINKER block exists in the checked-in
# `.cargo/config.toml` in the first place. Do NOT add an equivalent stripping
# step here "for parity" — that would slow down (or break) every subsequent
# build in this worktree to work around a problem that only exists on CI's
# runners.
#
# NARROWING (-p) IS FOR SPEED, NEVER FOR WEAKER COVERAGE
#
# Passing one or more `-p <crate>` replaces `--workspace --exclude
# kyomi-desktop` on passes 1 and 2 with the given `-p` flags — every other
# flag on those two passes (--locked, --all-targets, -D warnings, -A
# clippy::unwrap_used) carries over unchanged. Pass 3 is inherently
# kyomi-ui/wasm32-specific (CI's own invocation hardcodes `-p kyomi-ui`), so
# it runs whenever kyomi-ui is among the narrowed crates, or whenever -p is
# omitted entirely (the full, unnarrowed run). When narrowing excludes
# kyomi-ui, pass 3 is skipped — and the summary at the end says so by name,
# rather than quietly reporting a clean run when only two of three passes
# actually executed.
#
# WHY NARROWED SCOPE DIVERGES FROM CI — WORKSPACE FEATURE UNIFICATION
# (KYO-723 — DO NOT "SIMPLIFY" THIS BACK TO A BARE -p RUN)
#
# `-p <crate>` alone is not equivalent to "the same crate as it builds inside
# CI's workspace pass." CI's pass 1/2 build `--workspace --exclude
# kyomi-desktop`, which is `apps/server` and everything it depends on, all at
# once. Cargo's feature unification means every crate in THAT graph is built
# with the union of features any workspace member asked it to have — so
# `apps/server`'s `kyomi-ui = { workspace = true, features = ["ssr"] }`
# (apps/server/Cargo.toml:20) turns `kyomi-ui`'s `ssr` feature on for the
# WHOLE build, including the `-p kyomi-ui` pass, because it's the same
# `cargo` invocation. A bare `cargo clippy -p kyomi-ui` is a different,
# smaller graph that never includes `apps/server`, so `ssr` is off by
# default, `#[cfg(all(test, feature = "ssr"))] mod tests;` in
# crates/kyomi-ui/src/pages/settings/datasources.rs never compiles, and
# `credential_status_indicates_connected` — a function that function's own
# `#[cfg(test)]` module is the sole caller of — genuinely has no caller in
# THAT narrower graph. clippy correctly reports `dead_code` for the graph it
# was actually given; the graph was just never the one CI (or the shipping
# binary) builds. See
# docs/standards/build-toolchain/a-narrowed-clippy-run-can-report-dead-code-ci-will-never-see.md
# for the false-positive shape this produces and its cost in repeated review
# re-derivation, and
# docs/standards/build-toolchain/build-a-guard-with-the-features-the-artifact-ships-with.md
# for the general principle: a verification command's feature set is a claim
# about which source it compiled, and that claim has to be derived from the
# manifest that actually ships, not typed by hand at the call site.
#
# The fix here is exactly that derivation, made mechanical:
# derive_unified_features() (below) shells out to
# `cargo metadata --locked --format-version 1` and reads
# `.resolve.nodes[].features` for each `-p` crate — the JSON field that
# IS cargo's own record of what it unified the whole-workspace build's
# feature set to, for that crate, on this lockfile. That is deliberately not
# a hardcoded `if crate == kyomi-ui: add ssr` rule: `apps/server` also pulls
# in `kyomi-ui`'s `slack` feature (via `kyomi-ui/slack` behind
# `apps/server`'s own `slack` default feature), which a hand-written rule
# would plausibly miss, and any such rule silently goes stale the next time a
# Cargo.toml `features = [...]` list changes, since nothing would fail to
# remind the next editor to update it. Deriving from `cargo metadata` cannot
# drift from ci.yml's own resolution the way a second, hand-maintained list
# necessarily would — there is exactly one place cargo's feature unification
# is computed, and this script reads it rather than re-implementing it. DO
# NOT replace this derivation with a hardcoded per-crate feature table "to
# avoid needing jq" — that reintroduces the exact silent-drift failure mode
# this ticket exists to close.
#
# Known limitation, stated rather than hidden: `cargo metadata`'s resolve
# graph is computed over the ENTIRE workspace, including kyomi-desktop,
# whereas CI's passes explicitly `--exclude kyomi-desktop`. For `kyomi-ui`
# specifically this makes no difference today — apps/desktop/Cargo.toml
# depends on `kyomi-server` with `default-features = false` and does not
# depend on `kyomi-ui` at all, checked by hand at KYO-723 time — but for some
# other crate, in principle, a feature kyomi-desktop alone turns on could get
# included here when CI would never turn it on. That is the safe direction
# to be wrong in: it can only make this script's gate STRICTER than CI
# (lint more code, never less), never weaker, so a mismatch here fails loud
# as an unexpected local finding rather than shipping a quietly-weaker green.
#
# FAIL CLOSED ON A PASS THAT COULD NOT RUN
#
# If `wasm32-unknown-unknown` is not installed, pass 3 cannot run at all —
# and "could not run" must never look like "ran and passed". This script
# checks `rustup target list --installed` before attempting pass 3 and, if
# the target is missing, exits 3 (never 0) naming the fix (`rustup target
# add wasm32-unknown-unknown`). The same convention applies to narrowed
# feature derivation (KYO-723): if `jq` is missing, `cargo metadata` fails,
# or a named crate has no unique match in its output, this script exits 3
# rather than falling through to a run with no `--features` override —
# falling through silently would recreate, on every run, precisely the false
# `dead_code` this ticket exists to eliminate, which is a worse outcome than
# refusing to run at all. This mirrors scripts/check-ticket-in-flight.sh's
# own FAIL CLOSED convention: exit 3 (a check that could not complete) is
# reported ahead of exit 1 (lints found), and a caller must treat both as
# "not clean" — the whole point of this ticket is that a gate whose green is
# quietly weaker than it looks is the expensive failure, so a skipped pass
# must never look like a passed one.
#
# RUNS EVERY PASS EVEN AFTER AN EARLIER ONE FAILS
#
# A cold clippy run is expensive (tens of minutes for the unnarrowed
# workspace pass), so one invocation of this script should surface every
# problem it can find rather than stopping at the first. Modeled on the
# worktree-lifecycle-selftests job's own suite loop in ci.yml, which
# collects every failing suite before exiting non-zero, for the same reason.
#
# USAGE
#
#   preflight-clippy.sh [-p <crate>]... [--help]
#
# EXIT CODES
#
#   0 — every pass that ran was clean (a pass skipped by explicit -p
#       narrowing — see NARROWING above — is not a failure)
#   1 — at least one pass reported lints
#   2 — usage error (unknown flag, -p given with no value)
#   3 — a pass could not be run at all — wasm32-unknown-unknown missing, OR
#       (KYO-723) -p narrowing's unified feature set could not be derived
#       (jq missing, `cargo metadata` failed, or a named crate has no
#       unique match) — never conflated with 0, and reported ahead of 1 the
#       same way check-ticket-in-flight.sh reports its FAILURES ahead of
#       its HITS
#
# Bash + cargo + rustup + jq. jq is a new dependency as of KYO-723, used
# only to derive -p narrowing's unified `--features` set from `cargo
# metadata` (see WHY NARROWED SCOPE DIVERGES FROM CI above) — the
# unnarrowed run stays jq-free and byte-identical to CI's own invocations.
# jq ships preinstalled on GitHub's hosted ubuntu runners (the
# actions/runner-images default toolset) and was already present on the
# machine this fix was written and verified on. For the record, since an
# earlier draft of this comment claimed otherwise without checking:
# scripts/check-ticket-in-flight.sh and scripts/mark-branch-stranded.sh are
# NOT prior users of the standalone jq binary — both use `gh`'s own bundled
# `--jq` evaluator (see check-ticket-in-flight.sh's own header, "No jq
# binary … gh bundles its own jq evaluator") and remain jq-free themselves.
# This script is the first in scripts/ to depend on the real jq binary; no
# other new dependency, still no network beyond what cargo itself needs to
# compile. Self-test: scripts/preflight-clippy-test.sh.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_NAME="$(basename "$0")"

usage() {
    cat >&2 <<EOF
Usage: $SCRIPT_NAME [-p <crate>]... [--help]

Runs the same three \`cargo clippy\` invocations as ci.yml's \`clippy\` job, in
the same order, with the same flags.

  -p <crate>   Narrow passes 1 and 2 to this crate instead of the full
               workspace (repeatable). Every other flag on those two passes
               is unchanged, EXCEPT that narrowing also adds a --features
               flag derived from \`cargo metadata\` (KYO-723): the crate's
               unified feature set from a whole-workspace build, so a
               feature only turned on via another workspace member's
               dependency declaration (e.g. apps/server enabling kyomi-ui's
               \`ssr\` and \`slack\`) is still reflected, and the narrowed
               run cannot report dead_code CI would never see. Pass 3
               (kyomi-ui / wasm32) still runs when kyomi-ui is among the
               given crates, or when -p is omitted entirely (the full run);
               otherwise it is skipped, and the summary says so explicitly.
  --help       Show this help and exit 0.

Exit codes:
  0  every pass that ran was clean
  1  at least one pass reported lints
  2  usage error
  3  a pass could not be run at all — e.g. wasm32-unknown-unknown is not
     installed, or (with -p) the unified feature set could not be derived
     from \`cargo metadata\` (jq missing, cargo metadata failed, or a named
     crate has no unique match) — never reported as 0
EOF
}

# derive_unified_features <crate>... — prints, on stdout, a single sorted,
# comma-joined, package-qualified --features value (e.g.
# "kyomi-ui/slack,kyomi-ui/ssr") covering every non-default feature `cargo
# metadata` reports as unified for EACH given crate — see WHY NARROWED SCOPE
# DIVERGES FROM CI above for what "unified" means and why it matters. Prints
# nothing at all (not even a blank line) when every named crate resolves to
# `default` alone, so the caller can test for emptiness and add no
# `--features` flag. Never falls through silently: exits 3 (see FAIL CLOSED
# above) the moment jq is missing, `cargo metadata` fails, or a named crate
# has no unique match — each caught and reported individually, per
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md,
# rather than let a lookup failure degrade to "no extra features" and be
# mistaken for a real answer. Called only when -p narrowing is in effect;
# the unnarrowed run never invokes this function and never shells out to
# `cargo metadata` or `jq` at all.
derive_unified_features() {
    if ! command -v jq >/dev/null 2>&1; then
        echo "ERROR: jq is required to derive the unified --features set for -p narrowing (KYO-723)." >&2
        echo "       Install jq, or run without -p (the unnarrowed run needs no derivation)." >&2
        exit 3
    fi

    local metadata_json
    if ! metadata_json="$(cargo metadata --locked --format-version 1)"; then
        echo "ERROR: cargo metadata --locked --format-version 1 failed; cannot derive the" >&2
        echo "       unified --features set that -p narrowing needs. See cargo's error above." >&2
        exit 3
    fi

    local crate pkgid resolve_count feat_output feat
    declare -a all_features=()

    for crate in "$@"; do
        if ! pkgid="$(jq -r --arg name "$crate" \
            '[.packages[] | select(.name == $name) | .id] as $ids | if ($ids | length) == 1 then $ids[0] else "" end' \
            <<<"$metadata_json")"; then
            echo "ERROR: jq failed looking up package '$crate' in cargo metadata output." >&2
            exit 3
        fi
        if [ -z "$pkgid" ]; then
            echo "ERROR: cargo metadata has no unique package named '$crate' — check that -p '$crate' names a real, uniquely-named workspace member." >&2
            exit 3
        fi

        if ! resolve_count="$(jq -r --arg id "$pkgid" '[.resolve.nodes[] | select(.id == $id)] | length' <<<"$metadata_json")"; then
            echo "ERROR: jq failed looking up the resolve node for '$crate' (package id '$pkgid')." >&2
            exit 3
        fi
        if [ "$resolve_count" -ne 1 ]; then
            echo "ERROR: cargo metadata's resolve graph has no unique node for '$crate' (package id '$pkgid', found $resolve_count) — cannot derive its unified feature set." >&2
            exit 3
        fi

        if ! feat_output="$(jq -r --arg id "$pkgid" '.resolve.nodes[] | select(.id == $id) | .features[] | select(. != "default")' <<<"$metadata_json")"; then
            echo "ERROR: jq failed extracting features for '$crate' (package id '$pkgid') from cargo metadata output." >&2
            exit 3
        fi
        if [ -n "$feat_output" ]; then
            while IFS= read -r feat; do
                all_features+=("${crate}/${feat}")
            done <<<"$feat_output"
        fi
    done

    if [ "${#all_features[@]}" -eq 0 ]; then
        return 0
    fi

    local sorted_str
    sorted_str="$(printf '%s\n' "${all_features[@]}" | sort -u)" \
        || { echo "ERROR: sort failed while deriving the unified --features set." >&2; exit 3; }
    local -a sorted_features
    mapfile -t sorted_features <<<"$sorted_str"
    local IFS=,
    echo "${sorted_features[*]}"
}

declare -a NARROW_CRATES=()

while [ "$#" -gt 0 ]; do
    case "$1" in
        -p)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: -p requires a value" >&2
                exit 2
            fi
            NARROW_CRATES+=("$2")
            shift 2
            ;;
        --help | -h)
            usage
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            usage
            exit 2
            ;;
    esac
done

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

# CI's "Ensure kyomi-ui/dist/ exists for RustEmbed" step. RustEmbed's derive
# macro in apps/server/src/leptos_frontend.rs points at crates/kyomi-ui/dist/;
# if it's missing, the macro silently produces a struct with no `get()`
# method, and clippy fails with an unrelated-looking E0599 instead of any
# real lint. An empty directory is enough — no real assets are needed to lint.
mkdir -p crates/kyomi-ui/dist

declare -a SCOPE_ARGS=()
if [ "${#NARROW_CRATES[@]}" -eq 0 ]; then
    SCOPE_ARGS=(--workspace --exclude kyomi-desktop)
else
    for crate in "${NARROW_CRATES[@]}"; do
        SCOPE_ARGS+=(-p "$crate")
    done
fi

# KYO-723: only -p narrowing needs a derived --features override — the
# unnarrowed run already matches CI's own unification byte-for-byte, so it
# is left with an empty FEATURE_ARGS and never shells out to `cargo
# metadata` or `jq` at all (see derive_unified_features above).
declare -a FEATURE_ARGS=()
if [ "${#NARROW_CRATES[@]}" -gt 0 ]; then
    if ! DERIVED_FEATURES="$(derive_unified_features "${NARROW_CRATES[@]}")"; then
        exit 3
    fi
    if [ -n "$DERIVED_FEATURES" ]; then
        FEATURE_ARGS=(--features "$DERIVED_FEATURES")
    fi
fi

RUN_PASS3=1
if [ "${#NARROW_CRATES[@]}" -gt 0 ]; then
    RUN_PASS3=0
    for crate in "${NARROW_CRATES[@]}"; do
        if [ "$crate" = "kyomi-ui" ]; then
            RUN_PASS3=1
            break
        fi
    done
fi

declare -a FAILED_PASSES=()
declare -a SKIPPED_PASSES=()
COULD_NOT_RUN=0

# run_pass <label> <argv...> — runs one cargo clippy invocation, records the
# label in FAILED_PASSES on a nonzero exit, and otherwise reports clean.
# Never aborts the script: the `if "$@"; then` form is exempt from `set -e`
# the same way `if cmd; then` always is, which is what lets every pass run
# even after an earlier one fails (see RUNS EVERY PASS above).
run_pass() {
    local label="$1"
    shift
    echo "==> ${label}"
    echo "    + $*"
    if "$@"; then
        echo "    clean"
    else
        echo "    LINTS FOUND"
        FAILED_PASSES+=("$label")
    fi
    echo
}

run_pass "pass 1 (workspace, excluding desktop)" \
    cargo clippy --locked "${SCOPE_ARGS[@]}" "${FEATURE_ARGS[@]}" -- -D warnings

run_pass "pass 2 (all targets, incl. tests)" \
    cargo clippy --locked "${SCOPE_ARGS[@]}" "${FEATURE_ARGS[@]}" --all-targets -- -D warnings -A clippy::unwrap_used

if [ "$RUN_PASS3" -eq 1 ]; then
    wasm32_installed=0
    if command -v rustup >/dev/null 2>&1; then
        if rustup target list --installed 2>/dev/null | grep -qx "wasm32-unknown-unknown"; then
            wasm32_installed=1
        fi
    fi

    if [ "$wasm32_installed" -eq 1 ]; then
        run_pass "pass 3 (kyomi-ui, wasm32)" \
            cargo clippy --locked -p kyomi-ui --target wasm32-unknown-unknown --features hydrate -- -D warnings
    else
        echo "==> pass 3 (kyomi-ui, wasm32)"
        echo "    COULD NOT RUN: wasm32-unknown-unknown is not installed."
        echo "    Run: rustup target add wasm32-unknown-unknown"
        echo
        COULD_NOT_RUN=1
    fi
else
    SKIPPED_PASSES+=("pass 3 (kyomi-ui, wasm32) — skipped: -p narrowing did not include kyomi-ui")
fi

echo "===================================================================="
echo "SUMMARY"
for skipped in "${SKIPPED_PASSES[@]}"; do
    echo "  SKIPPED: $skipped"
done

if [ "$COULD_NOT_RUN" -eq 1 ]; then
    echo "RESULT: COULD NOT COMPLETE ALL PASSES — do not treat this as clean."
    exit 3
fi

if [ "${#FAILED_PASSES[@]}" -gt 0 ]; then
    echo "RESULT: LINTS FOUND in: ${FAILED_PASSES[*]}"
    exit 1
fi

echo "RESULT: CLEAN — every pass that ran reported no lints."
exit 0
