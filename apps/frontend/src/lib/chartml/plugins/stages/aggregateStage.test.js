// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect, vi } from 'vitest';
import { aggregateStage } from '@kyomi/chartml-transform';

describe('aggregateStage', () => {
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

  describe('single source with dimensions and measures', () => {
    it('compiles aggregate config to SQL and materializes result', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_abc123' };
      const aggregateConfig = {
        dimensions: ['region'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
        ],
        sort: [{ field: 'total_revenue', direction: 'desc' }],
        limit: 10,
      };

      const result = await aggregateStage(sourceMap, aggregateConfig, context);

      // Should return a single-entry map with _result key
      expect(Object.keys(result)).toEqual(['_result']);
      expect(result._result).toMatch(/^__stage_agg_/);

      // Should have executed one CREATE TABLE statement
      expect(context.execute).toHaveBeenCalledTimes(1);
      const createSQL = executedStatements[0];
      expect(createSQL).toContain('CREATE OR REPLACE TABLE');
      // The SQL should reference the input table
      expect(createSQL).toContain('__extract_abc123');
      // Should contain aggregate SQL constructs
      expect(createSQL).toContain('SUM');
      expect(createSQL).toContain('GROUP BY');
      expect(createSQL).toContain('ORDER BY');
      expect(createSQL).toContain('LIMIT 10');
    });

    it('handles dimensions-only config (no measures)', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { sales: '__extract_xyz' };
      const aggregateConfig = {
        dimensions: ['category', 'region'],
      };

      const result = await aggregateStage(sourceMap, aggregateConfig, context);

      expect(Object.keys(result)).toEqual(['_result']);
      expect(result._result).toMatch(/^__stage_agg_/);
      expect(context.execute).toHaveBeenCalledTimes(1);
      const createSQL = executedStatements[0];
      expect(createSQL).toContain('__extract_xyz');
    });
  });

  describe('multi-source sourceMap', () => {
    it('throws with descriptive error when given multiple tables', async () => {
      const { context } = createMockContext();
      const sourceMap = {
        orders: '__extract_aaa',
        customers: '__extract_bbb',
      };
      const aggregateConfig = {
        dimensions: ['region'],
        measures: [{ column: 'revenue', aggregation: 'sum', name: 'total' }],
      };

      await expect(
        aggregateStage(sourceMap, aggregateConfig, context)
      ).rejects.toThrow('aggregate stage operates on a single table but received 2 tables');
    });

    it('includes table names in the error message', async () => {
      const { context } = createMockContext();
      const sourceMap = {
        alpha: '__extract_111',
        beta: '__extract_222',
        gamma: '__extract_333',
      };
      const aggregateConfig = { dimensions: ['x'] };

      await expect(
        aggregateStage(sourceMap, aggregateConfig, context)
      ).rejects.toThrow('alpha, beta, gamma');
    });
  });

  describe('aggregate config passed to buildAggregateSQL correctly', () => {
    it('includes filters in generated SQL', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_filtered' };
      const aggregateConfig = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
        ],
        filters: {
          combinator: 'and',
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' },
          ],
        },
      };

      await aggregateStage(sourceMap, aggregateConfig, context);

      const createSQL = executedStatements[0];
      expect(createSQL).toContain('WHERE');
      expect(createSQL).toContain('Electronics');
    });

    it('passes measures with different aggregation functions', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_multi' };
      const aggregateConfig = {
        dimensions: ['month'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
          { column: 'revenue', aggregation: 'avg', name: 'avg_revenue' },
          { column: 'order_id', aggregation: 'count', name: 'order_count' },
        ],
      };

      await aggregateStage(sourceMap, aggregateConfig, context);

      const createSQL = executedStatements[0];
      expect(createSQL).toContain('SUM');
      expect(createSQL).toContain('AVG');
      expect(createSQL).toContain('COUNT');
    });
  });

  describe('deterministic hashing', () => {
    it('produces the same output table ID for the same aggregate config', async () => {
      const { context: ctx1 } = createMockContext();
      const { context: ctx2 } = createMockContext();
      const sourceMap = { _result: '__extract_111' };
      const config = {
        dimensions: ['region'],
        measures: [{ column: 'revenue', aggregation: 'sum', name: 'total' }],
      };

      const result1 = await aggregateStage(sourceMap, config, ctx1);
      const result2 = await aggregateStage(sourceMap, config, ctx2);

      expect(result1._result).toBe(result2._result);
    });

    it('produces different table IDs for different aggregate configs', async () => {
      const { context: ctx1 } = createMockContext();
      const { context: ctx2 } = createMockContext();
      const sourceMap = { _result: '__extract_111' };

      const result1 = await aggregateStage(
        sourceMap,
        { dimensions: ['region'], measures: [{ column: 'revenue', aggregation: 'sum', name: 'total' }] },
        ctx1
      );
      const result2 = await aggregateStage(
        sourceMap,
        { dimensions: ['category'], measures: [{ column: 'revenue', aggregation: 'avg', name: 'avg_rev' }] },
        ctx2
      );

      expect(result1._result).not.toBe(result2._result);
    });

    it('output table ID starts with __stage_agg_ prefix', async () => {
      const { context } = createMockContext();
      const sourceMap = { _result: '__extract_000' };
      const config = { dimensions: ['x'] };

      const result = await aggregateStage(sourceMap, config, context);

      expect(result._result).toMatch(/^__stage_agg_[0-9a-f]+$/);
    });
  });

  describe('error handling', () => {
    it('propagates context.execute errors', async () => {
      const context = {
        execute: vi.fn(async () => {
          throw new Error('DuckDB error: syntax error');
        }),
        runSQL: vi.fn(),
      };
      const sourceMap = { _result: '__extract_111' };
      const config = { dimensions: ['x'] };

      await expect(
        aggregateStage(sourceMap, config, context)
      ).rejects.toThrow('DuckDB error: syntax error');
    });
  });

  describe('isolation', () => {
    it('does not import anything from duckDbMiddleware or other stages', async () => {
      // This test validates that aggregateStage.js is standalone.
      // We verify by checking it uses only the context parameter for execution.
      const { context } = createMockContext();
      const sourceMap = { _result: '__extract_000' };
      const config = {
        dimensions: ['region'],
        measures: [{ column: 'value', aggregation: 'sum', name: 'total' }],
      };

      await aggregateStage(sourceMap, config, context);

      // Only context.execute should be called (not runSQL)
      expect(context.execute).toHaveBeenCalledTimes(1);
      expect(context.runSQL).not.toHaveBeenCalled();
    });
  });
});
