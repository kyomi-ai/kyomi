#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-real-identifiers-test.sh — self-test for check-real-identifiers.sh
#
# Plants synthetic fixtures in a tmp directory and invokes the linter against
# each one, asserting exit code and (where relevant) output content. Every
# identifier used anywhere in this file is synthetic — see the linter's own
# header and docs/standards/security/no-real-world-identifiers-in-a-public-repo.md
# for why that is not optional in this repo.
#
# Usage:
#   ./scripts/lint/check-real-identifiers-test.sh
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LINTER="$SCRIPT_DIR/check-real-identifiers.sh"

if [ ! -x "$LINTER" ]; then
    echo "ERROR: linter not executable at $LINTER" >&2
    exit 2
fi

TMP="$(mktemp -d -t check-real-identifiers-test-XXXXXX)"
trap 'rm -rf "$TMP"' EXIT

checks=0
fails=0

# ------------------------------------------------------------------------------
# assert_exit — invoke the linter with the given args, assert exit code.
#
# Args: $1 label, $2 expected exit code, remaining args passed to the linter.
# Sets $LAST_OUTPUT so callers can chain an assert_contains / assert_not_contains
# against the exact same invocation without running the linter twice.
# ------------------------------------------------------------------------------
LAST_OUTPUT=""
assert_exit() {
    local label="$1" expected="$2"
    shift 2
    local actual

    checks=$((checks + 1))

    set +e
    LAST_OUTPUT="$("$LINTER" "$@" 2>&1)"
    actual=$?
    set -e

    if [ "$actual" -eq "$expected" ]; then
        printf 'PASS  %-55s  (exit %d as expected)\n' "$label" "$actual"
    else
        printf 'FAIL  %-55s  expected exit %d, got %d\n' "$label" "$expected" "$actual"
        if [ -n "$LAST_OUTPUT" ]; then
            printf '      linter output:\n'
            printf '        %s\n' "$LAST_OUTPUT" | sed 's/^        /      /'
        fi
        fails=$((fails + 1))
    fi
}

assert_contains() {
    local label="$1" needle="$2"
    checks=$((checks + 1))
    if printf '%s' "$LAST_OUTPUT" | grep -qF "$needle"; then
        printf 'PASS  %-55s  (found %q)\n' "$label" "$needle"
    else
        printf 'FAIL  %-55s  expected output to contain %q\n' "$label" "$needle"
        printf '      got:\n'
        printf '        %s\n' "$LAST_OUTPUT" | sed 's/^        /      /'
        fails=$((fails + 1))
    fi
}

assert_not_contains() {
    local label="$1" needle="$2"
    checks=$((checks + 1))
    if printf '%s' "$LAST_OUTPUT" | grep -qF "$needle"; then
        printf 'FAIL  %-55s  output must NOT contain %q but did\n' "$label" "$needle"
        printf '      got:\n'
        printf '        %s\n' "$LAST_OUTPUT" | sed 's/^        /      /'
        fails=$((fails + 1))
    else
        printf 'PASS  %-55s  (did not find %q)\n' "$label" "$needle"
    fi
}

# ------------------------------------------------------------------------------
# Rule: gcp-service-account
# ------------------------------------------------------------------------------
cat > "$TMP/gsa.txt" <<'EOF'
Fixture: a service-account address.
credentials: test-service-account@test-project.iam.gserviceaccount.com
EOF
assert_exit    "gcp-service-account shape is rejected" 1 "$TMP/gsa.txt"
assert_contains "  reports the right rule"              "rule gcp-service-account"

# ------------------------------------------------------------------------------
# Rule: email-address
# ------------------------------------------------------------------------------
cat > "$TMP/email.txt" <<'EOF'
Contact sales@some-unlisted-domain-example.test for a demo.
EOF
assert_exit    "email-address with non-allowed domain is rejected" 1 "$TMP/email.txt"
assert_contains "  reports the right rule"                          "rule email-address"

cat > "$TMP/email_allowed.txt" <<'EOF'
Contact test-service-account@test-project or someone@example.com instead.
EOF
assert_exit "email-address with an allowed domain passes" 0 "$TMP/email_allowed.txt"

# ------------------------------------------------------------------------------
# Rule: public-ipv4 — a real public address is rejected...
# ------------------------------------------------------------------------------
cat > "$TMP/public_ip.txt" <<'EOF'
Upstream resolver observed at 1.1.1.1 during the incident.
EOF
assert_exit    "public IPv4 is rejected"    1 "$TMP/public_ip.txt"
assert_contains "  reports the right rule"  "rule public-ipv4"

