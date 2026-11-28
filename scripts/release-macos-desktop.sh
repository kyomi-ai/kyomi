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
# authenticated for the repository.
#
# The Mac needs: Apple silicon; at least 16 GB of RAM (the build's largest
# process peaked at 13.7 GB on a 32 GB M1 Max, the machine it was tested on); and
# 20 GB of free disk (a cold build used 14 GB: 11.9 GB of build products, 2.0 GB
# of crates, 0.2 GB of tools, all inside the temporary directory, or in
# CARGO_TARGET_DIR when that is set). A cold build took 43 minutes on that
# otherwise idle Mac; expect longer on a smaller Mac or one that is busy.
# `cargo install tauri-cli --version "^2" --locked` is compiled from source, as
# in CI, so the newest 2.x release that its lock allows is used, not a pinned one.
# Set CARGO_TARGET_DIR to an absolute path to keep build products between runs.
# If the final upload fails, the built dmg is copied to the current directory
# so the upload can be retried without a rebuild.
#
# The build must not inherit anything that changes what is shipped. It runs
# with a fresh, empty CARGO_HOME inside the work directory (as on a CI runner,
# so no user-level cargo config is read and every crate is downloaded again),
# with no rustc wrapper (sccache included; CI used sccache only for speed), and
# refuses to start if a cargo config file exists above the checkout or a
# CARGO_PROFILE_*, CARGO_BUILD_*, CARGO_TARGET_*, CARGO_ALIAS_*, TAURI_* (for
# example TAURI_CONFIG, which rewrites tauri.conf.json) or per-target compiler
# (CC_*, CFLAGS_*, ...) variable that changes the build is set.
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
run_in() {
    local dir="$1" status=0
    shift
    set -m
    (cd "$dir" && exec "$@") < /dev/null &
    wait "$!" || status=$?
    set +m
    return "$status"
}

# terminate <exit-code>: the INT/TERM/HUP handler. Under `set -m` every job's
# process group id is the job's pid, which `jobs -p` lists, so the handler needs
# no variable that could still be unset when the signal arrives.
terminate() {
    local group
    for group in $(jobs -p); do
        kill -TERM -- "-$group" 2> /dev/null || true
    done
    wait 2> /dev/null || true
    exit "$1"
}

# The build gets a fresh, empty CARGO_HOME inside the work directory, which is
# what a CI runner has: cargo then reads no user-level config (no profile,
# rustflags, target, linker, patch, source, env or include, however it is
# spelled) and the crates are downloaded again. This replaces trying to
# recognise dangerous settings in a user's files.
use_clean_cargo_home() {
    mkdir -p "$1/cargo-home" || die "cannot create $1/cargo-home"
    export CARGO_HOME="$1/cargo-home"
}

