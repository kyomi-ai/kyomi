#!/bin/bash
# Start Rust backend with Kyomi Connect support

set -e

cd "$(dirname "$0")/.."

echo "Loading environment..."
set -a
source .env
set +a

export PORT="${PORT:-8002}"
export CONNECT_JWT_PRIVATE_KEY="$(cat apps/backend-rust/connect-key.pem)"

echo "Starting Kyomi Rust backend on port $PORT..."
exec apps/backend-rust/target/release/kyomi-api