# ...and every excluded range is accepted: private/loopback/link-local/
# multicast/reserved/broadcast/the "this network" address, plus all three
# RFC 5737 documentation ranges.
cat > "$TMP/excluded_ips.txt" <<'EOF'
this-network 0.0.0.0
loopback 127.0.0.1
private-10 10.1.2.3
private-172 172.16.5.6
private-192 192.168.1.1
link-local 169.254.1.1
multicast 224.0.0.5
reserved 240.0.0.1
broadcast 255.255.255.255
rfc5737-test-net-1 192.0.2.55
rfc5737-test-net-2 198.51.100.23
rfc5737-test-net-3 203.0.113.99
EOF
assert_exit "every excluded IPv4 range passes" 0 "$TMP/excluded_ips.txt"

# ------------------------------------------------------------------------------
# Rule: credential-token
# ------------------------------------------------------------------------------
cat > "$TMP/credential.txt" <<'EOF'
Authorization: Bearer thisisadefinitelyfaketokenvaluepadding123456
EOF
assert_exit    "credential-token shape is rejected" 1 "$TMP/credential.txt"
assert_contains "  reports the right rule"           "rule credential-token"

# ------------------------------------------------------------------------------
# Rule: denylisted-identifier — a throwaway denylist over a synthetic value,
# via REAL_IDENTIFIERS_DENYLIST. This is the whole point of that override:
# the self-test never needs (or sees) the real, private plaintext list.
# ------------------------------------------------------------------------------
DENY_SALT="deadbeefdeadbeefdeadbeefdeadbeef"
DENY_VALUE="totallyfakewidgetsinc"
DENY_HASH="$(printf '%s:%s' "$DENY_SALT" "$DENY_VALUE" | sha256sum | awk '{print $1}')"

cat > "$TMP/throwaway-denylist.txt" <<EOF
# salt=$DENY_SALT
$DENY_HASH
EOF

cat > "$TMP/deny_whole_token.txt" <<EOF
Our old vendor was called $DENY_VALUE back then.
EOF
REAL_IDENTIFIERS_DENYLIST="$TMP/throwaway-denylist.txt" \
    assert_exit "denylist: whole token is rejected" 1 "$TMP/deny_whole_token.txt"
assert_contains "  reports the right rule" "rule denylisted-identifier"

cat > "$TMP/deny_compound_segment.txt" <<EOF
Their staging host was named ${DENY_VALUE}-prod01.internal.
EOF
REAL_IDENTIFIERS_DENYLIST="$TMP/throwaway-denylist.txt" \
    assert_exit "denylist: compound containing the token as a segment is rejected" 1 "$TMP/deny_compound_segment.txt"
assert_contains "  reports the right rule" "rule denylisted-identifier"

cat > "$TMP/deny_unrelated.txt" <<'EOF'
Nothing here matches anything on the denylist.
EOF
REAL_IDENTIFIERS_DENYLIST="$TMP/throwaway-denylist.txt" \
    assert_exit "denylist: unrelated content still passes" 0 "$TMP/deny_unrelated.txt"

# ------------------------------------------------------------------------------
# Message mode: --message-file
# ------------------------------------------------------------------------------
cat > "$TMP/commit-msg-bad.txt" <<EOF
KYO-999: mention $DENY_VALUE in passing

This should never have been written in a commit message.
EOF
REAL_IDENTIFIERS_DENYLIST="$TMP/throwaway-denylist.txt" \
    assert_exit "--message-file rejects a denylisted value" 1 --message-file "$TMP/commit-msg-bad.txt"
assert_contains "  reports the right rule"    "rule denylisted-identifier"
assert_contains "  uses the commit-message pseudo-path" "commit-message:"

cat > "$TMP/commit-msg-good.txt" <<'EOF'
KYO-999: a perfectly ordinary commit message

Nothing sensitive here.
EOF
REAL_IDENTIFIERS_DENYLIST="$TMP/throwaway-denylist.txt" \
    assert_exit "--message-file passes a clean message" 0 --message-file "$TMP/commit-msg-good.txt"

assert_exit "--message-file on a nonexistent file is a usage error" 2 --message-file "$TMP/does-not-exist.txt"

# ------------------------------------------------------------------------------
# Message mode: --messages <range>, against this repo's own real history.
# HEAD..HEAD is an empty range (no commits), so `git log` succeeds and
# produces no output — this is the "clean, nothing to scan" case, distinct
# from a failing git invocation.
# ------------------------------------------------------------------------------
assert_exit "--messages over an empty range passes" 0 --messages "HEAD..HEAD"

assert_exit "--messages over a bogus range is a usage error" 2 --messages "not-a-real-ref..HEAD"

