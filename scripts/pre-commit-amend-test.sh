#!/usr/bin/env bash
# KYO-634: real git commit/amend attempts with the actual hook and linters.
# Successful lint controls commit without review approval files.
# PRE_COMMIT_HOOK_SRC supports red/green runs
# against a scratch copy of the original hook without mutating this worktree.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_SRC="${PRE_COMMIT_HOOK_SRC:-$REPO_ROOT/.githooks/pre-commit}"
PASS=0 FAIL=0
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
# Isolate fixture commits from global hooks, signing, and lint overrides.
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME="Hook Test" GIT_AUTHOR_EMAIL="hook-test@kyomi.invalid"
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME" GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
unset SERVER_FN_LINT_DIR SERVER_FN_CALLOUT_MAX DISPOSAL_LINT_DIR DISPOSAL_BASELINE_FILE
unset REAL_IDENTIFIERS_DENYLIST

new_repo() {
    repo="$scratch/$1"
    git init -q -b main "$repo"
    mkdir -p "$repo/.githooks" "$repo/scripts"
    cp "$HOOK_SRC" "$repo/.githooks/pre-commit"
    chmod +x "$repo/.githooks/pre-commit"
    cp -R "$SCRIPT_DIR/lint" "$repo/scripts/lint"
    printf '.githooks/\nscripts/\n' > "$repo/.git/info/exclude"
    printf 'seed\n' > "$repo/README.md"
    git -C "$repo" add README.md
    git -C "$repo" commit -q -m seed
}

write_fixture() {
    local kind="$1"
    case "$kind" in
        rust) path=bad.rs; content='#[allow(dead_code)]\nfn unused() {}\n' ;;
        toml) path=Cargo.toml; content='[lints.rust]\ndead_code = "allow"\n' ;;
        server) path=crates/kyomi-ui/src/server_fns/bad.rs
            content='#[server(prefix = "/leptos-api")]\npub async fn bad() -> Result<(), ServerFnError> {\n    let context = use_context::<MissingService>();\n    Ok(())\n}\n' ;;
        disposal) path=crates/kyomi-ui/src/bad.rs
            content='fn bad() {\n    spawn_local(async move {\n        signal.set(true);\n    });\n}\n' ;;
    esac
    mkdir -p "$repo/$(dirname "$path")"
    printf '%b' "$content" > "$repo/$path"
    git -C "$repo" add "$path"
}

attempt() {
    git -C "$repo" config core.hooksPath .githooks
    before="$(git -C "$repo" rev-parse HEAD)"
    if output="$(git -C "$repo" commit "$@" -m attempt 2>&1)"; then status=0; else status=$?; fi
}

assert_blocked_by() {
    local label="$1" diagnostic="$2"
    if [ "$status" -eq 1 ] && [[ "$output" == *"$diagnostic"* ]] \
        && [ "$(git -C "$repo" rev-parse HEAD)" = "$before" ]; then
        printf '  PASS: %s\n' "$label"; PASS=$((PASS + 1))
    else
        printf '  FAIL: %s (exit %s)\n%s\n' "$label" "$status" "$output"
        FAIL=$((FAIL + 1))
    fi
}

assert_success() {
    if [ "$status" -eq 0 ]; then
        printf '  PASS: %s\n' "$1"; PASS=$((PASS + 1))
    else
        printf '  FAIL: %s (exit %s)\n%s\n' "$1" "$status" "$output"
        FAIL=$((FAIL + 1))
    fi
}

for mode in staged amend; do
    for kind in rust toml server disposal; do
        new_repo "$mode-$kind"
        write_fixture "$kind"
        args=()
        if [ "$mode" = amend ]; then
            # Deliberately hookless fixture, before activating the copied hook.
            git -C "$repo" commit -q -m hookless
            git -C "$repo" diff --cached --quiet
            args=(--amend --no-edit)
        fi
        attempt "${args[@]}"
        case "$kind" in
            rust|toml) diagnostic='New lint suppressions detected' ;;
            server) diagnostic='server_fn lint failed' ;;
            disposal) diagnostic='disposal-safety lint failed' ;;
        esac
        assert_blocked_by "$mode $kind rejected by its lint" "$diagnostic"
    done
done

# Root HEAD has no HEAD^: its full content must still be scanned.
repo="$scratch/root"
git init -q -b main "$repo"
mkdir -p "$repo/.githooks"
cp "$HOOK_SRC" "$repo/.githooks/pre-commit"
chmod +x "$repo/.githooks/pre-commit"
printf '.githooks/\n' > "$repo/.git/info/exclude"
write_fixture rust
git -C "$repo" commit -q -m hookless-root
attempt --amend --no-edit
assert_blocked_by 'root amendment rejects suppression' 'New lint suppressions detected'

# Nonempty index must not scan a suppression inherited from the old HEAD.
new_repo staged-history
write_fixture rust
git -C "$repo" commit -q -m hookless
printf 'changed\n' >> "$repo/README.md"
git -C "$repo" add README.md
attempt
assert_success 'nonempty staged delta ignores historical suppression without approval'

# Clean amendment invokes both real UI linters and succeeds.
new_repo clean-amend
path=crates/kyomi-ui/src/server_fns/clean.rs
mkdir -p "$repo/$(dirname "$path")"
printf '#[server(prefix = "/leptos-api")]\npub async fn clean() -> Result<(), ServerFnError> {\n    Ok(())\n}\n' > "$repo/$path"
git -C "$repo" add "$path"
git -C "$repo" commit -q -m clean
attempt --amend --no-edit
assert_success 'clean amendment succeeds without approval'
for diagnostic in 'server_fn lint clean.' 'disposal-safety lint clean.'; do
    if [[ "$output" == *"$diagnostic"* ]]; then
        printf '  PASS: clean amendment %s\n' "$diagnostic"; PASS=$((PASS + 1))
    else
        printf '  FAIL: clean amendment missing %s\n%s\n' "$diagnostic" "$output"; FAIL=$((FAIL + 1))
    fi
done
printf 'Results: %s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
