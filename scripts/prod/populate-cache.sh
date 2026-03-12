#!/bin/bash
set -e

NAMESPACE="kyomi"

echo "Populating BigQuery cache for production database..."

# Load environment and export all variables
if [ ! -f ".env.production" ]; then
    echo "Error: .env.production not found"
    exit 1
fi

set -a  # Mark all variables for export
source .env.production
set +a  # Stop marking variables for export

# Check if postgres pod is running
if ! kubectl get pod -n "$NAMESPACE" -l app=postgres -o jsonpath='{.items[0].status.phase}' 2>/dev/null | grep -q "Running"; then
    echo "Error: Production postgres pod is not running"
    exit 1
fi

# Get backend pod name
POD=$(kubectl get pod -n "$NAMESPACE" -l app=backend -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
if [ -z "$POD" ]; then
    echo "Error: Backend pod not found in namespace $NAMESPACE"
    exit 1
fi

echo "Running cache population inside backend pod..."
echo "This will populate the BigQuery public dataset cache"
echo ""

kubectl exec -n "$NAMESPACE" "$POD" -- \
    bash -c "cd /app/backend_scripts && python populate_cache.py"

EXIT_CODE=$?

if [ $EXIT_CODE -eq 0 ]; then
    echo ""
    echo "Cache population completed successfully!"
else
    echo ""
    echo "Cache population failed with exit code $EXIT_CODE"
    echo "Check logs: kubectl logs -n $NAMESPACE deployment/backend"
fi

exit $EXIT_CODE
