// SPDX-License-Identifier: AGPL-3.0-or-later
import { describe, it, expect, vi } from 'vitest';
import { forecastStage, buildForecastSQL } from '@kyomi/chartml-transform';

describe('forecastStage', () => {
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

  describe('basic forecast', () => {
    it('executes 3 SQL statements and returns output table with __stage_fcast_ prefix', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_abc123' };
      const forecastConfig = {
        timestamp: 'date',
        value: 'revenue',
      };

      const result = await forecastStage(sourceMap, forecastConfig, context);

      // Should return a single-entry map with _result key
      expect(Object.keys(result)).toEqual(['_result']);
      expect(result._result).toMatch(/^__stage_fcast_/);
      // Should NOT match src or pred intermediate tables
      expect(result._result).not.toMatch(/^__stage_fcast_src_/);
      expect(result._result).not.toMatch(/^__stage_fcast_pred_/);

      // Should have executed exactly 3 CREATE TABLE statements
      expect(context.execute).toHaveBeenCalledTimes(3);

      // Step 1: materialize source
      expect(executedStatements[0]).toContain('CREATE OR REPLACE TABLE');
      expect(executedStatements[0]).toContain('__stage_fcast_src_');
      expect(executedStatements[0]).toContain('__extract_abc123');
      expect(executedStatements[0]).toContain('date');
      expect(executedStatements[0]).toContain('revenue');

      // Step 2: forecast()
      expect(executedStatements[1]).toContain('CREATE OR REPLACE TABLE');
      expect(executedStatements[1]).toContain('__stage_fcast_pred_');
      expect(executedStatements[1]).toContain('forecast(');
      expect(executedStatements[1]).toContain("timestamp = 'date'");
      expect(executedStatements[1]).toContain("value = 'revenue'");

      // Step 3: UNION ALL
      expect(executedStatements[2]).toContain('CREATE OR REPLACE TABLE');
      expect(executedStatements[2]).toContain('UNION ALL');
      expect(executedStatements[2]).toContain('forecast_timestamp as date');
      expect(executedStatements[2]).toContain('is_forecast');
    });
  });

  describe('all optional params', () => {
    it('reflects horizon, confidence_level, and model in generated SQL', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_xyz' };
      const forecastConfig = {
        timestamp: 'month',
        value: 'sales',
        horizon: 12,
        confidence_level: 0.80,
        model: 'ets',
      };

      await forecastStage(sourceMap, forecastConfig, context);

      const forecastSQL = executedStatements[1];
      expect(forecastSQL).toContain('horizon = 12');
      expect(forecastSQL).toContain('confidence_level = 0.8');
      expect(forecastSQL).toContain("model = 'ets'");
    });
  });

  describe('group_by', () => {
    it('includes group_by columns in forecast() call and both UNION ALL halves', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_grouped' };
      const forecastConfig = {
        timestamp: 'month',
        value: 'revenue',
        group_by: ['region'],
      };

      await forecastStage(sourceMap, forecastConfig, context);

      // Step 1: source table includes group_by columns
      expect(executedStatements[0]).toContain('region');

      // Step 2: forecast() includes group_by parameter
      expect(executedStatements[1]).toContain("group_by = ['region']");

      // Step 3: UNION ALL includes group_by in both halves and ORDER BY
      const unionSQL = executedStatements[2];
      // Both halves should reference region
      const [beforeUnion, afterUnion] = unionSQL.split('UNION ALL');
      expect(beforeUnion).toContain('region');
      expect(afterUnion).toContain('region');
      // ORDER BY should include group_by before timestamp
      expect(unionSQL).toContain('ORDER BY region, month');
    });

    it('maintains consistent column order in both UNION ALL halves', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_grouped' };
      const forecastConfig = {
        timestamp: 'month',
        value: 'revenue',
        group_by: ['region'],
      };

      await forecastStage(sourceMap, forecastConfig, context);

      const unionSQL = executedStatements[2];
      const [beforeUnion, afterUnion] = unionSQL.split('UNION ALL');

      // Both halves must have columns in order: timestamp, value, group_by, forecast_cols
      // Historical: month, revenue, region, NULL as forecast, ...
      expect(beforeUnion).toMatch(/month.*revenue.*region.*NULL as forecast/);
      // Forecast: forecast_timestamp as month, NULL as revenue, region, forecast, ...
      expect(afterUnion).toMatch(/as month.*NULL as revenue.*region.*forecast/);
    });

    it('handles multiple group_by columns', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_multi_group' };
      const forecastConfig = {
        timestamp: 'date',
        value: 'amount',
        group_by: ['region', 'category'],
      };

      await forecastStage(sourceMap, forecastConfig, context);

      // Step 2: forecast() includes all group_by columns
      expect(executedStatements[1]).toContain("group_by = ['region', 'category']");

      // Step 3: ORDER BY includes both group_by columns
      expect(executedStatements[2]).toContain('ORDER BY region, category, date');
    });
  });

  describe('defaults', () => {
    it('uses horizon=3, confidence_level=0.95, model=auto when omitted', async () => {
      const { context, executedStatements } = createMockContext();
      const sourceMap = { _result: '__extract_defaults' };
      const forecastConfig = {
        timestamp: 'date',
        value: 'revenue',
      };

      await forecastStage(sourceMap, forecastConfig, context);

      const forecastSQL = executedStatements[1];
      expect(forecastSQL).toContain('horizon = 3');
      expect(forecastSQL).toContain('confidence_level = 0.95');
      expect(forecastSQL).toContain("model = 'auto'");
    });
  });

  describe('multi-source sourceMap', () => {
    it('throws with descriptive error when given multiple tables', async () => {
      const { context } = createMockContext();
      const sourceMap = {
        orders: '__extract_aaa',
        customers: '__extract_bbb',
      };
      const forecastConfig = {
        timestamp: 'date',
        value: 'revenue',
      };

      await expect(
        forecastStage(sourceMap, forecastConfig, context)
      ).rejects.toThrow('forecast stage operates on a single table but received 2 tables');
    });

    it('includes table names in the error message', async () => {
      const { context } = createMockContext();
      const sourceMap = {
        alpha: '__extract_111',
        beta: '__extract_222',
        gamma: '__extract_333',
      };
      const forecastConfig = { timestamp: 'x', value: 'y' };

      await expect(
        forecastStage(sourceMap, forecastConfig, context)
      ).rejects.toThrow('alpha, beta, gamma');
    });
  });

  describe('deterministic hashing', () => {
    it('produces the same output table ID for the same config', async () => {
      const { context: ctx1 } = createMockContext();
      const { context: ctx2 } = createMockContext();
      const sourceMap = { _result: '__extract_111' };
      const config = {
        timestamp: 'date',
        value: 'revenue',
        horizon: 6,
      };

      const result1 = await forecastStage(sourceMap, config, ctx1);
      const result2 = await forecastStage(sourceMap, config, ctx2);

      expect(result1._result).toBe(result2._result);
    });

    it('produces different table IDs for different configs', async () => {
      const { context: ctx1 } = createMockContext();
      const { context: ctx2 } = createMockContext();
      const sourceMap = { _result: '__extract_111' };

      const result1 = await forecastStage(
        sourceMap,
        { timestamp: 'date', value: 'revenue', horizon: 6 },
        ctx1
      );
      const result2 = await forecastStage(
        sourceMap,
        { timestamp: 'month', value: 'sales', horizon: 12 },
        ctx2
      );

      expect(result1._result).not.toBe(result2._result);
    });

    it('output table ID starts with __stage_fcast_ prefix', async () => {
      const { context } = createMockContext();
      const sourceMap = { _result: '__extract_000' };
      const config = { timestamp: 'x', value: 'y' };

      const result = await forecastStage(sourceMap, config, context);

      expect(result._result).toMatch(/^__stage_fcast_[0-9a-f]+$/);
    });
  });

  describe('isolation', () => {
    it('only calls context.execute, never context.runSQL', async () => {
      const { context } = createMockContext();
      const sourceMap = { _result: '__extract_000' };
      const config = {
        timestamp: 'date',
        value: 'revenue',
      };

      await forecastStage(sourceMap, config, context);

      expect(context.execute).toHaveBeenCalledTimes(3);
      expect(context.runSQL).not.toHaveBeenCalled();
    });
  });

  describe('error propagation', () => {
    it('propagates context.execute errors', async () => {
      const context = {
        execute: vi.fn(async () => {
          throw new Error('DuckDB error: extension not loaded');
        }),
        runSQL: vi.fn(),
      };
      const sourceMap = { _result: '__extract_111' };
      const config = { timestamp: 'date', value: 'revenue' };

      await expect(
        forecastStage(sourceMap, config, context)
      ).rejects.toThrow('DuckDB error: extension not loaded');
    });
  });
});

