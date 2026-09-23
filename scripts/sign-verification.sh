#!/bin/bash
# sign-verification.sh — Called by the test-verification-architect agent after verifying a PR.
# Signs the PR's head commit SHA + ticket ID with the verifier's private key.
# Usage: scripts/sign-verification.sh <pr_number> <private_key_pem_string>

set -e

PR_NUMBER="$1"
PRIVATE_KEY="$2"

if [ -z "$PR_NUMBER" ] || [ -z "$PRIVATE_KEY" ]; then
    echo "ERROR: Usage: sign-verification.sh <pr_number> <private_key_pem_string>" >&2
    exit 1
fi

# Get the PR's current head SHA — this is what we're signing.
# If the PR gets new commits after signing, the signature will not match and merge is blocked.
HEAD_SHA=$(gh pr view "$PR_NUMBER" --json headRefOid -q .headRefOid 2>/dev/null || true)

if [ -z "$HEAD_SHA" ]; then
    echo "ERROR: Could not resolve PR #${PR_NUMBER} head SHA. Is the PR open?" >&2
    exit 1
fi

# Write private key to temp file
KEY_FILE=$(mktemp)
SHA_FILE=$(mktemp)
OPENSSL_ERR_FILE=$(mktemp)
trap 'rm -f "$KEY_FILE" "$SHA_FILE" "$OPENSSL_ERR_FILE"' EXIT
echo "$PRIVATE_KEY" > "$KEY_FILE"
echo -n "$HEAD_SHA" > "$SHA_FILE"

# Sign the SHA with Ed25519.
#
# -rawin is REQUIRED, not optional decoration: `pkeyutl -sign` for an
# Ed25519 key fails outright on OpenSSL 3.0.x (Ubuntu 24.04, i.e.
# ubuntu-latest, which is what this script's own test suite,
# scripts/sign-verification-test.sh, runs on in CI) without it —
# "evp_pkey_signature_init: operation not supported for this keytype" —
# and only starts working flagless on OpenSSL 3.2+. KYO-712 hit the
# identical failure in scripts/sign-review.sh (the code-review approval
# signer) and ~/.local/bin/gh's own verify call; see that script's comment
# on its own `pkeyutl -sign` line for the measured version matrix
# (3.5.5 works either way, 3.0.13 only with -rawin, 1.1.1 fails both —
# pre-existing, not a regression -rawin causes). Signatures produced with
# and without -rawin are byte-identical and cross-verify in both
# directions, so this is not a key rotation or a format change — do not
# remove it as "redundant" on a box where the flagless form happens to
# work. ~/.local/bin/gh's verify call must carry the same flag (that file
# lives outside this repo and is not part of this change — see this
# ticket's PR body for the one-line edit it still needs).
SIGNATURE=$(openssl pkeyutl -sign -rawin -inkey "$KEY_FILE" -in "$SHA_FILE" 2>"$OPENSSL_ERR_FILE" | base64 -w 0)

if [ -z "$SIGNATURE" ]; then
    echo "ERROR: Signing failed." >&2
    echo "This does not necessarily mean the private key is malformed — check:" >&2
    echo "  - the key file / private key argument itself" >&2
    echo "  - the openssl version on this host (Ed25519 pkeyutl needs 3.2+," >&2
    echo "    or 3.0.x with -rawin, which this script already passes)" >&2
    echo "  - whether this openssl build supports -rawin at all" >&2
    echo "openssl's own error output:" >&2
    cat "$OPENSSL_ERR_FILE" >&2
    exit 1
fi

# Write approval file to the shared git common dir. This resolves to the main
# repo's .git/ even when called from a worktree (where .git is a pointer file,
# not a directory), so the wrapper at ~/.local/bin/gh can find it from either
# the worktree or the main repo.
GIT_COMMON_DIR=$(git rev-parse --path-format=absolute --git-common-dir 2>/dev/null)
if [ -z "$GIT_COMMON_DIR" ]; then
    echo "ERROR: Could not resolve git common dir. Are you in a git repository?" >&2
    exit 1
fi

mkdir -p "$GIT_COMMON_DIR/verification-approvals"
APPROVAL_FILE="$GIT_COMMON_DIR/verification-approvals/pr-${PR_NUMBER}"
cat > "$APPROVAL_FILE" <<EOF
${PR_NUMBER}
${HEAD_SHA}
${SIGNATURE}
EOF

echo "Verification approval signed for PR #${PR_NUMBER} at SHA ${HEAD_SHA}"
echo "Approval file: $APPROVAL_FILE"
