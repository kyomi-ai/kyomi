#!/bin/bash

# Reset PostgreSQL database for Kyomi DEV environment (drops all data and recreates tables)
# Uses .env file for configuration
# Use with caution - all data will be lost!

set -e

# DEV environment - always use .env
REPO_ROOT="$(git rev-parse --show-toplevel)"
BACKEND_DIR="$REPO_ROOT/apps/backend"
ENV_FILE="$REPO_ROOT/.env"

cd "$BACKEND_DIR"

echo "🔥 RESETTING Kyomi DEV Database (ALL DATA WILL BE LOST)..."
echo "📁 Repo root: $REPO_ROOT"
echo "📁 Backend dir: $BACKEND_DIR"

# Load DEV environment variables
if [ ! -f "$ENV_FILE" ]; then
    echo "❌ Error: .env file not found"
    exit 1
fi

echo "📋 Loading environment variables from $ENV_FILE"
set -a
source "$ENV_FILE"
set +a
echo "✅ Environment variables loaded"

# Get database URL from environment or use default
DATABASE_URL=${DATABASE_URL:-"sqlite:///./kyomi.db"}

echo "📊 Database URL: $(echo $DATABASE_URL | sed 's/:[^:]*@/:***@/')"

if [[ $DATABASE_URL == postgresql* ]]; then
    echo "🐘 Resetting PostgreSQL database..."

    # Extract connection details
    if [[ $DATABASE_URL =~ postgresql://([^:]+):([^@]+)@([^:]+):([^/]+)/(.+) ]]; then
        DB_USER="${BASH_REMATCH[1]}"
        DB_PASSWORD="${BASH_REMATCH[2]}"
        DB_HOST="${BASH_REMATCH[3]}"
        DB_PORT="${BASH_REMATCH[4]}"
        DB_NAME="${BASH_REMATCH[5]}"

        echo "📝 Database: $DB_NAME on $DB_HOST:$DB_PORT"

        # Test connection
        if command -v psql &> /dev/null; then
            echo "🔍 Testing PostgreSQL connection..."
            if PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" -c "SELECT 1;" &> /dev/null; then
                echo "✅ PostgreSQL connection successful"

                # Drop all tables
                echo "🗑️  Dropping all tables..."
                PGPASSWORD="$DB_PASSWORD" psql -h "$DB_HOST" -p "$DB_PORT" -U "$DB_USER" -d "$DB_NAME" <<EOF
-- Drop all tables (including dependencies)
DROP SCHEMA public CASCADE;
CREATE SCHEMA public;

-- Restore default permissions
GRANT ALL ON SCHEMA public TO $DB_USER;
GRANT ALL ON SCHEMA public TO public;

-- Reinstall extensions
CREATE EXTENSION IF NOT EXISTS vector;
CREATE EXTENSION IF NOT EXISTS pg_trgm;
EOF
                echo "✅ All tables dropped"

            else
                echo "❌ Failed to connect to PostgreSQL"
                echo "Please ensure PostgreSQL is running and accessible"
                exit 1
            fi
        else
            echo "❌ psql not found, cannot reset database"
            exit 1
        fi
    else
        echo "❌ Invalid PostgreSQL URL format"
        exit 1
    fi

elif [[ $DATABASE_URL == sqlite* ]]; then
    echo "📁 Resetting SQLite database..."

    # Extract SQLite file path
    SQLITE_PATH=$(echo $DATABASE_URL | sed 's|sqlite:///||')

    # Delete SQLite file if it exists
    if [ -f "$SQLITE_PATH" ]; then
        rm "$SQLITE_PATH"
        echo "✅ Deleted SQLite database: $SQLITE_PATH"
    else
        echo "ℹ️  SQLite database does not exist yet"
    fi

else
    echo "❌ Unsupported database URL: $DATABASE_URL"
    exit 1
fi

# Recreate database tables using setup script
echo ""
echo "🏗️  Recreating database tables..."
"$REPO_ROOT/scripts/dev/setup-database.sh"

echo ""
echo "🎉 Database reset complete!"
echo ""
