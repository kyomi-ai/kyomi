#!/bin/bash
# Migration script: Move analytics data from shared kyomi_analytics.events table
# to per-site ClickHouse databases.
#
# For each analytics site:
#   1. Creates per-site ClickHouse database + events table (no site_id column)
#   2. Copies data from shared table (excluding site_id column)
#   3. Updates analytics_sites.clickhouse_database in PostgreSQL
#   4. Grants SELECT on per-site database to the site's ClickHouse user
#   5. Updates the datasource connection_config to use the per-site database
#   6. Drops old row policy
#
# Note: the shared kyomi_analytics.events table is intentionally left in place
# after migration. Verify all data migrated correctly, then manually drop it:
#   SELECT count() FROM kyomi_analytics.events  -- compare to total above
#   DROP TABLE kyomi_analytics.events           -- only when confident
#
# Usage:
#   bash scripts/migrate-analytics-to-per-site-db.sh [--dry-run]
#
# Environment variables (loaded from .env if present):
#   DATABASE_URL                    PostgreSQL connection URL
#   ANALYTICS_CLICKHOUSE_HOST       ClickHouse hostname (default: localhost)
#   ANALYTICS_CLICKHOUSE_PORT       ClickHouse HTTP port (default: 8123)
#   ANALYTICS_CLICKHOUSE_PASSWORD   Admin password for ClickHouse default user

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "❌ Not in a git repository"
    exit 1
fi

# Load .env if present — disable set -e briefly to avoid dying on export-less
# variable assignments or benign non-zero exits inside the .env file.
if [ -f "$REPO_ROOT/apps/backend-rust/.env" ]; then
    set +e
    set -a
    # shellcheck source=/dev/null
    source "$REPO_ROOT/apps/backend-rust/.env"
    set +a
    set -e
fi

DRY_RUN=false
if [[ "${1:-}" == "--dry-run" ]]; then
    DRY_RUN=true
    echo "🔍 DRY RUN — no changes will be made"
fi

# ─── Config ──────────────────────────────────────────────────────────────────

CH_HOST="${ANALYTICS_CLICKHOUSE_HOST:-localhost}"
CH_PORT="${ANALYTICS_CLICKHOUSE_PORT:-8123}"
CH_PASSWORD="${ANALYTICS_CLICKHOUSE_PASSWORD:-}"
DATABASE_URL="${DATABASE_URL:-}"

if [ -z "$DATABASE_URL" ]; then
    echo "❌ DATABASE_URL is not set"
    exit 1
fi

# Mask password in DATABASE_URL for display — sed handles passwords containing @.
DATABASE_URL_MASKED="$(echo "$DATABASE_URL" | sed 's|://[^:]*:[^@]*@|://***:***@|')"
echo "📋 Config:"
echo "   ClickHouse: http://${CH_HOST}:${CH_PORT}"
echo "   PostgreSQL: ${DATABASE_URL_MASKED}"
echo ""

# ─── Helpers ─────────────────────────────────────────────────────────────────

# Write ClickHouse credentials to a temp curl config file so the admin password
# never appears in the curl command line (visible in `ps aux`).
_ch_creds_config=$(mktemp)
chmod 600 "$_ch_creds_config"
trap 'rm -f "$_ch_creds_config"' EXIT
{
    printf 'header = "X-ClickHouse-User: default"\n'
    printf 'header = "X-ClickHouse-Key: %s"\n' "${CH_PASSWORD}"
} > "$_ch_creds_config"

ch_query() {
    local sql="$1"
    curl -s -f -X POST \
        "http://${CH_HOST}:${CH_PORT}/" \
        -K "$_ch_creds_config" \
        --data-binary "$sql"
}

pg_query() {
    local sql="$1"
    psql "$DATABASE_URL" -t -c "$sql"
}

sanitize_workspace_id() {
    # Use printf to avoid the trailing newline that echo appends.
    # echo "$1" | tr -c ... would replace that newline with '_', producing a
    # trailing underscore that doesn't match the Rust database_name() output.
    # LC_ALL=C ensures ASCII-range matching is locale-independent.
    printf '%s' "$1" | LC_ALL=C tr -c 'a-zA-Z0-9_' '_'
}

# ─── Main ────────────────────────────────────────────────────────────────────

echo "📥 Fetching analytics sites from PostgreSQL..."
sites=$(pg_query "SELECT site_id, workspace_id FROM analytics_sites ORDER BY created_at")

if [ -z "$(echo "$sites" | tr -d ' \n')" ]; then
    echo "✅ No analytics sites found — nothing to migrate"
    exit 0
fi

total=$(echo "$sites" | grep -c '|' || true)
echo "📊 Found ${total} site(s) to migrate"
echo ""

migrated=0
skipped=0
failed=0

