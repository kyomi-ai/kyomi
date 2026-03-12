#!/bin/bash
set -e

# Frontend startup script - loads .env and starts the dev server

# Get the monorepo root directory
MONOREPO_ROOT="$(git rev-parse --show-toplevel)"
FRONTEND_DIR="$MONOREPO_ROOT/apps/frontend"

cd "$FRONTEND_DIR"

echo "🚀 Starting Kyomi Frontend..."
echo "📁 Frontend dir: $FRONTEND_DIR"

# Load environment variables from .env file
if [ -f .env.development ]; then
    echo "📋 Loading environment variables from .env.development"
    export $(cat .env.development | grep -v '^#' | xargs)
    echo "✅ Environment variables loaded"
else
    echo "⚠️  No .env.development file found, using defaults"
fi

# Start the dev server
echo "🔧 Starting Vite dev server..."
exec npm run dev