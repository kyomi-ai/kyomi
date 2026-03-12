#!/bin/bash

# Setup PostgreSQL database for Kyomi PRODUCTION environment
# Runs inside the k8s postgres pod via kubectl exec

set -e

NAMESPACE="kyomi"

echo "Setting up Kyomi PRODUCTION Database..."

# Load environment
if [ -f ".env.production" ]; then
    source .env.production
fi

# Get database URL from environment or use default
DATABASE_URL=${DATABASE_URL:-"sqlite:///./kyomi.db"}

echo "Database URL: $(echo $DATABASE_URL | sed 's/:[^:]*@/:***@/')"

if [[ $DATABASE_URL == postgresql* ]]; then
    echo "Setting up PostgreSQL database..."

    # Get postgres pod name
    POD=$(kubectl get pod -n "$NAMESPACE" -l app=postgres -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
    if [ -z "$POD" ]; then
        echo "Error: PostgreSQL pod not found in namespace $NAMESPACE"
        echo ""
        echo "Ensure the postgres deployment is running:"
        echo "  kubectl get pods -n $NAMESPACE -l app=postgres"
        exit 1
    fi

    # Extract connection details
    if [[ $DATABASE_URL =~ postgresql://([^:]+):([^@]+)@([^:]+):([^/]+)/(.+) ]]; then
        DB_USER="${BASH_REMATCH[1]}"
        DB_NAME="${BASH_REMATCH[5]}"

        echo "Database: $DB_NAME"
        echo "Pod: $POD"

        # Test connection
        echo "Testing PostgreSQL connection..."
        if kubectl exec -n "$NAMESPACE" "$POD" -- psql -U "$DB_USER" -d "$DB_NAME" -c "SELECT 1;" &> /dev/null; then
            echo "PostgreSQL connection successful"
            echo "   (Extensions will be installed by Alembic migrations)"
        else
            echo "Failed to connect to PostgreSQL"
            exit 1
        fi
    else
        echo "Invalid PostgreSQL URL format"
        exit 1
    fi

    # Run migrations
    echo ""
    echo "Running database migrations..."
    BACKEND_POD=$(kubectl get pod -n "$NAMESPACE" -l app=backend -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
    if [ -n "$BACKEND_POD" ]; then
        kubectl exec -n "$NAMESPACE" "$BACKEND_POD" -- alembic upgrade head
    else
        echo "Warning: Backend pod not found, skipping migrations"
    fi
else
    echo "Unsupported database URL: $DATABASE_URL"
    echo "Production uses PostgreSQL: postgresql://user:password@host:port/database"
    exit 1
fi

echo ""
echo "Database setup complete!"
