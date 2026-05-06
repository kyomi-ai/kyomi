#!/bin/bash
# Regenerate .sqlx/ query cache from a live database.
# Requires a running Postgres with the current schema applied.
set -e

cd "$(git rev-parse --show-toplevel)/apps/backend-rust"

export DATABASE_URL="${DATABASE_URL:-postgresql://kyomi:password@localhost:5433/kyomi}"

echo "Using DATABASE_URL: ${DATABASE_URL%%@*}@***"
cargo sqlx prepare --workspace

echo "Done. Commit the updated .sqlx/ directory."
