#!/bin/bash
# Seed trial sample data for production
#
# Run this ONCE after first deploying trial-clickhouse.
# The data persists in the volume, so no need to re-run.
#
# Prerequisites:
# - trial-clickhouse pod running in k8s
# - clickhouse-client installed on host
# - Python 3.8+ with clickhouse-connect
#
# Usage:
#   ./scripts/prod/seed-trial-data.sh

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
NAMESPACE="kyomi"

# Use kubectl port-forward to access clickhouse
CLICKHOUSE_HOST="localhost"
CLICKHOUSE_PORT="9002"
CLICKHOUSE_HTTP_PORT="8127"

# Load password from .env.production if available
if [ -f "$REPO_DIR/.env.production" ]; then
    source "$REPO_DIR/.env.production"
fi

SAMPLE_CLICKHOUSE_PASSWORD="${SAMPLE_CLICKHOUSE_PASSWORD:-readonly_trial_2024}"

echo "=== Seeding Trial Sample Data for Production ==="
echo ""
echo "This will populate the trial-clickhouse pod with sample data."
echo "Host: $CLICKHOUSE_HOST"
echo "Native Port: $CLICKHOUSE_PORT"
echo "HTTP Port: $CLICKHOUSE_HTTP_PORT"
echo ""

# Check if clickhouse-client is installed
if ! command -v clickhouse-client &> /dev/null; then
    echo "ERROR: clickhouse-client not found."
    echo "Install it with: sudo dnf install clickhouse-client"
    echo "Or: curl https://clickhouse.com/ | sh"
    exit 1
fi

# Check if trial-clickhouse pod is running
if ! kubectl get pod -n "$NAMESPACE" -l app=trial-clickhouse -o jsonpath='{.items[0].status.phase}' 2>/dev/null | grep -q "Running"; then
    echo "ERROR: trial-clickhouse pod is not running in namespace $NAMESPACE."
    echo "Check status: kubectl get pods -n $NAMESPACE -l app=trial-clickhouse"
    exit 1
fi

# Set up port-forward in background
echo "Setting up port-forward to trial-clickhouse..."
kubectl port-forward -n "$NAMESPACE" deployment/trial-clickhouse "$CLICKHOUSE_PORT:9000" "$CLICKHOUSE_HTTP_PORT:8123" &
PF_PID=$!
trap "kill $PF_PID 2>/dev/null" EXIT
sleep 2

# Run the seed script
"$REPO_DIR/scripts/sample-data/seed_sample_data.sh" \
    --host "$CLICKHOUSE_HOST" \
    --port "$CLICKHOUSE_PORT" \
    --http-port "$CLICKHOUSE_HTTP_PORT"

echo ""
echo "=== Done ==="
echo "Trial sample data is now available."
