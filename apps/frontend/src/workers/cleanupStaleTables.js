// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * DuckDB stale table cleanup routine.
 *
 * Drops tracked tables older than 24 hours and untracked tables that have
 * persisted across two consecutive cleanup runs (grace period).
 *
 * Extracted from duckdb-worker.js so the logic is independently testable.
 */

// Tracks untracked tables seen on the previous cleanup run.
// Safe as module-level state because this runs inside a Web Worker
// (single-threaded, single instance per tab).
let previousUntracked = new Set();

/**
 * Clean up stale DuckDB tables.
 *
 * @param {Function} getConnection - Async function that returns a DuckDB connection.
 *   The caller is responsible for providing a connection factory (e.g., () => db.connect()).
 *   The connection will be closed by this function.
 *
 * Non-fatal: errors are logged but never thrown.
 */
export async function cleanupStaleTables(getConnection) {
    try {
        const conn = await getConnection();
        try {
            const MAX_AGE_HOURS = 24;
            const cutoff = new Date(Date.now() - MAX_AGE_HOURS * 60 * 60 * 1000).toISOString();

            // 1. Get all tables in DuckDB
            const allTablesResult = await conn.query(
                `SELECT table_name FROM duckdb_tables() WHERE internal = false`
            );
            const allTables = new Set(allTablesResult.toArray().map(r => r.table_name));

            // 2. Get tracked tables from cache_metadata
            const trackedResult = await conn.query(
                `SELECT table_name, created_at FROM cache_metadata`
            );
            const tracked = trackedResult.toArray();
            const trackedNames = new Set(tracked.map(r => r.table_name));

            // 3. Drop tracked tables older than 24 hours
            for (const row of tracked) {
                if (new Date(row.created_at) < new Date(cutoff)) {
                    const escapedName = row.table_name.replace(/'/g, "''");
                    try {
                        await conn.query(`DROP TABLE IF EXISTS "${row.table_name}"`);
                        await conn.query(
                            `DELETE FROM cache_metadata WHERE table_name = '${escapedName}'`
                        );
                        console.warn(`[DuckDB Cleanup] Dropped stale tracked table: ${row.table_name}`);
                    } catch (dropError) {
                        console.warn(`[DuckDB Cleanup] Failed to drop tracked table ${row.table_name}:`, dropError.message);
                    }
                }
            }

            // 4. Identify untracked tables (exclude cache_metadata itself)
            const currentUntracked = new Set();
            for (const name of allTables) {
                if (name !== 'cache_metadata' && !trackedNames.has(name)) {
                    currentUntracked.add(name);
                }
            }

            // 5. Drop untracked tables seen on BOTH this run and the previous run
            for (const name of currentUntracked) {
                if (previousUntracked.has(name)) {
                    try {
                        await conn.query(`DROP TABLE IF EXISTS "${name}"`);
                        console.warn(`[DuckDB Cleanup] Dropped stale untracked table: ${name}`);
                    } catch (dropError) {
                        console.warn(`[DuckDB Cleanup] Failed to drop untracked table ${name}:`, dropError.message);
                    }
                }
            }

            // 6. Update previous set for next run
            previousUntracked = currentUntracked;
        } finally {
            await conn.close();
        }
    } catch (error) {
        console.warn('[DuckDB Cleanup] Cleanup failed (non-fatal):', error.message);
    }
}

/**
 * Reset the previousUntracked set. Used for testing.
 */
export function resetCleanupState() {
    previousUntracked = new Set();
}
