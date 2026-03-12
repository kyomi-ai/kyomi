#!/bin/bash
set -e

# Stop all Kyomi DEV services

# Find the git repository root
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "❌ Error: Not in a git repository"
    exit 1
fi

cd "$REPO_ROOT"

echo "🛑 Stopping all Kyomi DEV services..."
echo "📁 Repo root: $REPO_ROOT"
echo ""

# Stop backend API
echo "1️⃣ Stopping backend API..."
pkill -f "target/release/kyomi" || echo "   (backend not running)"

# Stop frontend
echo "2️⃣ Stopping frontend dev server..."
pkill -f "vite.*5173" || echo "   (frontend not running)"

# Stop services (PostgreSQL + Redis)
echo "3️⃣ Stopping PostgreSQL and Redis..."
./scripts/dev/stop-services.sh

echo ""
echo "✅ All DEV services stopped!"
