#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/release-macos-desktop.sh — build and publish the macOS desktop installer
#
# KYO-991. The macOS leg of .github/workflows/desktop.yml ran past its
# 300-minute limit, so that workflow builds Linux and Windows only and the Mac
# installer is built by hand, on an Apple-silicon Mac, with this script. It
# runs the same steps and settings the removed CI leg ran:
#
#   trunk build --release (kyomi-ui) -> wasm-opt -> chart app build.sh ->
#   cargo tauri build --bundles dmg, fat LTO, the release profile untouched
#
# from a clean checkout of the tag (a throwaway git worktree, so the working
# tree you ran it from is never used or changed), with the tool versions the
# workflow pins. It then checks the app inside the dmg is arm64-only and
# ad-hoc signed (as CI signed it), and uploads the dmg to that tag's GitHub
# release under the file name Tauri gave it, as CI did.
#
# Usage:
#   scripts/release-macos-desktop.sh <tag>              build, verify, upload
#   scripts/release-macos-desktop.sh --dry-run <ref>    build and verify any git
#                                                       ref; upload nothing
#
# <tag> is a release tag (vMAJOR.MINOR.PATCH, for example v2.7.17) whose
# GitHub release already exists (release.yml creates it) and does not yet have
# a Mac installer attached. Run from a logged-in Mac session with `gh`
# authenticated for the repository. A release build takes about 45 minutes
# plus the one-off compile of the pinned tauri-cli. Set CARGO_TARGET_DIR to an
# absolute path to keep build products between runs; otherwise every run
# compiles from scratch. If the final upload fails, the built dmg is copied to
# the current directory so the upload can be retried without a rebuild.
#
# Exit codes: 0 success; 2 usage error; 1 any other failure. Nothing here
# falls back: a missing tool, a version mismatch or a failed step stops the run.
# ------------------------------------------------------------------------------

set -euo pipefail

START_DIR="$PWD"
TAG_PATTERN='^v[0-9]+\.[0-9]+\.[0-9]+$'

die() {
    printf 'release-macos-desktop: %s\n' "$*" >&2
    exit 1
}

usage() {
    sed -n '/^# Usage:/,/^# <tag>/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# read_pin <file> <regex-with-one-capture-group> -> the single captured value
read_pin() {
    local file="$1" pattern="$2" value
    [ -f "$file" ] || die "cannot read pinned versions: $file does not exist"
    value="$(sed -nE "s/${pattern}/\\1/p" "$file")"
    [ -n "$value" ] || die "no version found in $file for pattern: $pattern"
    [ "$(printf '%s\n' "$value" | wc -l | tr -d ' ')" = 1 ] \
        || die "more than one version found in $file for pattern: $pattern"
    printf '%s\n' "$value"
}

# The pins live in the repository, not here, so a version bump in the workflow
# or the setup-trunk action is picked up by this script without an edit.
pin_trunk() { read_pin "$1/.github/actions/setup-trunk/action.yml" '^ *run: echo "version=([0-9.]+)" >> "\$GITHUB_OUTPUT"$'; }
pin_wasm_bindgen() { read_pin "$1/.github/workflows/desktop.yml" '^  WASM_BINDGEN_VERSION: ([0-9.]+)$'; }
pin_tailwind() { read_pin "$1/.github/workflows/desktop.yml" '^  TAILWIND_VERSION: (v[0-9.]+)$'; }
pin_binaryen() { read_pin "$1/.github/workflows/desktop.yml" '^  BINARYEN_VERSION: (version_[0-9]+)$'; }

require_tool() {
    command -v "$1" > /dev/null 2>&1 || die "required tool not found on PATH: $1${2:+ ($2)}"
}

require_apple_silicon_mac() {
    [ "$(uname -s)" = Darwin ] || die "this builds the macOS installer and must run on a Mac (this is $(uname -s))"
    [ "$(uname -m)" = arm64 ] || die "the installer is arm64 and must be built on an Apple-silicon Mac (this is $(uname -m))"
}

# origin_slug <remote-url> -> OWNER/REPO, for the https and ssh forms GitHub gives
origin_slug() {
    local slug
    slug="$(printf '%s\n' "$1" | sed -nE 's#^(https://github\.com/|git@github\.com:|ssh://git@github\.com/)([^/]+/[^/]+)$#\2#p' | sed -E 's#\.git$##')"
    [ -n "$slug" ] || die "origin is not a github.com remote: $1"
    printf '%s\n' "$slug"
}

# Validates the argument and prints "<mode> <ref>" (mode: release or dry-run).
parse_args() {
    local mode=release ref
    if [ "${1:-}" = "--dry-run" ]; then
        mode=dry-run
        shift
    fi
    [ "$#" -eq 1 ] || usage
    ref="$1"
    case "$ref" in -*) usage ;; esac
    if [ "$mode" = release ] && ! [[ "$ref" =~ $TAG_PATTERN ]]; then
        printf 'release-macos-desktop: "%s" is not a release tag (expected vMAJOR.MINOR.PATCH); use --dry-run for any other ref\n' "$ref" >&2
        exit 2
    fi
    printf '%s %s\n' "$mode" "$ref"
}