# ------------------------------------------------------------------------------
# Allowed fixture: a path listed in scripts/lint/real-identifiers-allow.txt
# passes even though its content would otherwise trip gcp-service-account or
# credential-token. Run against the REAL allow file (no override exists for
# it — the point is proving the seeded entries work against real content).
# Paths are given exactly as the allow-file's glob expects: relative to the
# repo root, which is how pre-commit invokes this linter over staged paths.
# ------------------------------------------------------------------------------
assert_exit "allow-listed GCP fixture (catalog_scheduler.rs) passes" 0 \
    "crates/kyomi-agent/src/catalog_scheduler.rs"
assert_exit "allow-listed GCP fixture (bigquery-create-modal.cjs) passes" 0 \
    "scripts/e2e-regression/bigquery-create-modal.cjs"
assert_exit "allow-listed Slack token fixture (encryption.rs) passes" 0 \
    "crates/kyomi-auth/src/encryption.rs"

# ------------------------------------------------------------------------------
# Clean input: only synthetic values throughout — exits 0.
# ------------------------------------------------------------------------------
cat > "$TMP/clean.txt" <<'EOF'
Everything in this fixture is synthetic: test-service-account@test-project,
acme-corp-472819, example.com, 203.0.113.7, and Bearer notarealtoken.
EOF
assert_exit "an all-synthetic fixture passes" 0 "$TMP/clean.txt"

# ------------------------------------------------------------------------------
# Acceptance criterion: failure output must never contain the matched value,
# for any rule. Assert this directly rather than trusting the design intent.
# ------------------------------------------------------------------------------
cat > "$TMP/no_leak_gsa.txt" <<'EOF'
credentials: test-service-account@test-project.iam.gserviceaccount.com
EOF
assert_exit "no-leak: gcp-service-account" 1 "$TMP/no_leak_gsa.txt"
assert_not_contains "  value not echoed" "test-service-account@test-project.iam.gserviceaccount.com"

assert_exit "no-leak: public-ipv4" 1 "$TMP/public_ip.txt"
assert_not_contains "  value not echoed" "1.1.1.1"

assert_exit "no-leak: credential-token" 1 "$TMP/credential.txt"
assert_not_contains "  value not echoed" "thisisadefinitelyfaketokenvaluepadding123456"

REAL_IDENTIFIERS_DENYLIST="$TMP/throwaway-denylist.txt" \
    assert_exit "no-leak: denylisted-identifier" 1 "$TMP/deny_whole_token.txt"
assert_not_contains "  value not echoed"       "$DENY_VALUE"
assert_not_contains "  hash not echoed either" "$DENY_HASH"

# ------------------------------------------------------------------------------
# Explicit-path mode silently skips a path that no longer exists (mirrors a
# staged deletion in pre-commit — a deleted file cannot introduce a new
# violation).
# ------------------------------------------------------------------------------
assert_exit "a deleted/nonexistent explicit path is skipped, not an error" 0 \
    "$TMP/this-file-was-never-created.txt"

# ------------------------------------------------------------------------------
# Usage errors.
# ------------------------------------------------------------------------------
assert_exit "unknown option is a usage error" 2 --not-a-real-flag
assert_exit "--message-file and --messages together is a usage error" 2 \
    --message-file "$TMP/commit-msg-good.txt" --messages "HEAD..HEAD"
assert_exit "--message-file combined with a path arg is a usage error" 2 \
    --message-file "$TMP/commit-msg-good.txt" "$TMP/clean.txt"

# ------------------------------------------------------------------------------
# Fail-closed: a denylist file that cannot be parsed must not silently
# behave like "no denylist rule" — it must fail the whole run.
# ------------------------------------------------------------------------------
cat > "$TMP/no-salt-denylist.txt" <<'EOF'
# no salt line here at all
4ae91ffb5c16d9f031b058093802c0d847e4d903bd6806cc6eff5b1f65e5c86f
EOF
REAL_IDENTIFIERS_DENYLIST="$TMP/no-salt-denylist.txt" \
    assert_exit "a denylist with no salt= line fails closed (not exit 0)" 1 "$TMP/clean.txt"

REAL_IDENTIFIERS_DENYLIST="$TMP/does-not-exist-denylist.txt" \
    assert_exit "a missing denylist path fails closed (not exit 0)" 1 "$TMP/clean.txt"

# ------------------------------------------------------------------------------
# Summary.
# ------------------------------------------------------------------------------
printf '\n%d checks, %d failed\n' "$checks" "$fails"
if [ "$fails" -eq 0 ]; then
    echo 'All fixtures passed.'
    exit 0
fi
exit 1