# check_cargo_config <checkout> refuses to continue when cargo would still read a
# setting from somewhere a clean CARGO_HOME does not cover: any cargo config file
# in a directory above <checkout> (cargo merges those too), or a CARGO_PROFILE_*,
# CARGO_BUILD_*, CARGO_TARGET_* or CARGO_ALIAS_* variable, a TAURI_* variable, or a per-target
# compiler variable (CC_*, CXX_*, CFLAGS_*, CXXFLAGS_*, CPPFLAGS_*, LDFLAGS_*), that changes the build. The variables
# that cannot change the output are allowed: CARGO_TARGET_DIR and
# CARGO_BUILD_TARGET_DIR only move the products, CARGO_BUILD_JOBS only sets
# parallelism, and CARGO_BUILD_RUSTC_WRAPPER is cleared by main. The checkout's
# own .cargo/config.toml is reviewed source and is allowed.
check_cargo_config() {
    local checkout="$1" dir file name
    dir="$(dirname "$checkout")"
    while :; do
        for file in "$dir/.cargo/config.toml" "$dir/.cargo/config"; do
            [ ! -f "$file" ] || die "cargo would read $file, which can change the build; a release must build from the checkout's own settings. Move that file aside for the build."
        done
        [ "$dir" = / ] && break
        dir="$(dirname "$dir")"
    done
    for name in $(env | cut -d= -f1 | grep -E '^CARGO_(PROFILE|BUILD|TARGET|ALIAS)_' || true); do
        case "$name" in CARGO_TARGET_DIR | CARGO_BUILD_TARGET_DIR | CARGO_BUILD_JOBS | CARGO_BUILD_RUSTC_WRAPPER) continue ;; esac
        die "$name is set; it changes the build, and a release must build from the checkout's own settings. Unset it for the build."
    done
    # Tauri reads TAURI_CONFIG (a JSON merge into tauri.conf.json: bundle settings,
    # resources, version) and other TAURI_* variables, which would change the app
    # while the identifier check still passed. Signing identities (APPLE_*) are
    # caught by the ad-hoc signature check after the build.
    for name in $(env | cut -d= -f1 | grep -E '^TAURI_' || true); do
        die "$name is set; Tauri reads it and it can change the bundle, and a release must build from the checkout's own settings. Unset it for the build."
    done
    # cc-rs reads per-target compiler variables such as CFLAGS_aarch64_apple_darwin
    # and CFLAGS_aarch64-apple-darwin. The hyphenated names cannot be unset from
    # bash (3.2 aborts on them), so they are refused, not cleared. main sets
    # CC_wasm32_unknown_unknown itself, as CI did, so that one is allowed.
    for name in $(env | cut -d= -f1 | grep -E '^(CC|CXX|CFLAGS|CXXFLAGS|CPPFLAGS|LDFLAGS)_' || true); do
        [ "$name" = CC_wasm32_unknown_unknown ] && continue
        die "$name is set; it changes how native code is compiled, and a release must build from the checkout's own settings. Unset it for the build."
    done
}

# check_release_ready <tag> stops before the build when the release does not
# exist or already carries a Mac installer. Needs gh and GH_REPO.
check_release_ready() {
    local ref="$1" existing
    gh release view "$ref" > /dev/null \
        || die "no GitHub release exists for $ref in ${GH_REPO:-this repository}; push the tag and let release.yml create it first"
    existing="$(gh release view "$ref" --json assets --jq '[.assets[].name | select(endswith("_aarch64.dmg"))] | join(" ")')" \
        || die "cannot list the assets of release $ref"
    [ -z "$existing" ] \
        || die "release $ref already has a Mac installer ($existing); delete that asset first if it must be replaced"
}