# run_in <dir> <command...> runs a long step as a background job in its own
# process group and waits for it, so that a signal sent to this script is
# acted on at once (bash defers a trapped signal until a foreground command
# ends, which here would be the end of a 45-minute build) and takes the whole
# build tree down with it, not just the shell that started it.
CHILD=""
run_in() {
    local dir="$1" status=0
    shift
    set -m
    (cd "$dir" && exec "$@") < /dev/null &
    CHILD=$!
    wait "$CHILD" || status=$?
    CHILD=""
    return "$status"
}

# terminate <exit-code>: the INT/TERM/HUP handler.
terminate() {
    if [ -n "$CHILD" ]; then
        kill -TERM -- "-$CHILD" 2> /dev/null || true
        wait "$CHILD" 2> /dev/null || true
    fi
    exit "$1"
}

# Download one pinned release asset. curl --fail makes a missing asset an error.
fetch() { curl --fail --silent --show-error --location "$1"; }

install_pinned_tools() {
    local tools="$1" src="$2"
    local trunk wasm_bindgen tailwind binaryen
    trunk="$(pin_trunk "$src")"
    wasm_bindgen="$(pin_wasm_bindgen "$src")"
    tailwind="$(pin_tailwind "$src")"
    binaryen="$(pin_binaryen "$src")"
    mkdir -p "$tools/bin"

    echo "==> pinned tools: trunk $trunk, wasm-bindgen $wasm_bindgen, tailwindcss $tailwind, binaryen $binaryen"
    fetch "https://github.com/trunk-rs/trunk/releases/download/v${trunk}/trunk-aarch64-apple-darwin.tar.gz" \
        | tar -xz -C "$tools/bin" trunk
    fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${wasm_bindgen}/wasm-bindgen-${wasm_bindgen}-aarch64-apple-darwin.tar.gz" \
        | tar -xz --strip-components=1 -C "$tools/bin" "wasm-bindgen-${wasm_bindgen}-aarch64-apple-darwin/wasm-bindgen"
    fetch "https://github.com/tailwindlabs/tailwindcss/releases/download/${tailwind}/tailwindcss-macos-arm64" \
        > "$tools/bin/tailwindcss"
    chmod +x "$tools/bin/tailwindcss"
    # wasm-opt finds libbinaryen through @loader_path/../lib, so keep the
    # release's bin/ and lib/ side by side.
    fetch "https://github.com/WebAssembly/binaryen/releases/download/${binaryen}/binaryen-${binaryen}-arm64-macos.tar.gz" \
        | tar -xz -C "$tools"
    ln -s "$tools/binaryen-${binaryen}/bin/wasm-opt" "$tools/bin/wasm-opt"

    # The same compile CI ran. --root keeps it out of ~/.cargo/bin.
    run_in "$PWD" cargo install tauri-cli --version "^2" --locked --root "$tools"

    [ "$("$tools/bin/trunk" --version)" = "trunk $trunk" ] || die "installed trunk is not $trunk"
    [ "$("$tools/bin/wasm-bindgen" --version)" = "wasm-bindgen $wasm_bindgen" ] || die "installed wasm-bindgen is not $wasm_bindgen"
}

# The exact edits the CI "Strip local-dev linker and trunk profile" step made.
strip_local_dev_config() {
    local src="$1"
    sed -i '' '/# BEGIN_LOCAL_DEV_LINKER/,/# END_LOCAL_DEV_LINKER/d' "$src/.cargo/config.toml"
    sed -i '' '/# BEGIN_LOCAL_DEV_DEBUGINFO/,/# END_LOCAL_DEV_DEBUGINFO/d' "$src/.cargo/config.toml"
    sed -i '' '/cargo_profile/d' "$src/crates/kyomi-ui/Trunk.toml"
}

