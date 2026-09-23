#!/bin/bash
# ------------------------------------------------------------------------------
# scripts/sign-review.sh — signs the staged diff with the reviewer's private
# key, called by the code-review-architect agent after a successful review
# (KYO-712, closing a gap found in the KYO-676 run).
#
# WHY THIS EXISTS
#
# Before this fix, the approval hash was `git diff --cached | sha256sum` —
# taken from the index at signing time, with nothing tying it to the diff a
# human or reviewer agent actually read. On 2026-09-09 (the KYO-676 run) a
# reviewer reviewed 4 staged files. Between its review and its signature, 3
# of them became unstaged (` M` in `git status` — worktree content intact,
# just removed from the index; the mechanism that did this was never
# identified, see below). This script hashed the single remaining staged
# file (a markdown doc) and signed that. `.githooks/pre-commit` verified the
# signature against the (now-narrowed) staged diff, printed
# "✅ Code review signature verified", and the commit went through — with
# none of the reviewed fix in it. Every check was green. The pre-existing
# empty-diff guard below (still present) only catches a *fully* emptied
# index; a partially narrowed one looks like a perfectly ordinary smaller
# review.
#
# THIS IS DELIBERATELY MECHANISM-INDEPENDENT
#
# The ticket that prompted this fix suspected `scripts/lint/check-disposal-
# safety.sh`. Measured and ruled out: staged a `crates/kyomi-ui/**/*.rs`
# file, snapshotted `git status --porcelain`, ran that script, snapshotted
# again — byte-identical, exit 0. It does not touch the index.
# `grep -rnE '\bgit +(add|rm|checkout|restore|stash|reset|apply|read-tree|
# update-index|clean)\b' scripts/ .githooks/` matches only comment and
# echo prose in this tree, never an executed command. On `origin/main`, before
# this change, the only hit was an error message in scripts/setup-hooks.sh
# (`echo "Fix with: chmod +x <hook> && git add <hook>"`); this change adds its
# own prose hits (this header, the new test files, and the echo strings this
# script and .githooks/pre-commit print on failure) but none of them is an
# executed command either. No script in this repo mutates the index. This
# fix was therefore built to refuse to sign a narrowed index
# *regardless of cause*, which is the correct response to a cause that
# could not be named: it catches this failure mode whether it comes from a
# script bug, a human running `git restore --staged` mid-review, an
# editor's git integration, or something not yet seen. That reasoning
# still holds and is the reason this stays mechanism-independent rather
# than, say, only re-checking after the specific command below.
#
# ONE CLASS OF COMMAND THAT PRODUCES THIS EXACT SHAPE HAS SINCE BEEN
# OBSERVED LIVE (KYO-712 Part D)
#
# During this ticket's own code review (2026-09-11), the reviewer itself
# ran `git --work-tree=/tmp/<other-dir> checkout origin/main -- scripts
# .githooks` from inside this worktree without also passing a matching
# `--git-dir`. Because `--git-dir` was left at its default (discovered from
# cwd, i.e. THIS worktree's real gitdir), git updated THIS worktree's index
# to match the checked-out revision for those paths, while writing the
# checked-out file *content* into the unrelated `--work-tree` directory —
# leaving this worktree's own on-disk files untouched. The result, reproduced
# in isolation in a throwaway repo mirroring this exact shape — three files
# staged and "reviewed": two inside the checked-out paths (`scripts/foo.txt`,
# `.githooks/bar.txt`) and one outside them (`docs/baz.txt`) — then one `git
# --work-tree=<other> checkout origin/main -- scripts .githooks` run against
# them with no `--git-dir` override. `git status --porcelain` goes from
# `M  .githooks/bar.txt` / `M  docs/baz.txt` / `M  scripts/foo.txt` to
# ` M .githooks/bar.txt` / `M  docs/baz.txt` / ` M scripts/foo.txt` — the two
# files inside the checked-out paths reset out of the index while their staged,
# reviewed content stays intact on disk; `docs/baz.txt` sits outside both
# checked-out paths, so the checkout never touches it and it stays fully
# staged. For the files the checkout did target, that is byte-for-byte the
# KYO-676 signature (` M`, worktree content intact, just removed from the
# index).
#
# Read this claim exactly as strongly as the evidence supports and no
# further: this reproduces the *class* of command that produces the
# KYO-676 shape, and it was observed live, by accident, during this very
# ticket's own review. It is NOT proof that this specific command caused
# the original 2026-09-09 KYO-676 incident — that incident's actual
# command was never captured, and still isn't. What this reproduction adds
# is a second, independent confirmation that the mechanism-independent
# design above is the right call: a real, previously-unconsidered command
# shape produces this exact failure mode, entirely unprompted, which is
# exactly the kind of cause this fix was built to catch without having to
# name it in advance. It strengthens the case for mechanism-independence;
# it does not replace it.
#
# THE ONE-POSITIONAL-ARGUMENT INTERFACE IS LOAD-BEARING
#
# This script is invoked by the code-review-architect agent, whose prompt
# lives at ~/.claude/agents/code-review-architect.md — untracked, outside
# this repo, and not editable from here. It calls
# `bash scripts/sign-review.sh "<private key>"` with exactly one positional
# argument. That invocation must keep working completely unchanged forever,
# because there is no PR that can update the caller in step with this file.
# Anything this script needs beyond the private key MUST be an optional flag
# appended after $1, never a change to what $1 means or how many positional
# arguments are required. Do not "simplify" this into a differently-shaped
# CLI without first reading this paragraph.
#
# WHAT THIS SCRIPT REFUSES
#
#   - An empty staged diff (pre-existing guard).
#   - Any tracked file with unstaged modifications at signing time — i.e.
#     `git diff --name-only` is non-empty. This is exactly the KYO-676
#     shape: content that was part of what got reviewed no longer being part
#     of what gets signed. Refusing here, before a signature is ever
#     produced, is strictly better than trying to detect the gap later,
#     because after this script runs the diff hash the reviewer approved is
#     permanently disconnected from "what the reviewer read".
#   - Any untracked, non-ignored file at signing time — i.e. `git ls-files
#     --others --exclude-standard` is non-empty (KYO-712 Part A, closing a
#     second gap in the same KYO-676 run: a *newly added* file that gets
#     unstaged does not show up in `git diff --name-only` at all — `A ` in
#     the index becomes `??` once dropped, i.e. untracked, not "modified" —
#     so the check above is blind to exactly this shape. Measured directly:
#     staging three new files, unstaging two, and signing the pre-Part-A
#     script exits 0 and signs the narrowed index anyway. The real KYO-676
#     diff contained a new file; it happened to be the one that stayed
#     staged. Had it been one of the ones dropped, the tracked-modification
#     guard alone would not have caught it.
#
#     `--exclude-standard` here is load-bearing, not incidental — do not
#     delete it while "simplifying" this check. `.review-approval`
#     (`.gitignore:88`), `docs/review-logs/` (via the `docs/*` rule), and
#     `target/` are all gitignored, and the reviewer agent writes its review
#     log immediately before it signs. Without `--exclude-standard` this
#     guard would see those as untracked-and-refuse on every single
#     legitimate signature, bricking the review gate outright.
#
#     This is consistent with, not stricter than, the rest of the review
#     workflow: `/backlog-fast`'s own post-implementation gate already
#     treats a `??` file at commit time as a defect — "work exists that was
#     never `git add`ed and the commit will silently omit it."
#
# ESCAPE HATCH: --allow-unstaged <reason>
#
# Fails closed by default. A human who has deliberately reviewed the
# narrowed set and wants to sign anyway must say so explicitly:
#   scripts/sign-review.sh "<private key>" --allow-unstaged "<reason>"
# The reason is required and non-empty, is echoed to stdout together with
# the offending paths (so the deliberate narrowing is visible in the
# transcript), and is recorded as an optional third line in
# `.review-approval` (`ALLOW-UNSTAGED:<reason>`) so
# `.githooks/pre-commit`'s own unstaged-file check (KYO-712 Part B) can tell
# an acknowledged narrowing from an unacknowledged one. There is
# deliberately no environment-variable equivalent — one documented override
# mechanism, not two. This single flag covers both refusal categories above
# (unstaged tracked modifications and untracked new files) — there is no
# separate flag per category.
#
# THE ALLOW-UNSTAGED REASON IS PART OF WHAT GETS SIGNED (KYO-712 Part C)
#
# A first version of this recorded line 3 as plain, unsigned text: the
# Ed25519 signature covered only the diff hash on line 1, never line 3.
# That meant anyone with local filesystem write access could hand-append
# `ALLOW-UNSTAGED:<any reason>` to an already, legitimately signed
# `.review-approval` — no private key required — and
# `.githooks/pre-commit` would report the narrowing as "acknowledged at
# signing" for a file nobody ever reviewed. This is a security control; it
# must resist a forged file, not just an accidental one. Fixed by binding
# the reason into the signed material: when --allow-unstaged is used, this
# script signs "<DIFF_HASH>\nALLOW-UNSTAGED:<reason>" instead of DIFF_HASH
# alone. When it is not used, it signs DIFF_HASH exactly as before — this
# is what keeps `.review-approval` files written by this script, with no
# line 3, verifiable by an older or newer copy of `.githooks/pre-commit`
# alike. `.githooks/pre-commit`'s review_approval_signature_valid()
# reconstructs the same combined value from whatever `.review-approval`
# actually contains and verifies against that, so a hand-forged line 3 —
# appended after signing, never part of what was actually signed — fails
# verification and blocks the commit. See that function's own header
# comment for the verification side of this.
#
# `.review-approval`'s first two lines (hash, signature) are unchanged in
# format — `.githooks/pre-commit` reads them with `sed -n '1p'`/`'2p'` and
# must keep working against approvals written by an older copy of this
# script. Any further lines are additive and must stay optional to read.
#
# Self-test: scripts/sign-review-test.sh.
#
# KYO-779: a rebase can rewrite already committed work with no staged diff.
# The explicit --committed-range mode signs a separate approval containing
# the base and HEAD object IDs plus the net diff hash. Its public-key-only
# verifier rejects an old approval after either object changes. This file is
# deliberately separate from .review-approval, which pre-commit interprets
# only as a staged-diff approval.
# ------------------------------------------------------------------------------

