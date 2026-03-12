#!/bin/bash

# Restart frontend dev server

echo "🔄 Restarting Kyomi Frontend..."

# Get the monorepo root directory
MONOREPO_ROOT="$(git rev-parse --show-toplevel)"

# Stop frontend (kill all Vite processes)
echo "Stopping all Vite processes..."
pkill -9 -f "vite" 2>/dev/null || true
lsof -ti:5173 | xargs kill -9 2>/dev/null || true

# Wait a moment for processes to stop
sleep 2

# Start frontend with proper environment
echo "Starting frontend on port 5173..."
exec "$MONOREPO_ROOT/scripts/dev/start-frontend.sh"
