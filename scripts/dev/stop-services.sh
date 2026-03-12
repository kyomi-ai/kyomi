#!/bin/bash
set -e

# Stop PostgreSQL and Redis DEV containers
# Gracefully stops containers while preserving data

# Find the git repository root
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "❌ Error: Not in a git repository"
    exit 1
fi

cd "$REPO_ROOT"

echo "🛑 Stopping Kyomi DEV services..."

# Stop services with docker-compose
docker-compose -f docker-compose.dev.yml down

echo "✅ All DEV services stopped"
echo "💾 Data preserved in docker volumes:"
echo "   PostgreSQL: postgres_data_dev"
echo "   Redis: redis_data_dev"
echo ""
echo "📝 To start again: ./scripts/dev/start-services.sh"
