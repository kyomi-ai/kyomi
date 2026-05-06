#!/bin/sh
set -e

# Kyomi Connect installer
# Usage: curl -fsSL https://connect.kyomi.ai/install.sh | sh

INSTALL_DIR="/usr/local/bin"
BINARY_NAME="kyomi-connect"
BASE_URL="https://connect.kyomi.ai/download"

# Parse arguments
TOKEN=""
while [ $# -gt 0 ]; do
    case "$1" in
        --token)
            TOKEN="$2"
            shift 2
            ;;
        --token=*)
            TOKEN="${1#--token=}"
            shift
            ;;
        *)
            shift
            ;;
    esac
done

# Clean up temp directory on exit
cleanup() {
    if [ -n "$TMPDIR" ] && [ -d "$TMPDIR" ]; then
        rm -rf "$TMPDIR"
    fi
}
trap cleanup EXIT

# Detect OS
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
case "$OS" in
    linux) OS="linux" ;;
    darwin) OS="macos" ;;
    *) echo "Error: Unsupported OS: $OS"; exit 1 ;;
esac

# Detect architecture
ARCH=$(uname -m)
case "$ARCH" in
    x86_64|amd64) ARCH="amd64" ;;
    aarch64|arm64) ARCH="arm64" ;;
    *) echo "Error: Unsupported architecture: $ARCH"; exit 1 ;;
esac

echo "  Kyomi Connect Installer"
echo "  OS: $OS, Arch: $ARCH"
echo ""

# Determine download tool
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO- "$1"; }
else
    echo "Error: curl or wget is required"
    exit 1
fi

# Download URL pattern
DOWNLOAD_URL="${BASE_URL}/${BINARY_NAME}-${OS}-${ARCH}"
CHECKSUM_URL="${DOWNLOAD_URL}.sha256"

echo "  Downloading ${BINARY_NAME}..."
TMPDIR=$(mktemp -d)
fetch "$DOWNLOAD_URL" > "$TMPDIR/$BINARY_NAME"

# Verify checksum if available
CHECKSUM_DOWNLOADED=false
if fetch "$CHECKSUM_URL" > "$TMPDIR/$BINARY_NAME.sha256" 2>/dev/null; then
    if [ -s "$TMPDIR/$BINARY_NAME.sha256" ]; then
        CHECKSUM_DOWNLOADED=true
    fi
fi

if [ "$CHECKSUM_DOWNLOADED" = true ]; then
    echo "  Verifying checksum..."
    EXPECTED=$(cat "$TMPDIR/$BINARY_NAME.sha256" | awk '{print $1}')
    if command -v sha256sum >/dev/null 2>&1; then
        ACTUAL=$(sha256sum "$TMPDIR/$BINARY_NAME" | awk '{print $1}')
    elif command -v shasum >/dev/null 2>&1; then
        ACTUAL=$(shasum -a 256 "$TMPDIR/$BINARY_NAME" | awk '{print $1}')
    else
        echo "  Warning: No sha256sum or shasum found, skipping checksum verification"
        ACTUAL="$EXPECTED"
    fi

    if [ "$EXPECTED" != "$ACTUAL" ]; then
        echo "Error: Checksum verification failed"
        echo "  Expected: $EXPECTED"
        echo "  Actual:   $ACTUAL"
        exit 1
    fi
    echo "  Checksum verified"
else
    echo "  Warning: No checksum file available, skipping verification"
fi

# Install
chmod +x "$TMPDIR/$BINARY_NAME"

if [ -w "$INSTALL_DIR" ]; then
    mv "$TMPDIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
else
    echo "  Installing to $INSTALL_DIR (requires sudo)..."
    sudo mv "$TMPDIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
fi

echo ""
echo "  Kyomi Connect installed to $INSTALL_DIR/$BINARY_NAME"

echo ""
echo "  Running setup..."
echo ""
if [ -n "$TOKEN" ]; then
    exec "$INSTALL_DIR/$BINARY_NAME" setup --token "$TOKEN" < /dev/tty
else
    exec "$INSTALL_DIR/$BINARY_NAME" setup < /dev/tty
fi
