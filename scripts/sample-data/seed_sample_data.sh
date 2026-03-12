#!/bin/bash
# Seed sample data for Acme Analytics trial database
#
# This script:
# 1. Creates the database and tables
# 2. Generates sample data
# 3. Creates a read-only user for trial access
#
# Prerequisites:
# - ClickHouse server running
# - Python 3.8+ with clickhouse-connect installed
# - clickhouse-client installed (for DDL)
#
# Usage:
#   ./seed_sample_data.sh [--host HOST] [--port PORT] [--user USER] [--password PASSWORD]

set -e

# Default configuration
CLICKHOUSE_HOST="${CLICKHOUSE_HOST:-localhost}"
CLICKHOUSE_PORT="${CLICKHOUSE_PORT:-9000}"
CLICKHOUSE_HTTP_PORT="${CLICKHOUSE_HTTP_PORT:-8123}"
CLICKHOUSE_USER="${CLICKHOUSE_USER:-default}"
CLICKHOUSE_PASSWORD="${CLICKHOUSE_PASSWORD:-}"
DATABASE="acme_analytics"
READONLY_USER="sample_readonly"
READONLY_PASSWORD="${SAMPLE_CLICKHOUSE_PASSWORD:-readonly_trial_2024}"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --host)
            CLICKHOUSE_HOST="$2"
            shift 2
            ;;
        --port)
            CLICKHOUSE_PORT="$2"
            shift 2
            ;;
        --http-port)
            CLICKHOUSE_HTTP_PORT="$2"
            shift 2
            ;;
        --user)
            CLICKHOUSE_USER="$2"
            shift 2
            ;;
        --password)
            CLICKHOUSE_PASSWORD="$2"
            shift 2
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

echo "=== Seeding Acme Analytics Sample Database ==="
echo "Host: $CLICKHOUSE_HOST"
echo "Port: $CLICKHOUSE_PORT (native), $CLICKHOUSE_HTTP_PORT (HTTP)"
echo "User: $CLICKHOUSE_USER"
echo ""

# Function to run ClickHouse SQL
run_sql() {
    local sql="$1"
    if [ -n "$CLICKHOUSE_PASSWORD" ]; then
        clickhouse-client --host "$CLICKHOUSE_HOST" --port "$CLICKHOUSE_PORT" \
            --user "$CLICKHOUSE_USER" --password "$CLICKHOUSE_PASSWORD" \
            --query "$sql"
    else
        clickhouse-client --host "$CLICKHOUSE_HOST" --port "$CLICKHOUSE_PORT" \
            --user "$CLICKHOUSE_USER" \
            --query "$sql"
    fi
}

# Step 1: Create database and tables
echo "1. Creating database and tables..."
if [ -n "$CLICKHOUSE_PASSWORD" ]; then
    clickhouse-client --host "$CLICKHOUSE_HOST" --port "$CLICKHOUSE_PORT" \
        --user "$CLICKHOUSE_USER" --password "$CLICKHOUSE_PASSWORD" \
        --multiquery < "$SCRIPT_DIR/create_sample_database.sql"
else
    clickhouse-client --host "$CLICKHOUSE_HOST" --port "$CLICKHOUSE_PORT" \
        --user "$CLICKHOUSE_USER" \
        --multiquery < "$SCRIPT_DIR/create_sample_database.sql"
fi
echo "   Done."

# Step 2: Generate and insert sample data
echo ""
echo "2. Generating and inserting sample data..."
python3 "$SCRIPT_DIR/generate_sample_data.py" \
    --clickhouse-host "$CLICKHOUSE_HOST" \
    --clickhouse-port "$CLICKHOUSE_HTTP_PORT" \
    --clickhouse-user "$CLICKHOUSE_USER" \
    --clickhouse-password "$CLICKHOUSE_PASSWORD" \
    --database "$DATABASE"
echo "   Done."

# Step 3: Create read-only user
echo ""
echo "3. Creating read-only user for trial access..."
run_sql "CREATE USER IF NOT EXISTS $READONLY_USER IDENTIFIED BY '$READONLY_PASSWORD'" || true
run_sql "GRANT SELECT ON $DATABASE.* TO $READONLY_USER" || true
echo "   User: $READONLY_USER"
echo "   Password: $READONLY_PASSWORD"
echo "   Done."

# Step 4: Verify data
echo ""
echo "4. Verifying data..."
SUB_COUNT=$(run_sql "SELECT count() FROM $DATABASE.subscriptions")
USER_COUNT=$(run_sql "SELECT count() FROM $DATABASE.users")
EVENT_COUNT=$(run_sql "SELECT count() FROM $DATABASE.events")
SESSION_COUNT=$(run_sql "SELECT count() FROM $DATABASE.website_sessions")

echo "   Subscriptions: $SUB_COUNT"
echo "   Users: $USER_COUNT"
echo "   Events: $EVENT_COUNT"
echo "   Website Sessions: $SESSION_COUNT"

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Environment variables for your .env file:"
echo ""
echo "SAMPLE_CLICKHOUSE_HOST=$CLICKHOUSE_HOST"
echo "SAMPLE_CLICKHOUSE_PORT=$CLICKHOUSE_PORT"
echo "SAMPLE_CLICKHOUSE_DATABASE=$DATABASE"
echo "SAMPLE_CLICKHOUSE_USER=$READONLY_USER"
echo "SAMPLE_CLICKHOUSE_PASSWORD=$READONLY_PASSWORD"
echo ""
echo "Test the read-only user:"
echo "  clickhouse-client --host $CLICKHOUSE_HOST --port $CLICKHOUSE_PORT \\"
echo "    --user $READONLY_USER --password '$READONLY_PASSWORD' \\"
echo "    --query 'SELECT count() FROM $DATABASE.subscriptions'"
