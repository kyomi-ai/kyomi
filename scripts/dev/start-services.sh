#!/bin/bash
set -e

# Start PostgreSQL and Redis DEV containers
# PostgreSQL: port 5433 (prod uses 5432)
# Redis: port 6380 (prod uses 6379)

# Find the git repository root
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "❌ Error: Not in a git repository"
    exit 1
fi

cd "$REPO_ROOT"

echo "🚀 Starting Kyomi DEV services..."
echo "📁 Repo root: $REPO_ROOT"

# Load environment variables from .env file
if [ -f "$REPO_ROOT/.env" ]; then
    echo "📋 Loading environment variables from .env"
    set -a
    source "$REPO_ROOT/.env"
    set +a
fi

# Create db-init directory if it doesn't exist
if [ ! -d "./scripts/dev/db-init" ]; then
    echo "📁 Creating db-init directory..."
    mkdir -p ./scripts/dev/db-init
fi

# Start services with docker-compose
echo "🔄 Starting PostgreSQL and Redis..."
docker-compose -f docker-compose.dev.yml up -d

# Wait for PostgreSQL to be ready
echo "⏳ Waiting for PostgreSQL to be ready..."
timeout=60
counter=0

while [ $counter -lt $timeout ]; do
    if docker-compose -f docker-compose.dev.yml exec -T postgres pg_isready -U kyomi -d kyomi > /dev/null 2>&1; then
        echo "✅ PostgreSQL is ready!"
        break
    fi
    echo "   Waiting... ($counter/$timeout)"
    sleep 2
    counter=$((counter + 2))
done

if [ $counter -ge $timeout ]; then
    echo "❌ PostgreSQL failed to start within $timeout seconds"
    docker-compose -f docker-compose.dev.yml logs postgres | tail -20
    exit 1
fi

# Wait for Redis to be ready
echo "⏳ Waiting for Redis to be ready..."
counter=0

while [ $counter -lt $timeout ]; do
    if docker-compose -f docker-compose.dev.yml exec -T redis redis-cli ping > /dev/null 2>&1; then
        echo "✅ Redis is ready!"
        break
    fi
    echo "   Waiting... ($counter/$timeout)"
    sleep 2
    counter=$((counter + 2))
done

if [ $counter -ge $timeout ]; then
    echo "❌ Redis failed to start within $timeout seconds"
    docker-compose -f docker-compose.dev.yml logs redis | tail -20
    exit 1
fi

echo ""
echo "🎉 All DEV services are running!"
echo ""
echo "🐘 PostgreSQL:"
echo "   Host: localhost"
echo "   Port: 5433"
echo "   Database: kyomi"
echo "   Username: kyomi"
echo "   Password: password"
echo "   Data: docker volume (postgres_data_dev)"
echo ""
echo "🔴 Redis:"
echo "   Host: localhost"
echo "   Port: 6380"
echo "   URL: redis://localhost:6380/0"
echo "   Data: docker volume (redis_data_dev)"
echo ""
echo "📝 Useful commands:"
echo "   docker-compose -f docker-compose.dev.yml logs -f    # View logs"
echo "   docker-compose -f docker-compose.dev.yml ps         # Check status"
echo "   ./scripts/dev/stop-services.sh             # Stop services"
