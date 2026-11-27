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
# <tag> is a release tag (vMAJOR.MINOR.PATCH, for example v2.7.17) whose GitHub release already exists
# (release.yml creates it). Run from a logged-in Mac session with `gh`
# authenticated for the repository. A release build takes about 45 minutes
# plus the one-off compile of the pinned tauri-cli. Set CARGO_TARGET_DIR to
# keep build products between runs; otherwise every run compiles from scratch.
#
# Exit codes: 0 success; 2 usage error; 1 any other failure. Nothing here
# falls back: a missing tool, a version mismatch or a failed step stops the run.
# ------------------------------------------------------------------------------

set -euo pipefail

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
    cargo install tauri-cli --version "^2" --locked --root "$tools"

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

    lipo -info "$binary"
    archs="$(lipo -archs "$binary")"
    [ "$archs" = arm64 ] || die "the app binary is '$archs', not arm64 only"

    (cd "$app" && find . -type f | LC_ALL=C sort)
    plutil -p "$app/Contents/Info.plist"
    codesign --verify --deep --strict "$app"
    signature="$(codesign -dvv "$app" 2>&1)"
    printf '%s\n' "$signature"
    printf '%s\n' "$signature" | grep -q '^Signature=adhoc$' \
        || die "the app is not ad-hoc signed (CI signed it with identity \"-\")"
    printf '%s\n' "$signature" | grep -q '^Identifier=ai.kyomi.desktop$' \
        || die "the app's identifier is not ai.kyomi.desktop"
    du -sk "$dmg" "$app" "$binary"
}

verify_dmg() {
    local dmg="$1" mount status=0
    mount="$(mktemp -d -t kyomi-dmg)"
    hdiutil attach -readonly -nobrowse -mountpoint "$mount" "$dmg" > /dev/null
    (inspect_app "$mount/Kyomi.app" "$dmg") || status=$?
    hdiutil detach "$mount" > /dev/null
    rmdir "$mount"
    [ "$status" -eq 0 ] || die "verification of $dmg failed"
}

cleanup() {
    git -C "$REPO" worktree remove --force "$SRC" > /dev/null 2>&1 || true
    rm -rf "$WORK"
}

main() {
    local parsed mode ref
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

    local commit tools
    REPO="$(git rev-parse --show-toplevel)" || die "run this from inside a kyomi checkout"
    if [ "$mode" = release ]; then
        git -C "$REPO" fetch --quiet origin "refs/tags/$ref:refs/tags/$ref" \
            || die "tag $ref is not on origin"
        gh release view "$ref" > /dev/null 2>&1 \
            || die "no GitHub release exists for $ref; push the tag and let release.yml create it first"
        commit="$(git -C "$REPO" rev-parse --verify "refs/tags/$ref^{commit}")"
    else
        commit="$(git -C "$REPO" rev-parse --verify "$ref^{commit}")" || die "$ref is not a git ref"
    fi

    WORK="$(mktemp -d -t kyomi-macos-release)"
    SRC="$WORK/src"
    tools="$WORK/tools"
    trap cleanup EXIT

    echo "==> clean checkout of $ref ($commit)"
    git -C "$REPO" worktree add --quiet --detach "$SRC" "$commit"

    install_pinned_tools "$tools" "$SRC"
    strip_local_dev_config "$SRC"

    # The build environment CI had: pinned toolchain from rust-toolchain.toml
    # (so no inherited override), Homebrew llvm first on PATH, and CI=true,
    # which makes Tauri's dmg bundler skip the Finder-scripted window styling
    # exactly as it did on the runner.
    unset RUSTUP_TOOLCHAIN RUSTC_WRAPPER
    export PATH="$tools/bin:$llvm/bin:$PATH"
    export CC_wasm32_unknown_unknown="$llvm/bin/clang"
    export CARGO_TERM_COLOR=always
    export CI=true

    echo "==> verify Cargo.lock is current"
    (cd "$SRC" && cargo metadata --locked --format-version 1 > /dev/null)

    echo "==> trunk build --release (kyomi-ui)"
    (cd "$SRC/crates/kyomi-ui" && trunk build --release)

    echo "==> wasm-opt"
    local wasm
    for wasm in "$SRC"/crates/kyomi-ui/dist/*_bg.wasm; do
        [ -f "$wasm" ] || die "trunk produced no *_bg.wasm in crates/kyomi-ui/dist"
        wasm-opt --enable-bulk-memory-opt --enable-nontrapping-float-to-int -O -o "${wasm}.opt" "$wasm"
        mv "${wasm}.opt" "$wasm"
    done

    echo "==> MCP chart app"
    (cd "$SRC/apps/mcp-chart-app-wasm" && bash build.sh)

    echo "==> cargo tauri build --bundles dmg (fat LTO, release profile)"
    (cd "$SRC/apps/desktop" && CARGO_PROFILE_RELEASE_LTO=true cargo tauri build --bundles dmg)

    local target_dir dmgs dmg
    target_dir="$(cd "$SRC" && cargo metadata --locked --format-version 1 --no-deps \
        | python3 -I -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
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
    gh release upload "$ref" "$dmg"
    echo "==> done: $(basename "$dmg") attached to $ref"
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    main "$@"
fi