set -e
set -o pipefail

# A committed-range approval is separate from .review-approval: the latter
# belongs to pre-commit and must continue to describe the staged diff only.
# `--verify-committed-range` needs no private key and is run immediately before
# a rebase-only push. The public key matches .githooks/pre-commit.
REVIEW_PUBLIC_KEY="-----BEGIN PUBLIC KEY-----
MCowBQYDK2VwAyEApRtsZODQxaUNP383HB/iqLHSlrf92Fe3UB43Bc2TaG0=
-----END PUBLIC KEY-----"
RANGE_APPROVAL=.review-range-approval
RANGE_FORMAT=COMMITTED-RANGE-V1

range_state() {
    local base_ref="$1" base head merge_base staged unstaged untracked
    base="$(git rev-parse --verify "${base_ref}^{commit}")" || return 1
    head="$(git rev-parse --verify 'HEAD^{commit}')" || return 1
    merge_base="$(git merge-base "$base" "$head")" || return 1
    if [ "$base" != "$merge_base" ] || [ "$base" = "$head" ]; then
        echo "ERROR: committed range must have a non-empty branch diff based on $base_ref." >&2
        return 1
    fi
    # No staged or working-tree content may be mistaken for reviewed commits.
    staged="$(git diff --cached --name-only)" || return 1
    unstaged="$(git diff --name-only)" || return 1
    untracked="$(git ls-files --others --exclude-standard)" || return 1
    if [ -n "$staged$unstaged$untracked" ]; then
        echo "ERROR: committed-range review requires a clean index and working tree." >&2
        return 1
    fi
    local diff_hash
    diff_hash="$(git diff "$base" "$head" | sha256sum | awk '{print $1}')" || return 1
    if [ "$diff_hash" = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" ]; then
        echo "ERROR: committed range has no content change to review." >&2
        return 1
    fi
    printf '%s\n%s\n%s\n' "$base" "$head" "$diff_hash"
}

if [ "${1:-}" = "--verify-committed-range" ]; then
    if [ "$#" -ne 2 ]; then
        echo "Usage: scripts/sign-review.sh --verify-committed-range <base-ref>" >&2
        exit 1
    fi
    [ -f "$RANGE_APPROVAL" ] || { echo "ERROR: no committed-range approval found." >&2; exit 1; }
    state="$(range_state "$2")" || exit 1
    expected="$(printf '%s\n%s' "$RANGE_FORMAT" "$state")"
    actual="$(sed -n '1,4p' "$RANGE_APPROVAL")"
    if [ "$actual" != "$expected" ] || [ "$(wc -l < "$RANGE_APPROVAL")" -ne 5 ]; then
        echo "ERROR: committed-range approval is stale or malformed; request a fresh review." >&2
        exit 1
    fi
    pub_file="$(mktemp)"; sig_file="$(mktemp)"; val_file="$(mktemp)"
    trap 'rm -f "$pub_file" "$sig_file" "$val_file"' EXIT
    printf '%s\n' "$REVIEW_PUBLIC_KEY" > "$pub_file"
    printf '%s' "$actual" > "$val_file"
    if ! sed -n '5p' "$RANGE_APPROVAL" | base64 -d > "$sig_file" 2>/dev/null \
        || ! openssl pkeyutl -verify -rawin -pubin -inkey "$pub_file" -in "$val_file" -sigfile "$sig_file" >/dev/null 2>&1; then
        echo "ERROR: committed-range review signature is invalid." >&2
        exit 1
    fi
    echo "Committed-range review signature verified."
    exit 0
fi

PRIVATE_KEY="${1:-}"

if [ -z "$PRIVATE_KEY" ]; then
    echo "ERROR: Private key argument required." >&2
    echo "Usage: scripts/sign-review.sh <private_key_pem_string> [--allow-unstaged <reason> | --committed-range <base-ref>]" >&2
    exit 1
fi
shift

ALLOW_UNSTAGED=0
ALLOW_UNSTAGED_REASON=""
RANGE_BASE_REF=""

while [ $# -gt 0 ]; do
    case "$1" in
        --allow-unstaged)
            if [ $# -lt 2 ] || [ -z "$2" ]; then
                echo "ERROR: --allow-unstaged requires a non-empty reason." >&2
                exit 1
            fi
            case "$2" in
                *$'\n'*)
                    echo "ERROR: --allow-unstaged reason must not contain a newline (.review-approval is line-based)." >&2
                    exit 1
                    ;;
            esac
            ALLOW_UNSTAGED=1
            ALLOW_UNSTAGED_REASON="$2"
            shift 2
            ;;
        --committed-range)
            if [ $# -lt 2 ] || [ -z "$2" ] || [ -n "$RANGE_BASE_REF" ]; then
                echo "ERROR: --committed-range requires one base ref." >&2
                exit 1
            fi
            RANGE_BASE_REF="$2"
            shift 2
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            echo "Usage: scripts/sign-review.sh <private_key_pem_string> [--allow-unstaged <reason> | --committed-range <base-ref>]" >&2
            exit 1
            ;;
    esac
done

if [ -n "$RANGE_BASE_REF" ]; then
    if [ "$ALLOW_UNSTAGED" -eq 1 ]; then
        echo "ERROR: --allow-unstaged cannot be used with --committed-range." >&2
        exit 1
    fi
    state="$(range_state "$RANGE_BASE_REF")" || exit 1
    signed_value="$(printf '%s\n%s' "$RANGE_FORMAT" "$state")"
    key_file="$(mktemp)"; val_file="$(mktemp)"
    trap 'rm -f "$key_file" "$val_file"' EXIT
    printf '%s\n' "$PRIVATE_KEY" > "$key_file"
    printf '%s' "$signed_value" > "$val_file"
    signature="$(openssl pkeyutl -sign -rawin -inkey "$key_file" -in "$val_file" | base64 -w 0)" || exit 1
    [ -n "$signature" ] || { echo "ERROR: committed-range signing failed." >&2; exit 1; }
    printf '%s\n%s\n' "$signed_value" "$signature" > "$RANGE_APPROVAL"
    echo "Committed range signed for $RANGE_BASE_REF."
    exit 0
fi

# Write private key to temp file (Ed25519 PEM format)
KEY_FILE=$(mktemp)
HASH_FILE=$(mktemp)
UNSTAGED_FILE=$(mktemp)
UNTRACKED_FILE=$(mktemp)
trap 'rm -f "$KEY_FILE" "$HASH_FILE" "$UNSTAGED_FILE" "$UNTRACKED_FILE"' EXIT
echo "$PRIVATE_KEY" > "$KEY_FILE"

# --- Refuse to sign a narrowed index (KYO-712 / KYO-676) ---
#
# Captured to a real file with its exit status checked directly, NOT via a
# process substitution — a process substitution's own exit status is never
# seen by the parent shell (even under `set -e`), so a failing `git diff`
# would otherwise silently look like "no unstaged files" instead of an
# error (docs/standards/error-handling/empty-on-failure-must-not-look-like-
# a-real-result.md). -z / NUL-delimited so filenames containing spaces or
# newlines are handled correctly.
if ! git diff --name-only -z > "$UNSTAGED_FILE"; then
    echo "ERROR: 'git diff --name-only' failed — cannot verify the index is not narrowed." >&2
    exit 1
fi

unstaged_files=()
while IFS= read -r -d '' f; do
    unstaged_files+=("$f")
done < "$UNSTAGED_FILE"

# Same discipline for untracked, non-ignored files (KYO-712 Part A) — a
# newly added file that gets unstaged leaves the tracked-diff check above
# blind (it's `??`, not modified). --exclude-standard is load-bearing: see
# the header's WHAT THIS SCRIPT REFUSES section before touching this line.
if ! git ls-files --others --exclude-standard -z > "$UNTRACKED_FILE"; then
    echo "ERROR: 'git ls-files --others --exclude-standard' failed — cannot verify no untracked files are present." >&2
    exit 1
fi

untracked_files=()
while IFS= read -r -d '' f; do
    untracked_files+=("$f")
done < "$UNTRACKED_FILE"

if [ "${#unstaged_files[@]}" -gt 0 ] || [ "${#untracked_files[@]}" -gt 0 ]; then
    if [ "$ALLOW_UNSTAGED" -eq 1 ]; then
        echo "⚠️  Signing with unstaged/untracked changes present (--allow-unstaged)."
        echo "   Reason: $ALLOW_UNSTAGED_REASON"
        if [ "${#unstaged_files[@]}" -gt 0 ]; then
            echo "   Modified tracked files with unstaged changes (not covered by the diff hash being signed):"
            printf '     %s\n' "${unstaged_files[@]}"
        fi
        if [ "${#untracked_files[@]}" -gt 0 ]; then
            echo "   New files that were never staged (untracked, non-ignored — not covered by the diff hash being signed):"
            printf '     %s\n' "${untracked_files[@]}"
        fi
    else
        echo "" >&2
        echo "ERROR: refusing to sign — the working tree has changes not covered by the staged diff:" >&2
        if [ "${#unstaged_files[@]}" -gt 0 ]; then
            echo "" >&2
            echo "  Modified tracked files with unstaged changes:" >&2
            printf '    %s\n' "${unstaged_files[@]}" >&2
        fi
        if [ "${#untracked_files[@]}" -gt 0 ]; then
            echo "" >&2
            echo "  New files that were never staged (untracked, non-ignored):" >&2
            printf '    %s\n' "${untracked_files[@]}" >&2
        fi
        echo "" >&2
        echo "The staged diff about to be signed is not necessarily the whole change" >&2
        echo "that was reviewed. This is the KYO-676 failure mode: a file leaves the" >&2
        echo "index between review and signing, and the signature ends up covering only" >&2
        echo "what's left — silently, with every downstream check still green. A newly" >&2
        echo "added file that gets unstaged goes further: it disappears from" >&2
        echo "'git diff --name-only' entirely (it becomes untracked, not modified), so" >&2
        echo "checking tracked-file diffs alone misses it (KYO-712 Part A)." >&2
        echo "" >&2
        echo "Fix: git add the modified and/or new files listed above so the index" >&2
        echo "matches what was reviewed, then get a fresh review. If the narrowing is" >&2
        echo "deliberate and has actually been reviewed as such, re-run with:" >&2
        echo "--allow-unstaged \"<reason>\"" >&2
        exit 1
    fi
fi

# Compute sha256 of the staged diff
DIFF_HASH=$(git diff --cached | sha256sum | awk '{print $1}')

if [ -z "$DIFF_HASH" ] || [ "$DIFF_HASH" = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" ]; then
    echo "ERROR: No staged changes to sign." >&2
    exit 1
fi

# Sign the hash with Ed25519 (must use -in file, not stdin — Ed25519 requires
# it). KYO-712 Part C: when --allow-unstaged was used, sign the hash
# together with the ALLOW-UNSTAGED reason ("<hash>\nALLOW-UNSTAGED:<reason>")
# rather than the hash alone, so .githooks/pre-commit's
# review_approval_signature_valid() can detect a line 3 that was appended
# after signing rather than part of what was actually signed. Without
# --allow-unstaged, this signs DIFF_HASH exactly as before — unchanged, for
# compatibility with .githooks/pre-commit verifying approvals that never
# have a line 3.
#
if [ "$ALLOW_UNSTAGED" -eq 1 ]; then
    printf '%s\nALLOW-UNSTAGED:%s' "$DIFF_HASH" "$ALLOW_UNSTAGED_REASON" > "$HASH_FILE"
else
    echo -n "$DIFF_HASH" > "$HASH_FILE"
fi
# -rawin is REQUIRED, not optional decoration: `pkeyutl -sign`/`-verify` for
# an Ed25519 key fails outright on OpenSSL 3.0.x (Ubuntu 24.04, which is
# what ubuntu-latest is at the time of writing — the CI runner this script's
# own test suite runs on) without it — "evp_pkey_signature_init: operation
# not supported for this keytype" — and only starts working flagless on
# OpenSSL 3.2+. Measured directly: OpenSSL 3.5.5 signs/verifies fine either
# way; 3.0.13 only works with -rawin; 1.1.1 fails both with and without it
# (pre-existing, not a regression -rawin causes). Signatures produced with
# and without -rawin are byte-identical and cross-verify in both
# directions, so this is not a key rotation or a format change — do not
# remove it as "redundant" on a box where the flagless form happens to work.
SIGNATURE=$(openssl pkeyutl -sign -rawin -inkey "$KEY_FILE" -in "$HASH_FILE" | base64 -w 0)

if [ -z "$SIGNATURE" ]; then
    echo "ERROR: Signing failed — check private key format." >&2
    exit 1
fi

# Write approval file. Lines 1-2 (hash, signature) are a fixed format read
# by .githooks/pre-commit via `sed -n '1p'`/`'2p'` — never reorder or remove
# them. Line 3 is optional and additive.
{
    printf '%s\n' "$DIFF_HASH"
    printf '%s\n' "$SIGNATURE"
    if [ "$ALLOW_UNSTAGED" -eq 1 ]; then
        printf 'ALLOW-UNSTAGED:%s\n' "$ALLOW_UNSTAGED_REASON"
    fi
} > .review-approval

echo "Review approval signed for diff hash: ${DIFF_HASH}"
