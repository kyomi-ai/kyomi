#!/bin/bash
set -e

# Rust backend startup script - loads .env and runs cargo

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "Error: Not in a git repository"
    exit 1
fi

RUST_DIR="$REPO_ROOT/apps/backend-rust"

# Load environment variables from .env
if [ -f "$REPO_ROOT/.env" ]; then
    set -a
    source "$REPO_ROOT/.env"
    set +a
else
    echo "Error: .env file not found at $REPO_ROOT/.env"
    exit 1
fi

# Rust backend serves on the same port as Python backend (nginx expects 8002)
export PORT="${PORT:-8002}"

cd "$RUST_DIR"
exec cargo run "$@"
