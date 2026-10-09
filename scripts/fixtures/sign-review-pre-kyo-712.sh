#!/bin/bash
# TEST FIXTURE ONLY — never use this signer for a real review.
# Frozen from scripts/sign-review.sh at commit
# 686c9a914a367d5f4a860b21a241b6e1d4a4749f (parent of KYO-712's abe205f8).
# This is the genuine historical hash-only, two-line approval producer.
# The only executable change from that revision is adding -rawin below:
# OpenSSL 3.0.x requires it for Ed25519; newer versions produce the same
# signature bytes with it. Keep this fixture frozen to preserve the legacy
# protocol tested by pre-commit-hook-test.sh, independent of origin/main.
# sign-review.sh — Called by the code-review-architect agent after a successful review.
# Signs the staged diff with the reviewer's private key.
# Usage: scripts/sign-review.sh <private_key_pem_string>

set -e

PRIVATE_KEY="$1"

if [ -z "$PRIVATE_KEY" ]; then
    echo "ERROR: Private key argument required." >&2
    exit 1
fi

# Write private key to temp file (Ed25519 PEM format)
KEY_FILE=$(mktemp)
HASH_FILE=$(mktemp)
trap 'rm -f "$KEY_FILE" "$HASH_FILE"' EXIT
echo "$PRIVATE_KEY" > "$KEY_FILE"

# Compute sha256 of the staged diff
DIFF_HASH=$(git diff --cached | sha256sum | awk '{print $1}')

if [ -z "$DIFF_HASH" ] || [ "$DIFF_HASH" = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" ]; then
    echo "ERROR: No staged changes to sign." >&2
    exit 1
fi

# Sign the hash with Ed25519 (must use -in file, not stdin — Ed25519 requires it)
echo -n "$DIFF_HASH" > "$HASH_FILE"
SIGNATURE=$(openssl pkeyutl -sign -rawin -inkey "$KEY_FILE" -in "$HASH_FILE" | base64 -w 0)

if [ -z "$SIGNATURE" ]; then
    echo "ERROR: Signing failed — check private key format." >&2
    exit 1
fi

# Write approval file
cat > .review-approval <<EOF
${DIFF_HASH}
${SIGNATURE}
EOF

echo "Review approval signed for diff hash: ${DIFF_HASH}"
