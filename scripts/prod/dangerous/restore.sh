#!/bin/bash
set -e

NAMESPACE="kyomi"

if [ -z "$1" ]; then
    echo "Usage: ./scripts/prod/dangerous/restore.sh <backup-file.sql.gz>"
    echo ""
    echo "Available backups:"
    ls -lh backups/ 2>/dev/null || echo "No backups found"
    exit 1
fi

BACKUP_FILE="$1"

if [ ! -f "$BACKUP_FILE" ]; then
    echo "Error: Backup file not found: $BACKUP_FILE"
    exit 1
fi

echo "WARNING: This will restore the database from:"
echo "   $BACKUP_FILE"
echo ""
echo "Current data will be LOST. Are you sure? (type 'yes' to continue)"
read -r CONFIRM

if [ "$CONFIRM" != "yes" ]; then
    echo "Cancelled."
    exit 0
fi

# Load environment
if [ -f ".env.production" ]; then
    source .env.production
fi

# Get postgres pod name
POD=$(kubectl get pod -n "$NAMESPACE" -l app=postgres -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
if [ -z "$POD" ]; then
    echo "Error: PostgreSQL pod not found in namespace $NAMESPACE"
    exit 1
fi

echo "Restoring database from $BACKUP_FILE..."

# Decompress and restore
if [[ "$BACKUP_FILE" == *.gz ]]; then
    gunzip -c "$BACKUP_FILE" | kubectl exec -i -n "$NAMESPACE" "$POD" -- \
        psql -U "${POSTGRES_USER:-kyomi}" "${POSTGRES_DB:-kyomi}"
else
    cat "$BACKUP_FILE" | kubectl exec -i -n "$NAMESPACE" "$POD" -- \
        psql -U "${POSTGRES_USER:-kyomi}" "${POSTGRES_DB:-kyomi}"
fi

echo "Database restored successfully!"
