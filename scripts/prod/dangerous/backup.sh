#!/bin/bash
set -e

BACKUP_DIR="./backups"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="$BACKUP_DIR/kyomi_$TIMESTAMP.sql"
NAMESPACE="kyomi"

echo "Backing up database..."

# Create backup directory if it doesn't exist
mkdir -p "$BACKUP_DIR"

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

# Backup using kubectl exec
kubectl exec -n "$NAMESPACE" "$POD" -- \
    pg_dump -U "${POSTGRES_USER:-kyomi}" "${POSTGRES_DB:-kyomi}" > "$BACKUP_FILE"

# Verify backup has content before compressing
if [ ! -s "$BACKUP_FILE" ]; then
    echo "ERROR: Backup file is empty! Database may be empty or pg_dump failed."
    rm -f "$BACKUP_FILE"
    exit 1
fi

# Verify backup contains actual table data (not just schema)
TABLE_COUNT=$(grep -c "^CREATE TABLE" "$BACKUP_FILE" 2>/dev/null || echo "0")
if [ "$TABLE_COUNT" -eq 0 ]; then
    echo "WARNING: Backup contains no CREATE TABLE statements - database may be empty"
fi

# Compress backup
gzip "$BACKUP_FILE"

# Verify compressed file exists and has content
if [ ! -s "$BACKUP_FILE.gz" ]; then
    echo "ERROR: Compressed backup file is empty or missing!"
    exit 1
fi

BACKUP_SIZE=$(du -h "$BACKUP_FILE.gz" | cut -f1)
echo "Backup saved to: $BACKUP_FILE.gz ($BACKUP_SIZE, $TABLE_COUNT tables)"

# Delete backups older than 30 days
DELETED=$(find "$BACKUP_DIR" -name "kyomi_*.sql.gz" -mtime +30 -delete -print | wc -l)
if [ "$DELETED" -gt 0 ]; then
    echo "Deleted $DELETED backup(s) older than 30 days"
fi
