#!/usr/bin/env bash
# Exercise real commits with the tracked hook, without review artifacts.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME='Hook Test' GIT_AUTHOR_EMAIL='hook-test@kyomi.invalid'
export GIT_COMMITTER_NAME="$GIT_AUTHOR_NAME" GIT_COMMITTER_EMAIL="$GIT_AUTHOR_EMAIL"
PASS=0 FAIL=0
new_repo() {
    repo="$scratch/$1"
    git init -q -b main "$repo"
    mkdir -p "$repo/.githooks" "$repo/scripts"
    cp "$SCRIPT_DIR/../.githooks/pre-commit" "$repo/.githooks/pre-commit"
    cp -R "$SCRIPT_DIR/lint" "$repo/scripts/lint"
    printf '.githooks/\nscripts/\n' > "$repo/.git/info/exclude"
    git -C "$repo" config core.hooksPath .githooks
    printf 'seed\n' > "$repo/README.md"
    git -C "$repo" add README.md
}
attempt() {
    if output="$(git -C "$repo" commit -m test 2>&1)"; then status=0; else status=$?; fi
}
check() {
    if [ "$status" -eq "$2" ] && [[ "$output" == *"$3"* ]]; then
        echo "PASS: $1"; PASS=$((PASS + 1))
    else
        printf 'FAIL: %s (exit %s)\n%s\n' "$1" "$status" "$output"; FAIL=$((FAIL + 1))
    fi
}
new_repo no-approval
attempt
check 'commit succeeds with no review approval' 0 'No new lint suppressions'
new_repo obsolete-approval
# An obsolete malformed artifact must have no effect on the hook.
printf 'obsolete\n' > "$repo/.review-approval"
attempt
check 'obsolete approval artifact has no effect' 0 'No new lint suppressions'
new_repo suppression
printf '#[allow(dead_code)]\nfn unused() {}\n' > "$repo/bad.rs"
git -C "$repo" add bad.rs
attempt
check 'staged lint suppression remains blocked' 1 'New lint suppressions detected'
for state in untracked unstaged; do
    for kind in server disposal; do
        new_repo "$state-$kind"
        path=crates/kyomi-ui/src/bad.rs
        if [ "$kind" = server ]; then path=crates/kyomi-ui/src/server_fns/bad.rs; fi
        mkdir -p "$repo/$(dirname "$path")"
        if [ "$state" = unstaged ]; then
            printf 'fn clean() {}\n' > "$repo/$path"
            git -C "$repo" add "$path"
            git -C "$repo" commit -q -m clean
            printf "changed\n" >> "$repo/README.md"
            git -C "$repo" add README.md
        fi
        if [ "$kind" = server ]; then
            printf '#[server(prefix = "/leptos-api")]\npub async fn bad() -> Result<(), ServerFnError> {\n    let context = use_context::<MissingService>();\n    Ok(())\n}\n' > "$repo/$path"
            diagnostic='server_fn lint failed for unstaged/untracked files'
        else
            printf 'fn bad() {\n    spawn_local(async move {\n        signal.set(true);\n    });\n}\n' > "$repo/$path"
            diagnostic='disposal-safety lint failed'
        fi
        attempt
        check "$state $kind violation remains blocked" 1 "$diagnostic"
    done
done
new_repo clean-untracked
mkdir -p "$repo/crates/kyomi-ui/src"
printf 'fn clean() {}\n' > "$repo/crates/kyomi-ui/src/clean.rs"
attempt
check 'clean untracked UI file is linted without a signed exception' 0 'Checking disposal-safety lint for unstaged/untracked files'
printf 'Results: %s passed, %s failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ]