# Checks the app in a mounted dmg is arm64-only and ad-hoc signed as CI signed
# it, and logs what is inside. Runs in a subshell so a failed check (die) ends
# only the check, and the caller can still unmount before failing.
inspect_app() {
    local app="$1" dmg="$2" binary archs signature
    binary="$app/Contents/MacOS/kyomi-desktop"
    [ -x "$binary" ] || die "$dmg does not contain Kyomi.app/Contents/MacOS/kyomi-desktop"

    # Every command ends in `|| die`: this runs inside a subshell on the left of
    # `||`, where bash ignores `set -e`, so a bare failing command would be missed.
    lipo -info "$binary" || die "lipo cannot read $binary"
    archs="$(lipo -archs "$binary")" || die "lipo cannot read the architectures of $binary"
    [ "$archs" = arm64 ] || die "the app binary is '$archs', not arm64 only"

    (cd "$app" && find . -type f | LC_ALL=C sort) || die "cannot list the files in $app"
    plutil -p "$app/Contents/Info.plist" || die "cannot read $app/Contents/Info.plist"
    codesign --verify --deep --strict "$app" || die "the code signature of $app does not verify"
    signature="$(codesign -dvv "$app" 2>&1)" || die "codesign cannot describe $app"
    printf '%s\n' "$signature"
    printf '%s\n' "$signature" | grep -q '^Signature=adhoc$' \
        || die "the app is not ad-hoc signed (CI signed it with identity \"-\")"
    printf '%s\n' "$signature" | grep -q '^Identifier=ai.kyomi.desktop$' \
        || die "the app's identifier is not ai.kyomi.desktop"
    du -sk "$dmg" "$app" "$binary" || die "cannot size $app"
}

# The mountpoint lives under $WORK and MOUNT is detached by cleanup, so an
# interrupted or failed run never leaves the dmg attached.
verify_dmg() {
    local dmg="$1" status=0
    local mountpoint="$WORK/dmg-mount"
    mkdir -p "$mountpoint"
    hdiutil attach -readonly -nobrowse -mountpoint "$mountpoint" "$dmg" > /dev/null \
        || die "cannot mount $dmg"
    MOUNT="$mountpoint"
    (inspect_app "$MOUNT/Kyomi.app" "$dmg") || status=$?
    hdiutil detach "$MOUNT" > /dev/null || die "cannot detach $MOUNT"
    MOUNT=""
    [ "$status" -eq 0 ] || die "verification of $dmg failed"
}

# Runs on every exit, including failed runs. Each step is best effort because
# cleanup must not hide the error that ended the run; what it cannot remove is
# named, never silently left.
MOUNT=""
cleanup() {
    set +e # an EXIT trap runs under the script's `set -e`; every step must run
    cd "$START_DIR" || cd /
    if [ -n "$MOUNT" ] && ! hdiutil detach -force "$MOUNT" > /dev/null 2>&1; then
        printf 'release-macos-desktop: could not detach %s; leaving %s in place\n' "$MOUNT" "$WORK" >&2
        return 0
    fi
    if [ -e "$SRC/.git" ]; then
        git -C "$REPO" worktree remove --force "$SRC" > /dev/null 2>&1 \
            || printf 'release-macos-desktop: could not remove worktree %s; run: git worktree remove --force %s\n' "$SRC" "$SRC" >&2
    fi
    rm -rf "$WORK"
    return 0
}

