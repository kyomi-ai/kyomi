#!/usr/bin/env bash
# Exercise the installer's real write_env in POSIX sh without interactive setup.
# Observe permissions at secret writes: a final-mode check misses KYO-652.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INSTALLER="$SCRIPT_DIR/../deploy/install.sh"
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT
PASS=0
FAIL=0

pass() { printf '  PASS %s\n' "$1"; PASS=$((PASS + 1)); }
fail() { printf '  FAIL %s\n' "$1"; FAIL=$((FAIL + 1)); }

# Anchor extraction on function definitions, not the changed permission code.
awk '/^write_env\(\) \{/ { copying=1 } /^start_services\(\) \{/ { copying=0 } copying' \
    "$INSTALLER" > "$tmpdir/write-env.sh"
mkdir "$tmpdir/bin"
cat > "$tmpdir/bin/chmod" <<'CHMOD'
#!/bin/sh
if [ "$FAIL_CHMOD" = yes ]; then exit 1; fi
exec "$REAL_CHMOD" "$@"
CHMOD
chmod +x "$tmpdir/bin/chmod"
export REAL_CHMOD="$(command -v chmod)"

cat > "$tmpdir/run.sh" <<'RUN'
#!/bin/sh
set -e
. "$FUNCTION_FILE"
info() { :; }
ok() { : > "$CASE_DIR/success"; }

# Redirection is already open when this shell function runs. Inspect mode
# before emitting each secret, then delegate to the real shell builtin.
printf() {
    case "$1" in
        POSTGRES_PASSWORD=*|JWT_SECRET_KEY=*|ENCRYPTION_KEY=*|LLM_API_KEY=*)
            mode=$(stat -c '%a' "$INSTALL_DIR/.env")
            command printf '%s\n' "$mode" >> "$CASE_DIR/write-modes"
            if [ "$FAIL_WRITE" = yes ]; then return 1; fi
            ;;
    esac
    command printf "$@"
}

POSTGRES_PASSWORD='synthetic database value'
JWT_SECRET_KEY='synthetic jwt value'
ENCRYPTION_KEY='synthetic encryption value'
LLM_PROVIDER="$TEST_PROVIDER"
LLM_API_KEY='synthetic api value'
KYOMI_URL='http://installer.test.invalid:3100'
WEBAUTHN_RP_ID='installer.test.invalid'
umask "$TEST_UMASK"
trap 'umask > "$CASE_DIR/final-umask"' 0
write_env
RUN

run_case() {
    local name="$1" mask="$2" existing="$3" provider="$4" chmod_failure="$5" write_failure="$6"
    local case_dir="$tmpdir/$name"
    mkdir -p "$case_dir/install"
    # Keep harness observations readable even when the tested umask is 777.
    touch "$case_dir/write-modes" "$case_dir/final-umask"
    chmod 600 "$case_dir/write-modes" "$case_dir/final-umask"
    if [[ "$existing" = file ]]; then
        printf 'old configuration\n' > "$case_dir/install/.env"
        chmod 666 "$case_dir/install/.env"
    elif [[ "$existing" = directory ]]; then
        mkdir "$case_dir/install/.env"
    fi
    local status=0
    CASE_DIR="$case_dir" INSTALL_DIR="$case_dir/install" FUNCTION_FILE="$tmpdir/write-env.sh" \
        TEST_UMASK="$mask" TEST_PROVIDER="$provider" FAIL_CHMOD="$chmod_failure" \
        FAIL_WRITE="$write_failure" PATH="$tmpdir/bin:$PATH" \
        sh "$tmpdir/run.sh" > "$case_dir/output" 2>&1 || status=$?

    if [[ "$existing" = directory || "$chmod_failure" = yes || "$write_failure" = yes ]]; then
        if [[ "$status" -ne 0 && ! -e "$case_dir/success" ]]; then
            pass "$name: failure aborts before success"
        else
            fail "$name: failure must abort before success"
        fi
        if [[ "$existing" != directory ]]; then
            if ! grep -q 'synthetic' "$case_dir/install/.env"; then
                pass "$name: no secret was emitted"
            else
                fail "$name: secret emitted after failure"
            fi
        fi
    else
        local expected_writes=4
        [[ -n "$provider" ]] || expected_writes=3
        if [[ "$status" -eq 0 && -e "$case_dir/success" && -f "$case_dir/write-modes" ]] && \
            [[ "$(wc -l < "$case_dir/write-modes")" -eq "$expected_writes" ]] && \
            ! grep -qv '^600$' "$case_dir/write-modes"; then
            pass "$name: every secret write occurs with mode 600"
        else
            fail "$name: secret writes must all occur with mode 600"
            [[ ! -f "$case_dir/write-modes" ]] || cat "$case_dir/write-modes"
        fi
        if [[ "$(stat -c '%a' "$case_dir/install/.env")" = 600 ]] && \
            grep -q '^POSTGRES_PASSWORD=synthetic database value$' "$case_dir/install/.env" && \
            grep -q '^JWT_SECRET_KEY=synthetic jwt value$' "$case_dir/install/.env" && \
            grep -q '^ENCRYPTION_KEY=synthetic encryption value$' "$case_dir/install/.env" && \
            ! grep -q 'old configuration' "$case_dir/install/.env"; then
            pass "$name: complete configuration replaces existing content with final mode 600"
        else
            fail "$name: configuration content or final mode incorrect"
        fi
        if [[ -n "$provider" ]]; then
            grep -q '^LLM_API_KEY=synthetic api value$' "$case_dir/install/.env" && \
                pass "$name: configured provider key retained" || fail "$name: provider key missing"
        else
            ! grep -q '^LLM_API_KEY=' "$case_dir/install/.env" && \
                pass "$name: absent provider emits no key" || fail "$name: unexpected provider key"
        fi
    fi
    local expected_mask
    expected_mask="$(sh -c 'umask "$1"; umask' sh "$mask")"
    if [[ "$(cat "$case_dir/final-umask")" = "$expected_mask" ]]; then
        pass "$name: caller umask preserved"
    else
        fail "$name: caller umask changed"
    fi
}

echo 'Running installer environment permission tests...'
run_case world-writable 000 absent anthropic no no
run_case default-mask 022 absent anthropic no no
run_case existing-readable 000 file anthropic no no
run_case restrictive-mask 777 absent anthropic no no
run_case no-provider 000 absent '' no no
run_case chmod-failure 000 file anthropic yes no
run_case creation-failure 000 directory anthropic no no
run_case write-failure 000 absent anthropic no yes
printf '\n%d passed, %d failed\n' "$PASS" "$FAIL"
[[ "$FAIL" -eq 0 ]]
