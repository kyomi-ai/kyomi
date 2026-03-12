#!/bin/bash
# Index BigQuery public datasets in production

set -e

# Load env vars from .env.production
if [ ! -f ".env.production" ]; then
    echo "❌ Error: .env.production not found"
    exit 1
fi

source .env.production

echo "🚀 Indexing BigQuery public datasets..."
echo "📍 Using your local gcloud application-default credentials"
echo "🗄️  Connecting to production database at localhost:5432"
echo ""

# Run locally using local gcloud credentials and prod database
cd apps/backend
DATABASE_URL="postgresql://${POSTGRES_USER:-kyomi}:${POSTGRES_PASSWORD}@localhost:5432/${POSTGRES_DB:-kyomi}" \
python -m src.api.services.bigquery_public_indexer

echo ""
echo "✅ Public dataset indexing complete"