main() {
    local parsed mode ref
    case "${CARGO_TARGET_DIR:-/}" in
        /*) ;;
        *) die "CARGO_TARGET_DIR must be an absolute path (the build runs from inside the checkout): $CARGO_TARGET_DIR" ;;
    esac
    parsed="$(parse_args "$@")" || exit $?
    mode="${parsed%% *}"
    ref="${parsed#* }"

    require_apple_silicon_mac
    for tool in git cargo rustup curl tar python3 brew hdiutil lipo codesign plutil; do
        require_tool "$tool"
    done
    require_tool brotli "brew install brotli"
    local llvm
    llvm="$(brew --prefix llvm 2> /dev/null)" || die "Homebrew llvm is not installed (brew install llvm)"
    [ -x "$llvm/bin/clang" ] || die "Homebrew llvm has no clang at $llvm/bin/clang"
    if [ "$mode" = release ]; then
        require_tool gh "brew install gh"
        gh auth status > /dev/null 2>&1 || die "gh is not authenticated (gh auth login)"
    fi

    local commit tools existing
    REPO="$(git rev-parse --show-toplevel)" || die "run this from inside a kyomi checkout"
    if [ "$mode" = release ]; then
        # gh reads the repository from GH_REPO; take it from origin, where the
        # tag is fetched from, not from whichever remote gh would guess.
        GH_REPO="$(origin_slug "$(git -C "$REPO" remote get-url origin)")"
        export GH_REPO
        git -C "$REPO" fetch --quiet origin "refs/tags/$ref:refs/tags/$ref" \
            || die "could not fetch tag $ref from origin (git's message is above)"
        # Check the release before the 45-minute build, not after it.
        gh release view "$ref" > /dev/null \
            || die "no GitHub release exists for $ref in $GH_REPO; push the tag and let release.yml create it first"
        existing="$(gh release view "$ref" --json assets --jq '[.assets[].name | select(endswith("_aarch64.dmg"))] | join(" ")')"
        [ -z "$existing" ] \
            || die "release $ref already has a Mac installer ($existing); delete that asset first if it must be replaced"
        commit="$(git -C "$REPO" rev-parse --verify "refs/tags/$ref^{commit}")"
    else
        commit="$(git -C "$REPO" rev-parse --verify "$ref^{commit}")" || die "$ref is not a git ref"
    fi

    WORK="$(mktemp -d -t kyomi-macos-release)"
    SRC="$WORK/src"
    tools="$WORK/tools"
    trap cleanup EXIT

    # Interrupting the run still removes the worktree: turn the signals into an
    # exit so the EXIT trap runs.
    trap 'terminate 130' INT
    trap 'terminate 143' TERM
    trap 'terminate 129' HUP

    echo "==> clean checkout of $ref ($commit)"
    git -C "$REPO" worktree add --quiet --detach "$SRC" "$commit"

    # The build environment CI had: the toolchain pinned by rust-toolchain.toml
    # (so nothing inherited from this shell picks another compiler, wrapper or
    # flags), Homebrew llvm first on PATH, and CI=true, which makes Tauri's dmg
    # bundler skip the Finder-scripted window styling exactly as it did on the
    # runner. Everything below runs from inside the checkout.
    unset RUSTUP_TOOLCHAIN RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_RUSTC_WRAPPER \
        RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET
    export PATH="$tools/bin:$llvm/bin:$PATH"
    export CC_wasm32_unknown_unknown="$llvm/bin/clang"
    export CARGO_TERM_COLOR=always
    export CI=true
    cd "$SRC"

    # Cheap and first: a stale lock, or a local [patch.crates-io] override in
    # ~/.cargo/config.toml (which changes resolution), stops the run before any
    # download or compile.
    echo "==> verify Cargo.lock is current"
    cargo metadata --locked --format-version 1 > /dev/null \
        || die "Cargo.lock is stale or a local [patch.crates-io] override is active; a release must build from the lock on the registry"

    # A dmg left in a reused target directory by an earlier build would make
    # "exactly one dmg" fail after the whole build, so clear it first.
    local target_dir
    target_dir="$(cargo metadata --locked --format-version 1 --no-deps \
        | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')" \
        || die "cannot read the cargo target directory"
    rm -f "$target_dir"/release/bundle/dmg/*_aarch64.dmg

    install_pinned_tools "$tools" "$SRC"
    strip_local_dev_config "$SRC"

    echo "==> trunk build --release (kyomi-ui)"
    run_in "$SRC/crates/kyomi-ui" trunk build --release

    echo "==> wasm-opt"
    local wasm
    for wasm in "$SRC"/crates/kyomi-ui/dist/*_bg.wasm; do
        [ -f "$wasm" ] || die "trunk produced no *_bg.wasm in crates/kyomi-ui/dist"
        wasm-opt --enable-bulk-memory-opt --enable-nontrapping-float-to-int -O -o "${wasm}.opt" "$wasm"
        mv "${wasm}.opt" "$wasm"
    done

    echo "==> MCP chart app"
    run_in "$SRC/apps/mcp-chart-app-wasm" bash build.sh

    echo "==> cargo tauri build --bundles dmg (fat LTO, release profile)"
    run_in "$SRC/apps/desktop" env CARGO_PROFILE_RELEASE_LTO=true cargo tauri build --bundles dmg

    local dmgs dmg
    dmgs=("$target_dir"/release/bundle/dmg/*_aarch64.dmg)
    [ "${#dmgs[@]}" -eq 1 ] && [ -f "${dmgs[0]}" ] \
        || die "expected exactly one *_aarch64.dmg in $target_dir/release/bundle/dmg, found ${#dmgs[@]}"
    dmg="${dmgs[0]}"

    echo "==> verify $dmg"
    verify_dmg "$dmg"

    if [ "$mode" = dry-run ]; then
        echo "==> dry run: built and verified $(basename "$dmg"); nothing uploaded"
        return 0
    fi

    echo "==> upload $(basename "$dmg") to release $ref"
    if ! gh release upload "$ref" "$dmg"; then
        # The work directory is deleted on exit, and it holds the only copy of a
        # 45-minute build: keep the dmg so the upload can be retried by hand.
        cp "$dmg" "$START_DIR/" || die "upload failed and the dmg could not be saved from $dmg"
        die "upload to $ref failed; the built dmg is saved as $START_DIR/$(basename "$dmg"). Check gh release view $ref --json assets for a partial asset first; then retry with: gh release upload $ref $START_DIR/$(basename "$dmg") (add --clobber only if a partial asset exists)"
    fi
    echo "==> done: $(basename "$dmg") attached to $ref"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
