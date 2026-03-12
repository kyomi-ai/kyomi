# Database Migrations with sqlx

**Status**: Active (as of v1.5, February 2026)
**Migration System**: sqlx-cli (Rust)
**Location**: `apps/backend-rust/migrations/`
**Runtime**: `sqlx::migrate!()` macro embedded in the Rust binary

---

## Quick Reference

```bash
# Create a new migration
cd apps/backend-rust && sqlx migrate add "add_mcp_servers_table"
# → Creates: migrations/YYYYMMDDHHMMSS_add_mcp_servers_table.sql

# Apply migrations locally
cd apps/backend-rust && sqlx migrate run

# Check migration status
cd apps/backend-rust && sqlx migrate info

# Regenerate compile-time query cache (after schema changes)
cd apps/backend-rust && cargo sqlx prepare --workspace
```

---

## How It Works

### Compile-Time + Runtime

sqlx provides two mechanisms:

1. **Compile-time**: The `sqlx::migrate!()` macro embeds all migration SQL files into the Rust binary at build time. No external files needed at runtime.

2. **Runtime**: When the binary starts, it runs pending migrations before serving traffic. Uses `pg_advisory_lock` internally for multi-replica safety (safe with 2+ pods starting simultaneously).

### Migration Tracking

sqlx tracks applied migrations in the `_sqlx_migrations` table (created automatically):

```
_sqlx_migrations
├── version (BIGINT) ← timestamp prefix from filename
├── description (TEXT) ← name from filename
├── checksum (BYTEA) ← SHA-256 of SQL content
├── installed_on (TIMESTAMPTZ)
└── success (BOOL)
```

### Production Flow

1. CI builds the Rust binary with migrations embedded
2. Kubernetes deploys new pods
3. First pod to start acquires `pg_advisory_lock` and runs pending migrations
4. Other pods wait for the lock, then skip (already applied)
5. All pods start serving traffic

No separate migration step needed — it's built into the binary.

---

## Creating Migrations

### When to Create a Migration

- Adding/removing/modifying tables or columns
- Adding/removing indexes
- Changing column types or constraints
- Backfilling data
- Adding extensions

### Step-by-Step

1. **Create the migration file**:
```bash
cd apps/backend-rust
sqlx migrate add "add_timezone_to_users"
```

This creates `migrations/YYYYMMDDHHMMSS_add_timezone_to_users.sql`.

2. **Write the SQL**:
```sql
-- Add timezone column to users table
ALTER TABLE users ADD COLUMN IF NOT EXISTS timezone VARCHAR(100);
```

3. **Apply locally**:
```bash
sqlx migrate run
```

4. **Regenerate the compile-time query cache**:
```bash
cargo sqlx prepare --workspace
```

5. **Commit all three**:
```bash
git add migrations/YYYYMMDDHHMMSS_add_timezone_to_users.sql
git add .sqlx/
git commit -m "Add timezone column to users table"
```

### Naming Conventions

Migration filenames: `YYYYMMDDHHMMSS_<action>_<target>_<details>.sql`

Good examples:
- `add_user_preferences_table`
- `add_timezone_column_to_users`
- `remove_deprecated_oauth_table`
- `backfill_user_timezones`
- `add_index_to_chat_messages_created_at`

### Idempotent SQL (Recommended)

Write migrations so they can be re-run without errors:

```sql
-- Tables
CREATE TABLE IF NOT EXISTS user_preferences (...);

-- Columns
ALTER TABLE users ADD COLUMN IF NOT EXISTS timezone VARCHAR(100);

-- Indexes
CREATE INDEX IF NOT EXISTS idx_users_timezone ON users(timezone);

-- Constraints (use DO block)
DO $$ BEGIN
    ALTER TABLE users ADD CONSTRAINT fk_users_workspace
        FOREIGN KEY (workspace_id) REFERENCES workspaces(workspace_id);
EXCEPTION
    WHEN duplicate_object THEN NULL;
    WHEN duplicate_table THEN NULL;
END $$;
```

---

## Testing Migrations

### Local Testing

```bash
# 1. Apply migration
cd apps/backend-rust && sqlx migrate run

# 2. Verify schema
PGPASSWORD=password psql -h localhost -p 5433 -U kyomi kyomi
\d table_name

# 3. Test the Rust binary builds
cargo check

# 4. Regenerate query cache
cargo sqlx prepare --workspace
```

### Fresh Database Test

```bash
# Create a fresh test database
PGPASSWORD=password createdb -h localhost -p 5433 -U kyomi kyomi_test

# Apply all migrations
DATABASE_URL=postgresql://kyomi:password@localhost:5433/kyomi_test sqlx migrate run

# Verify
PGPASSWORD=password psql -h localhost -p 5433 -U kyomi kyomi_test -c "\dt"

# Clean up
PGPASSWORD=password dropdb -h localhost -p 5433 -U kyomi kyomi_test
```

### CI Validation

The CI pipeline:
1. Starts a temporary Postgres with pgvector
2. Builds the Rust Docker image with `SQLX_OFFLINE=false`
3. Inside Docker, installs sqlx-cli and runs `sqlx migrate run` to apply schema
4. `cargo build` validates all compile-time queries against the live schema
5. Cleans up temporary Postgres

---

## Deploying Migrations

Migrations deploy automatically — they're embedded in the binary and run at startup. No manual steps needed.

### Manual Operations (rare)

```bash
# Check migration status on production
export KUBECONFIG=~/.kube/config-prod
POD=$(kubectl get pods -n kyomi -l app=kyomi-api -o jsonpath='{.items[0].metadata.name}')

# View applied migrations (query _sqlx_migrations table)
kubectl exec -n kyomi $POD -- /app/kyomi-api --migrate-info  # Not available yet
# Or check via psql on the production database
```

---

## Best Practices

### DO

- Write idempotent SQL (IF NOT EXISTS, DO blocks with exception handlers)
- Keep migrations small — one logical change per migration
- Commit migration file + `.sqlx/` cache changes together
- Test migrations locally before pushing
- Add `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` for column additions

### DON'T

- Don't edit applied migrations (once in production, they're immutable)
- Don't delete migration files
- Don't forget to regenerate `.sqlx/` cache after schema changes
- Don't combine schema + data migrations in one file

---

## Migration History

### v1.5 - sqlx-cli Migration System (February 2026)

Migrated from Alembic (Python) to sqlx-cli (Rust) as part of Python backend decommissioning.

- `20260215000000` - Baseline schema (37 tables, all extensions, indexes, triggers)

### v1.3 - Alembic Migration System (January 2026)

Previous system (archived in `apps/backend/alembic/`). The Python backend source code is kept as reference.

---

**Last Updated**: February 15, 2026
**System Version**: v1.5
**Migration System**: sqlx-cli 0.8.x
