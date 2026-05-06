#!/bin/bash
# Start Rust backend with Kyomi Connect support

set -e

cd "$(dirname "$0")/.."

echo "Loading environment..."
set -a
source .env
set +a

export PORT="${PORT:-8002}"

echo "Starting Kyomi Rust backend on port $PORT..."
exec target/release/kyomi