# upload_release_asset <tag> <dmg>: the work directory is deleted on exit and
# holds the only copy of a long build, so a failed upload saves the dmg first.
upload_release_asset() {
    local ref="$1" dmg="$2"
    if ! run_in "$PWD" gh release upload "$ref" "$dmg"; then
        cp "$dmg" "$START_DIR/" || die "upload failed and the dmg could not be saved from $dmg"
        die "upload to $ref failed; the built dmg is saved as $START_DIR/$(basename "$dmg"). Check gh release view $ref --json assets for a partial asset first; then retry with: gh release upload $ref $START_DIR/$(basename "$dmg") (add --clobber only if a partial asset exists)"
    fi
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

# The mountpoint lives under $WORK, and cleanup detaches it if it is mounted, so an
# interrupted or failed run never leaves the dmg attached.
verify_dmg() {
    local dmg="$1" status=0
    local mountpoint="$WORK/dmg-mount"
    mkdir -p "$mountpoint"
    hdiutil attach -readonly -nobrowse -mountpoint "$mountpoint" "$dmg" > /dev/null \
        || die "cannot mount $dmg"
    (inspect_app "$mountpoint/Kyomi.app" "$dmg") || status=$?
    hdiutil detach "$mountpoint" > /dev/null || die "cannot detach $mountpoint"
    [ "$status" -eq 0 ] || die "verification of $dmg failed"
}

# Runs on every exit, including failed runs. Each step is best effort because
# cleanup must not hide the error that ended the run; what it cannot remove is
# named, never silently left.
cleanup() {
    set +e # an EXIT trap runs under the script's `set -e`; every step must run
    cd "$START_DIR" || cd /
    # Ask the system, not a variable: a signal can arrive between the attach and
    # any bookkeeping about it. $WORK is a physical path, as `mount` lists it.
    if mount | grep -qF " on $WORK/dmg-mount "; then
        if ! hdiutil detach -force "$WORK/dmg-mount" > /dev/null 2>&1; then
            printf 'release-macos-desktop: could not detach %s; leaving %s in place\n' "$WORK/dmg-mount" "$WORK" >&2
            return 0
        fi
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
    parsed="$(parse_args "$@")" || exit $?
    mode="${parsed%% *}"
    ref="${parsed#* }"
    case "${CARGO_TARGET_DIR:-/}" in
        /*) ;;
        *) die "CARGO_TARGET_DIR must be an absolute path (the build runs from inside the checkout): $CARGO_TARGET_DIR" ;;
    esac

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

    local commit tools
    REPO="$(git rev-parse --show-toplevel)" || die "run this from inside a kyomi checkout"
    if [ "$mode" = release ]; then
        # gh reads the repository from GH_REPO; take it from origin, where the
        # tag is fetched from, not from whichever remote gh would guess.
        GH_REPO="$(origin_slug "$(git -C "$REPO" remote get-url origin)")"
        export GH_REPO
        git -C "$REPO" fetch --quiet origin "refs/tags/$ref:refs/tags/$ref" \
            || die "could not fetch tag $ref from origin (git's message is above)"
        # Check the release before the long build, not after it.
        check_release_ready "$ref"
        commit="$(git -C "$REPO" rev-parse --verify "refs/tags/$ref^{commit}")"
    else
        commit="$(git -C "$REPO" rev-parse --verify "$ref^{commit}")" || die "$ref is not a git ref"
    fi

    WORK="$(cd "$(mktemp -d -t kyomi-macos-release)" && pwd -P)"
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
    # (so nothing inherited from this shell picks another compiler or flags),
    # Homebrew llvm first on PATH, and CI=true, which makes Tauri's dmg bundler
    # skip the Finder-scripted window styling exactly as it did on the runner.
    # Settings that could change the compiled output are refused (see
    # check_cargo_config), cleared, or kept out of reach by the clean CARGO_HOME
    # (so a user's ~/.cargo/config.toml, with a wrapper such as sccache or any
    # profile or flags, is never read); the rustc wrapper is also set empty (CI
    # used sccache only to go faster, and this build uses none). Everything below
    # runs from inside the checkout.
    check_cargo_config "$SRC"
    use_clean_cargo_home "$WORK"
    unset RUSTUP_TOOLCHAIN RUSTC RUSTDOC RUSTC_BOOTSTRAP RUSTC_WORKSPACE_WRAPPER RUSTFLAGS \
        CARGO_ENCODED_RUSTFLAGS RUSTDOCFLAGS CARGO_ENCODED_RUSTDOCFLAGS \
        CC CXX CFLAGS CXXFLAGS CPPFLAGS LDFLAGS SDKROOT MACOSX_DEPLOYMENT_TARGET CRATE_CC_NO_DEFAULTS
    export CARGO_INCREMENTAL=0 # as on the CI runner (dtolnay/rust-toolchain sets it)
    export RUSTC_WRAPPER="" CARGO_BUILD_RUSTC_WRAPPER=""
    export PATH="$tools/bin:$llvm/bin:$PATH"
    export CC_wasm32_unknown_unknown="$llvm/bin/clang"
    export CARGO_TERM_COLOR=always
    export CI=true
    cd "$SRC"

    # First, before the long compile: this downloads the registry index and the
    # crates' manifests into the clean CARGO_HOME, and fails if Cargo.lock would
    # have to change.
    echo "==> verify Cargo.lock is current"
    cargo metadata --locked --format-version 1 > /dev/null \
        || die "cargo metadata --locked failed (see cargo's message above): the lock is stale or the registry is unreachable; a release must build from the committed lock"

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
        run_in "$SRC" wasm-opt --enable-bulk-memory-opt --enable-nontrapping-float-to-int -O -o "${wasm}.opt" "$wasm"
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
    echo "==> disk used (KB): build products $(du -sk "$target_dir" | cut -f1), crates $(du -sk "$CARGO_HOME" | cut -f1), tools $(du -sk "$tools" | cut -f1)"

    if [ "$mode" = dry-run ]; then
        echo "==> dry run: built and verified $(basename "$dmg"); nothing uploaded"
        return 0
    fi

    echo "==> upload $(basename "$dmg") to release $ref"
    upload_release_asset "$ref" "$dmg"
    echo "==> done: $(basename "$dmg") attached to $ref"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