describe('buildForecastSQL', () => {
  it('returns statements array with 3 entries and outputTableId', () => {
    const result = buildForecastSQL('__extract_abc', {
      timestamp: 'date',
      value: 'revenue',
    });

    expect(result.statements).toHaveLength(3);
    expect(result.outputTableId).toMatch(/^__stage_fcast_/);
    expect(result.outputTableId).not.toMatch(/^__stage_fcast_src_/);
    expect(result.outputTableId).not.toMatch(/^__stage_fcast_pred_/);
  });

  it('generates correct intermediate table names', () => {
    const result = buildForecastSQL('__extract_abc', {
      timestamp: 'date',
      value: 'revenue',
    });

    // Step 1 creates src table
    expect(result.statements[0]).toMatch(/"__stage_fcast_src_[0-9a-f]+"/);
    // Step 2 creates pred table
    expect(result.statements[1]).toMatch(/"__stage_fcast_pred_[0-9a-f]+"/);
    // Step 3 creates final table
    expect(result.statements[2]).toContain(`"${result.outputTableId}"`);
  });

  it('only selects timestamp, value, and group_by columns in step 1', () => {
    const result = buildForecastSQL('__input_table', {
      timestamp: 'month',
      value: 'revenue',
      group_by: ['region'],
    });

    // Step 1 should SELECT specific columns, not *
    expect(result.statements[0]).toContain('SELECT month, revenue, region');
    expect(result.statements[0]).not.toContain('SELECT *');
  });

  it('does not include group_by clause when group_by is empty', () => {
    const result = buildForecastSQL('__input', {
      timestamp: 'date',
      value: 'value',
    });

    expect(result.statements[1]).not.toContain('group_by');
  });

  it('is deterministic — same inputs produce same outputs', () => {
    const config = { timestamp: 'date', value: 'revenue', horizon: 6 };
    const r1 = buildForecastSQL('__table_a', config);
    const r2 = buildForecastSQL('__table_a', config);

    expect(r1.outputTableId).toBe(r2.outputTableId);
    expect(r1.statements).toEqual(r2.statements);
  });

  it('different inputs produce different table IDs', () => {
    const r1 = buildForecastSQL('__table_a', { timestamp: 'date', value: 'revenue' });
    const r2 = buildForecastSQL('__table_b', { timestamp: 'date', value: 'revenue' });

    expect(r1.outputTableId).not.toBe(r2.outputTableId);
  });
});
