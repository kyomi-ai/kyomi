#!/bin/bash
set -e

# Start all Kyomi DEV services (postgres, redis, backend, frontend)

# Find the git repository root
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "❌ Error: Not in a git repository"
    exit 1
fi

cd "$REPO_ROOT"

echo "🚀 Starting all Kyomi DEV services..."
echo "📁 Repo root: $REPO_ROOT"
echo ""

# Start services (PostgreSQL + Redis)
echo "1️⃣ Starting PostgreSQL and Redis..."
./scripts/dev/start-services.sh

# Start backend
echo ""
echo "2️⃣ Starting backend API..."
./scripts/dev/start-rust-backend.sh

# Start frontend
echo ""
echo "3️⃣ Starting frontend dev server..."
./scripts/dev/start-frontend.sh

echo ""
echo "✅ All DEV services started!"
echo "   Frontend: http://localhost:5173"
echo "   Backend:  http://localhost:8002"
echo "   PostgreSQL: localhost:5433"
echo "   Redis: localhost:6380"