while IFS='|' read -r site_id workspace_id; do
    # Strip all whitespace (spaces, tabs, newlines) from psql output
    site_id="$(echo "$site_id" | tr -d '[:space:]')"
    workspace_id="$(echo "$workspace_id" | tr -d '[:space:]')"

    if [ -z "$site_id" ] || [ -z "$workspace_id" ]; then
        continue
    fi

    # Validate site_id is the expected 16-char lowercase hex format.
    # This prevents malformed identifiers from being embedded in SQL statements
    # (our generate_site_id() always produces hex, but be defensive).
    if ! [[ "$site_id" =~ ^[a-f0-9]+$ ]]; then
        echo "   ❌ site_id '${site_id}' is not lowercase hex — skipping (unexpected format)"
        failed=$((failed + 1))
        continue
    fi

    sanitized_ws=$(sanitize_workspace_id "$workspace_id")
    db_name="site_${sanitized_ws}_${site_id}"
    ch_username="kyomi_site_${site_id}"

    echo "🔄 Migrating site: ${site_id} (workspace: ${workspace_id})"
    echo "   → Database: ${db_name}"

    # Check if already migrated
    existing_db=$(pg_query "SELECT clickhouse_database FROM analytics_sites WHERE site_id = '${site_id}'" | tr -d '[:space:]')
    if [ -n "$existing_db" ]; then
        echo "   ⏭️  Already migrated (clickhouse_database = ${existing_db}) — skipping"
        skipped=$((skipped + 1))
        continue
    fi

    # Show expected row count in dry-run mode
    if [ "$DRY_RUN" = true ]; then
        dry_count=$(ch_query "SELECT count() FROM kyomi_analytics.events WHERE site_id = '${site_id}' FORMAT TabSeparated" | tr -d '[:space:]') || true
        dry_count="${dry_count:-unknown}"
        echo "   [DRY RUN] Would migrate ${dry_count} events to database \`${db_name}\`"
        migrated=$((migrated + 1))
        continue
    fi

    # Step 1: Create per-site database
    echo "   📁 Creating database..."
    # Backtick-quote the database name so identifiers with special characters
    # are handled correctly by ClickHouse, even though db_name only contains
    # alphanumeric + underscore chars in practice.
    if ! ch_query "CREATE DATABASE IF NOT EXISTS \`${db_name}\`"; then
        echo "   ❌ Failed to create database — skipping site"
        failed=$((failed + 1))
        continue
    fi

    # Step 2: Create events table (no site_id column)
    echo "   📋 Creating events table..."
    if ! ch_query "CREATE TABLE IF NOT EXISTS \`${db_name}\`.events (
        visitor_id String,
        session_id String,
        user_id String DEFAULT '',
        timestamp DateTime64(3),
        event_name LowCardinality(String),
        hostname LowCardinality(String),
        pathname String,
        referrer String,
        referrer_source LowCardinality(String),
        utm_source LowCardinality(String),
        utm_medium LowCardinality(String),
        utm_campaign LowCardinality(String),
        utm_term String,
        utm_content String,
        country_code LowCardinality(String),
        region LowCardinality(String),
        city String,
        browser LowCardinality(String),
        browser_version LowCardinality(String),
        os LowCardinality(String),
        os_version LowCardinality(String),
        device_type LowCardinality(String),
        screen_width UInt16,
        screen_height UInt16,
        properties Map(String, String)
    ) ENGINE = MergeTree()
    PARTITION BY toYYYYMM(timestamp)
    ORDER BY (toDate(timestamp), event_name, visitor_id)
    TTL toDateTime(timestamp) + INTERVAL 2 YEAR"; then
        echo "   ❌ Failed to create events table — skipping site"
        failed=$((failed + 1))
        continue
    fi

    # Step 3: Copy data from shared table (excluding site_id)
    echo "   📤 Copying data from kyomi_analytics.events..."
    # Use TabSeparated format — returns a bare integer, no python3 required.
    # || true prevents set -euo pipefail from killing the whole script if ch_query
    # exits non-zero (e.g. curl -f on HTTP error or ClickHouse unreachable).
    # The validation below catches the empty/non-numeric result and aborts this site.
    row_count=$(ch_query "SELECT count() FROM kyomi_analytics.events WHERE site_id = '${site_id}' FORMAT TabSeparated" | tr -d '[:space:]') || true
    if [ -z "$row_count" ] || ! [[ "$row_count" =~ ^[0-9]+$ ]]; then
        echo "   ❌ Failed to get source event count from ClickHouse (got: '${row_count}') — skipping site to prevent data loss"
        failed=$((failed + 1))
        continue
    fi
    echo "   📊 Found ${row_count} source events to copy"

    if [ "$row_count" != "0" ]; then
        # Guard against double-insert on retry: check if destination already has data.
        # This can happen if the INSERT succeeded but the PostgreSQL UPDATE below failed.
        dest_pre_count=$(ch_query "SELECT count() FROM \`${db_name}\`.events FORMAT TabSeparated" | tr -d '[:space:]') || true
        if [ -n "$dest_pre_count" ] && [[ "$dest_pre_count" =~ ^[0-9]+$ ]] && [ "$dest_pre_count" != "0" ]; then
            echo "   ⚠️  Destination already has ${dest_pre_count} events — skipping INSERT to prevent duplication"
        else
            if ! ch_query "INSERT INTO \`${db_name}\`.events
                SELECT
                    visitor_id, session_id, user_id, timestamp, event_name,
                    hostname, pathname, referrer, referrer_source,
                    utm_source, utm_medium, utm_campaign, utm_term, utm_content,
                    country_code, region, city, browser, browser_version,
                    os, os_version, device_type, screen_width, screen_height, properties
                FROM kyomi_analytics.events
                WHERE site_id = '${site_id}'"; then
                echo "   ❌ Failed to copy data — skipping site"
                failed=$((failed + 1))
                continue
            fi

            # Verify destination row count matches source to catch silent data loss
            dest_count=$(ch_query "SELECT count() FROM \`${db_name}\`.events FORMAT TabSeparated" | tr -d '[:space:]') || true
            if [ -z "$dest_count" ] || ! [[ "$dest_count" =~ ^[0-9]+$ ]]; then
                echo "   ❌ Failed to verify destination row count after INSERT — manual check required"
                failed=$((failed + 1))
                continue
            fi
            if [ "$dest_count" != "$row_count" ]; then
                echo "   ❌ Destination count (${dest_count}) != source count (${row_count}) — manual check required"
                failed=$((failed + 1))
                continue
            fi
            echo "   ✅ Copied and verified ${dest_count} events"
        fi
    fi

    # Step 4: Update PostgreSQL — set clickhouse_database
    echo "   📝 Updating analytics_sites.clickhouse_database..."
    if ! pg_query "UPDATE analytics_sites SET clickhouse_database = '${db_name}' WHERE site_id = '${site_id}'"; then
        echo "   ❌ Failed to update analytics_sites — data was copied but metadata not updated"
        echo "   ℹ️  Manual fix: UPDATE analytics_sites SET clickhouse_database = '${db_name}' WHERE site_id = '${site_id}'"
        failed=$((failed + 1))
        continue
    fi

    # Step 5: Grant SELECT on per-site database to the site's ClickHouse user
    echo "   🔑 Granting SELECT on \`${db_name}\`.* to \`${ch_username}\`..."
    ch_query "GRANT SELECT ON \`${db_name}\`.* TO \`${ch_username}\`" 2>/dev/null || \
        echo "   ⚠️  GRANT failed (user may not exist yet) — skipping grant"

    # Step 6: Update datasource connection_config to use per-site database
    echo "   🔗 Updating datasource connection_config..."
    if ! pg_query "UPDATE datasource_configs dc
        SET connection_config = jsonb_set(connection_config, '{database}', to_jsonb('${db_name}'::text))
        FROM analytics_sites a
        WHERE a.site_id = '${site_id}'
          AND dc.id = a.datasource_id
          AND a.datasource_id IS NOT NULL"; then
        # Non-fatal: data is migrated and PG updated. Datasource still queries old database
        # but this is recoverable — the datasource entry can be manually updated.
        echo "   ⚠️  Failed to update datasource connection_config — datasource may query old database"
        echo "   ℹ️  Manual fix: UPDATE datasource_configs dc SET connection_config = jsonb_set(connection_config, '{database}', to_jsonb('${db_name}'::text)) FROM analytics_sites a WHERE a.site_id = '${site_id}' AND dc.id = a.datasource_id"
    fi

    # Step 7: Drop old row policy
    echo "   🗑️  Dropping old row policy..."
    ch_query "DROP ROW POLICY IF EXISTS site_${site_id}_policy ON kyomi_analytics.events" 2>/dev/null || \
        echo "   ℹ️  Row policy not found (may not have existed)"

    echo "   ✅ Site ${site_id} migrated successfully"
    migrated=$((migrated + 1))
    echo ""

done <<< "$sites"

echo ""
echo "═══════════════════════════════════════════"
echo "Migration complete:"
echo "  ✅ Migrated:  ${migrated}"
echo "  ⏭️  Skipped:   ${skipped}"
echo "  ❌ Failed:    ${failed}"
echo "═══════════════════════════════════════════"
echo ""
echo "Note: kyomi_analytics.events was NOT dropped. Verify data integrity, then:"
echo "  ch_query \"SELECT count() FROM kyomi_analytics.events\"  -- should match total"
echo "  ch_query \"DROP TABLE kyomi_analytics.events\"           -- only when confident"

if [ "$failed" -gt 0 ]; then
    exit 1
fi
