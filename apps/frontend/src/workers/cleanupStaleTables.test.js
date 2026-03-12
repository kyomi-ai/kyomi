// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { cleanupStaleTables, resetCleanupState } from './cleanupStaleTables.js';

/**
 * Create a mock DuckDB connection that tracks queries and returns
 * configurable results for duckdb_tables() and cache_metadata queries.
 */
function createMockConnection({ allTables = [], trackedRows = [] } = {}) {
    const executedQueries = [];

    const conn = {
        query: vi.fn(async (sql) => {
            executedQueries.push(sql);

            // Return all tables list
            if (sql.includes('duckdb_tables()')) {
                return {
                    toArray: () => allTables.map(name => ({ table_name: name })),
                };
            }

            // Return tracked table metadata
            if (sql.includes('SELECT table_name, created_at FROM cache_metadata')) {
                return {
                    toArray: () => trackedRows,
                };
            }

            // DROP and DELETE return nothing meaningful
            return { toArray: () => [] };
        }),
        close: vi.fn(),
    };

    return { conn, executedQueries };
}

function hoursAgo(hours) {
    return new Date(Date.now() - hours * 60 * 60 * 1000).toISOString();
}

describe('cleanupStaleTables', () => {
    beforeEach(() => {
        resetCleanupState();
        vi.restoreAllMocks();
    });

    it('drops tracked tables older than 24 hours and removes metadata', async () => {
        const { conn, executedQueries } = createMockConnection({
            allTables: ['cache_metadata', '__extract_old'],
            trackedRows: [
                { table_name: '__extract_old', created_at: hoursAgo(25) },
            ],
        });

        await cleanupStaleTables(async () => conn);

        const dropQueries = executedQueries.filter(q => q.includes('DROP TABLE'));
        const deleteQueries = executedQueries.filter(q => q.includes('DELETE FROM cache_metadata'));

        expect(dropQueries).toHaveLength(1);
        expect(dropQueries[0]).toContain('"__extract_old"');
        expect(deleteQueries).toHaveLength(1);
        expect(deleteQueries[0]).toContain('__extract_old');
    });

    it('keeps tracked tables younger than 24 hours', async () => {
        const { conn, executedQueries } = createMockConnection({
            allTables: ['cache_metadata', '__extract_fresh'],
            trackedRows: [
                { table_name: '__extract_fresh', created_at: hoursAgo(2) },
            ],
        });

        await cleanupStaleTables(async () => conn);

        const dropQueries = executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(0);
    });

    it('does not drop untracked tables on first run (grace period)', async () => {
        const { conn, executedQueries } = createMockConnection({
            allTables: ['cache_metadata', 'agent_created_table'],
            trackedRows: [],
        });

        await cleanupStaleTables(async () => conn);

        const dropQueries = executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(0);
    });

    it('drops untracked tables seen on two consecutive runs', async () => {
        // First run — records the untracked table
        const mock1 = createMockConnection({
            allTables: ['cache_metadata', 'leaked_stage_table'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock1.conn);

        // Second run — same untracked table still present → should drop
        const mock2 = createMockConnection({
            allTables: ['cache_metadata', 'leaked_stage_table'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock2.conn);

        const dropQueries = mock2.executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(1);
        expect(dropQueries[0]).toContain('"leaked_stage_table"');
    });

    it('does not drop untracked table that disappears between runs', async () => {
        // First run — records untracked table
        const mock1 = createMockConnection({
            allTables: ['cache_metadata', 'temp_table'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock1.conn);

        // Second run — table is gone (cleaned up by pipeline)
        const mock2 = createMockConnection({
            allTables: ['cache_metadata'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock2.conn);

        const dropQueries = mock2.executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(0);
    });

    it('never drops cache_metadata', async () => {
        // First run
        const mock1 = createMockConnection({
            allTables: ['cache_metadata'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock1.conn);

        // Second run — cache_metadata is still excluded
        const mock2 = createMockConnection({
            allTables: ['cache_metadata'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock2.conn);

        const dropQueries = mock2.executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(0);
    });

    it('handles errors non-fatally when getConnection fails', async () => {
        const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

        await cleanupStaleTables(async () => {
            throw new Error('Connection failed');
        });

        // Should not throw — function completes normally
        expect(warnSpy).toHaveBeenCalledWith(
            '[DuckDB Cleanup] Cleanup failed (non-fatal):',
            'Connection failed'
        );
    });

    it('handles errors non-fatally when a query fails', async () => {
        const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

        const conn = {
            query: vi.fn(async () => {
                throw new Error('Query failed');
            }),
            close: vi.fn(),
        };

        await cleanupStaleTables(async () => conn);

        // Should not throw — function catches and logs
        expect(warnSpy).toHaveBeenCalled();
    });

    it('runs cleanly on a fresh database with no tables', async () => {
        const { conn, executedQueries } = createMockConnection({
            allTables: ['cache_metadata'],
            trackedRows: [],
        });

        await cleanupStaleTables(async () => conn);

        const dropQueries = executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(0);
        expect(conn.close).toHaveBeenCalledOnce();
    });

    it('handles mixed scenario: old tracked, fresh tracked, and untracked tables', async () => {
        // First run to seed previousUntracked
        const mock1 = createMockConnection({
            allTables: ['cache_metadata', '__extract_old', '__extract_fresh', 'agent_table'],
            trackedRows: [
                { table_name: '__extract_old', created_at: hoursAgo(48) },
                { table_name: '__extract_fresh', created_at: hoursAgo(1) },
            ],
        });
        await cleanupStaleTables(async () => mock1.conn);

        // First run should drop old tracked, keep fresh, record untracked
        const run1Drops = mock1.executedQueries.filter(q => q.startsWith('DROP TABLE'));
        expect(run1Drops).toHaveLength(1);
        expect(run1Drops[0]).toContain('"__extract_old"');

        // Second run — agent_table still untracked → should drop it
        const mock2 = createMockConnection({
            allTables: ['cache_metadata', '__extract_fresh', 'agent_table'],
            trackedRows: [
                { table_name: '__extract_fresh', created_at: hoursAgo(1) },
            ],
        });
        await cleanupStaleTables(async () => mock2.conn);

        const run2Drops = mock2.executedQueries.filter(q => q.startsWith('DROP TABLE'));
        expect(run2Drops).toHaveLength(1);
        expect(run2Drops[0]).toContain('"agent_table"');
    });

    it('closes connection even when cleanup encounters errors', async () => {
        const conn = {
            query: vi.fn()
                .mockResolvedValueOnce({ toArray: () => [{ table_name: 'cache_metadata' }] }) // duckdb_tables
                .mockRejectedValueOnce(new Error('cache_metadata query failed')), // cache_metadata SELECT
            close: vi.fn(),
        };

        vi.spyOn(console, 'warn').mockImplementation(() => {});

        await cleanupStaleTables(async () => conn);

        expect(conn.close).toHaveBeenCalledOnce();
    });

    it('quotes table names with special characters in DROP statements', async () => {
        // First run to seed previousUntracked
        const mock1 = createMockConnection({
            allTables: ['cache_metadata', 'my-table.2024'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock1.conn);

        // Second run — should drop with quoted name
        const mock2 = createMockConnection({
            allTables: ['cache_metadata', 'my-table.2024'],
            trackedRows: [],
        });
        await cleanupStaleTables(async () => mock2.conn);

        const dropQueries = mock2.executedQueries.filter(q => q.includes('DROP TABLE'));
        expect(dropQueries).toHaveLength(1);
        expect(dropQueries[0]).toBe('DROP TABLE IF EXISTS "my-table.2024"');
    });

    it('escapes single quotes in tracked table names for DELETE', async () => {
        const { conn, executedQueries } = createMockConnection({
            allTables: ['cache_metadata', "table'name"],
            trackedRows: [
                { table_name: "table'name", created_at: hoursAgo(25) },
            ],
        });

        await cleanupStaleTables(async () => conn);

        const deleteQueries = executedQueries.filter(q => q.includes('DELETE FROM cache_metadata'));
        expect(deleteQueries).toHaveLength(1);
        expect(deleteQueries[0]).toContain("table''name");
    });
});
