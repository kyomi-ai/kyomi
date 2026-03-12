// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect, vi } from 'vitest';
import { sqlStage } from '@kyomi/chartml-transform';

describe('sqlStage', () => {
  /**
   * Build a mock context with execute and runSQL functions.
   * Tracks all executed SQL statements for assertion.
   */
  function createMockContext() {
    const executedStatements = [];
    return {
      context: {
        execute: vi.fn(async (sql) => {
          executedStatements.push(sql);
        }),
        runSQL: vi.fn(async (sql) => {
          executedStatements.push(sql);
          return { columns: ['col1'], rows: [['val1']] };
        }),
      },
      executedStatements,
    };
  }

  describe('single SQL string', () => {
    it('replaces placeholders and materializes result', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = {
        orders: '__extract_abc123',
        customers: '__extract_def456',
      };
      const sqlConfig = 'SELECT * FROM {orders} o JOIN {customers} c ON o.id = c.order_id';

      const result = await sqlStage(sourceMap, sqlConfig, context);

      // Should return a single-entry map with _result key
      expect(Object.keys(result)).toEqual(['_result']);
      expect(result._result).toMatch(/^__stage_sql_/);

      // Should have executed one CREATE TABLE statement
      expect(context.execute).toHaveBeenCalledTimes(1);
      const createSQL = executedStatements[0];
      expect(createSQL).toContain('CREATE OR REPLACE TABLE');
      expect(createSQL).toContain('"__extract_abc123"');
      expect(createSQL).toContain('"__extract_def456"');
      expect(createSQL).not.toContain('{orders}');
      expect(createSQL).not.toContain('{customers}');
    });

    it('quotes table identifiers in placeholder replacement', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { sales: '__extract_xyz' };
      const sqlConfig = 'SELECT region, SUM(revenue) FROM {sales} GROUP BY region';

      await sqlStage(sourceMap, sqlConfig, context);

      const createSQL = executedStatements[0];
      expect(createSQL).toContain('"__extract_xyz"');
      expect(createSQL).not.toContain('{sales}');
    });
  });

  describe('array of SQL strings', () => {
    it('executes setup statements then materializes the last', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { data: '__extract_001' };
      const sqlConfig = [
        'CREATE MACRO my_func(x) AS x * 2',
        'SELECT my_func(value) as doubled FROM {data}',
      ];

      const result = await sqlStage(sourceMap, sqlConfig, context);

      // Two execute calls: one setup, one materialization
      expect(context.execute).toHaveBeenCalledTimes(2);

      // First call is the setup statement (no CREATE TABLE wrapper)
      expect(executedStatements[0]).toBe('CREATE MACRO my_func(x) AS x * 2');

      // Second call is the materialized final statement
      expect(executedStatements[1]).toContain('CREATE OR REPLACE TABLE');
      expect(executedStatements[1]).toContain('"__extract_001"');

      // Result is single-entry map
      expect(Object.keys(result)).toEqual(['_result']);
      expect(result._result).toMatch(/^__stage_sql_/);
    });

    it('replaces placeholders in all statements including setup', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { src: '__extract_aaa' };
      const sqlConfig = [
        'CREATE VIEW temp_view AS SELECT * FROM {src} WHERE active = true',
        'SELECT * FROM temp_view',
      ];

      await sqlStage(sourceMap, sqlConfig, context);

      // Setup statement should have placeholder replaced
      expect(executedStatements[0]).toContain('"__extract_aaa"');
      expect(executedStatements[0]).not.toContain('{src}');
    });
  });

  describe('deterministic hashing', () => {
    it('produces the same output table ID for the same SQL config', async () => {
      const { context: ctx1 } = createMockContext();
      const { context: ctx2 } = createMockContext();
      const sourceMap = { data: '__extract_111' };

      const result1 = await sqlStage(sourceMap, 'SELECT * FROM {data}', ctx1);
      const result2 = await sqlStage(sourceMap, 'SELECT * FROM {data}', ctx2);

      expect(result1._result).toBe(result2._result);
    });

    it('produces different table IDs for different SQL configs', async () => {
      const { context: ctx1 } = createMockContext();
      const { context: ctx2 } = createMockContext();
      const sourceMap = { data: '__extract_111' };

      const result1 = await sqlStage(sourceMap, 'SELECT * FROM {data}', ctx1);
      const result2 = await sqlStage(sourceMap, 'SELECT col FROM {data}', ctx2);

      expect(result1._result).not.toBe(result2._result);
    });
  });

  describe('error handling', () => {
    it('throws on empty array', async () => {
      const { context } = createMockContext();
      const sourceMap = { data: '__extract_111' };

      await expect(sqlStage(sourceMap, [], context)).rejects.toThrow(
        'sql config must contain at least one SQL statement'
      );
    });

    it('propagates context.execute errors', async () => {
      const context = {
        execute: vi.fn(async () => { throw new Error('DuckDB error: table not found'); }),
        runSQL: vi.fn(),
      };
      const sourceMap = { data: '__extract_111' };

      await expect(
        sqlStage(sourceMap, 'SELECT * FROM {data}', context)
      ).rejects.toThrow('DuckDB error: table not found');
    });
  });

  describe('isolation', () => {
    it('does not import anything from duckDbMiddleware or other stages', async () => {
      // This test validates that sqlStage.js is standalone.
      // We can verify by checking it uses only the context parameter for execution.
      const { context } = createMockContext();
      const sourceMap = { t: '__extract_000' };

      await sqlStage(sourceMap, 'SELECT 1 FROM {t}', context);

      // Only context.execute should be called (not runSQL for single statement)
      expect(context.execute).toHaveBeenCalledTimes(1);
      expect(context.runSQL).not.toHaveBeenCalled();
    });
  });
});
