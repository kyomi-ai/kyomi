// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Tests for Transform SQL Builder (ChartML v2.0)
 *
 * The compiler quotes simple identifiers with double quotes for DuckDB compatibility.
 * Expressions containing parentheses or wildcards are left unquoted.
 */

import { describe, test, expect } from 'vitest';
import { buildAggregateSQL, requiresAggregation } from './transformSQLBuilder.js';

describe('transformSQLBuilder', () => {
  const tableName = '__extract_abc123';

  describe('Passthrough (no transform)', () => {
    test('empty transform spec', () => {
      const spec = {};

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe('SELECT * FROM __extract_abc123');
    });

    test('undefined transform spec', () => {
      const sql = buildAggregateSQL(tableName);

      expect(sql).toBe('SELECT * FROM __extract_abc123');
    });

    test('passthrough with filters only', () => {
      const spec = {
        filters: {
          rules: [
            { field: 'revenue', operator: '>', value: 1000 }
          ]
        }
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe('SELECT * FROM __extract_abc123\nWHERE "revenue" > 1000');
    });

    test('passthrough with sort only', () => {
      const spec = {
        sort: [
          { field: 'revenue', direction: 'desc' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe('SELECT * FROM __extract_abc123\nORDER BY "revenue" DESC');
    });

    test('passthrough with limit only', () => {
      const spec = {
        limit: 100
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe('SELECT * FROM __extract_abc123\nLIMIT 100');
    });

    test('passthrough with filters, sort, and limit', () => {
      const spec = {
        filters: {
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' }
          ]
        },
        sort: [
          { field: 'revenue', direction: 'desc' }
        ],
        limit: 50,
        offset: 100
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe("SELECT * FROM __extract_abc123\nWHERE \"category\" = 'Electronics'\nORDER BY \"revenue\" DESC\nLIMIT 50 OFFSET 100");
    });
  });

  describe('Simple cases', () => {
    test('simple dimensions only (no aggregation)', () => {
      const spec = {
        dimensions: ['product', 'region']
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe(
        'SELECT\n  "product",\n  "region"\nFROM __extract_abc123'
      );
    });

    test('dimensions with column rename', () => {
      const spec = {
        dimensions: [
          { column: 'sale_date', name: 'date' },
          'product'
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe(
        'SELECT\n  sale_date as "date",\n  "product"\nFROM __extract_abc123'
      );
    });

    test('calculated dimension', () => {
      const spec = {
        dimensions: [
          'product',
          { column: "DATE_TRUNC(sale_date, 'MONTH')", name: 'month' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe(
        "SELECT\n  \"product\",\n  DATE_TRUNC(sale_date, 'MONTH') as \"month\"\nFROM __extract_abc123"
      );
    });
  });

  describe('Aggregations', () => {
    test('simple aggregation', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toBe(
        'SELECT\n  "product",\n  SUM("revenue") as "total_revenue"\nFROM __extract_abc123\nGROUP BY "product"'
      );
    });

    test('multiple aggregations', () => {
      const spec = {
        dimensions: ['product', 'region'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
          { column: 'units', aggregation: 'sum', name: 'total_units' },
          { column: 'customer_id', aggregation: 'countDistinct', name: 'unique_customers' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('SUM("revenue") as "total_revenue"');
      expect(sql).toContain('SUM("units") as "total_units"');
      expect(sql).toContain('COUNT(DISTINCT "customer_id") as "unique_customers"');
      expect(sql).toContain('GROUP BY "product", "region"');
    });

    test('pre-aggregation calculation', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'quantity * unit_price', aggregation: 'sum', name: 'total_line_value' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('SUM(quantity * unit_price) as "total_line_value"');
    });

    test('calculated dimension with aggregation', () => {
      const spec = {
        dimensions: [
          'product',
          { column: "DATE_TRUNC(sale_date, 'MONTH')", name: 'month' }
        ],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain("DATE_TRUNC(sale_date, 'MONTH') as \"month\"");
      expect(sql).toContain('SUM("revenue") as "total_revenue"');
      expect(sql).toContain("GROUP BY \"product\", DATE_TRUNC(sale_date, 'MONTH')");
    });
  });

  describe('Calculated measures (post-aggregation)', () => {
    test('simple calculated measure', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
          { column: 'units', aggregation: 'sum', name: 'total_units' },
          { expression: 'total_revenue / total_units', name: 'avg_price' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('SUM("revenue") as "total_revenue"');
      expect(sql).toContain('SUM("units") as "total_units"');
      expect(sql).toContain('(SUM("revenue") / SUM("units")) as "avg_price"');
    });

    test('chained calculated measures', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
          { column: 'cost', aggregation: 'sum', name: 'total_cost' },
          { expression: 'total_revenue - total_cost', name: 'profit' },
          { expression: 'profit / total_revenue', name: 'profit_margin' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('SUM("revenue") as "total_revenue"');
      expect(sql).toContain('SUM("cost") as "total_cost"');
      expect(sql).toContain('(SUM("revenue") - SUM("cost")) as "profit"');
      // profit_margin should resolve profit to its expression
      expect(sql).toContain('((SUM("revenue") - SUM("cost")) / SUM("revenue")) as "profit_margin"');
    });
  });

  describe('Filters', () => {
    test('simple WHERE filter (pre-aggregation)', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ],
        filters: {
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' }
          ]
        }
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain("WHERE \"category\" = 'Electronics'");
    });

    test('simple HAVING filter (post-aggregation)', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ],
        filters: {
          rules: [
            { field: 'total_revenue', operator: '>=', value: 50000 }
          ]
        }
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('HAVING SUM("revenue") >= 50000');
    });

    test('mixed WHERE and HAVING filters', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ],
        filters: {
          combinator: 'and',
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' },
            { field: 'revenue', operator: '>', value: 100 },
            { field: 'total_revenue', operator: '>=', value: 50000 }
          ]
        }
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain("WHERE \"category\" = 'Electronics' AND \"revenue\" > 100");
      expect(sql).toContain('HAVING SUM("revenue") >= 50000');
    });

    test('filter with OR combinator', () => {
      const spec = {
        dimensions: ['product'],
        filters: {
          combinator: 'or',
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' },
            { field: 'category', operator: '=', value: 'Appliances' }
          ]
        }
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain("WHERE \"category\" = 'Electronics' OR \"category\" = 'Appliances'");
    });

    test('filter with IN operator', () => {
      const spec = {
        dimensions: ['product'],
        filters: {
          rules: [
            { field: 'region', operator: 'in', value: ['North', 'South', 'East'] }
          ]
        }
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain("WHERE \"region\" IN ('North', 'South', 'East')");
    });
  });

  describe('Sorting', () => {
    test('simple sort', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ],
        sort: [
          { field: 'total_revenue', direction: 'desc' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('ORDER BY "total_revenue" DESC');
    });

    test('multi-column sort', () => {
      const spec = {
        dimensions: ['product', 'region'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ],
        sort: [
          { field: 'region', direction: 'asc' },
          { field: 'total_revenue', direction: 'desc' }
        ]
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('ORDER BY "region" ASC, "total_revenue" DESC');
    });
  });

  describe('Limit and offset', () => {
    test('limit only', () => {
      const spec = {
        dimensions: ['product'],
        limit: 100
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('LIMIT 100');
    });

    test('limit with offset', () => {
      const spec = {
        dimensions: ['product'],
        limit: 100,
        offset: 200
      };

      const sql = buildAggregateSQL(tableName, spec);

      expect(sql).toContain('LIMIT 100 OFFSET 200');
    });
  });

  describe('Complex scenarios', () => {
    test('full spec with all features', () => {
      const spec = {
        dimensions: [
          'product',
          { column: "DATE_TRUNC(sale_date, 'MONTH')", name: 'month' }
        ],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' },
          { column: 'units', aggregation: 'sum', name: 'total_units' },
          { column: 'quantity * unit_price', aggregation: 'sum', name: 'total_line_value' },
          { expression: 'total_revenue / total_units', name: 'avg_price' }
        ],
        filters: {
          combinator: 'and',
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' },
            { field: 'revenue', operator: '>', value: 100 },
            { field: 'total_revenue', operator: '>=', value: 50000 }
          ]
        },
        sort: [
          { field: 'month', direction: 'asc' },
          { field: 'total_revenue', direction: 'desc' }
        ],
        limit: 100
      };

      const sql = buildAggregateSQL(tableName, spec);

      // Check all parts are present
      expect(sql).toContain('SELECT');
      expect(sql).toContain('"product"');
      expect(sql).toContain("DATE_TRUNC(sale_date, 'MONTH') as \"month\"");
      expect(sql).toContain('SUM("revenue") as "total_revenue"');
      expect(sql).toContain('SUM("units") as "total_units"');
      expect(sql).toContain('SUM(quantity * unit_price) as "total_line_value"');
      expect(sql).toContain('(SUM("revenue") / SUM("units")) as "avg_price"');
      expect(sql).toContain("WHERE \"category\" = 'Electronics' AND \"revenue\" > 100");
      expect(sql).toContain("GROUP BY \"product\", DATE_TRUNC(sale_date, 'MONTH')");
      expect(sql).toContain('HAVING SUM("revenue") >= 50000');
      expect(sql).toContain('ORDER BY "month" ASC, "total_revenue" DESC');
      expect(sql).toContain('LIMIT 100');
    });
  });

  describe('requiresAggregation', () => {
    test('returns false for dimensions only', () => {
      const spec = {
        dimensions: ['product', 'region']
      };

      expect(requiresAggregation(spec)).toBe(false);
    });

    test('returns true for aggregated measures', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ]
      };

      expect(requiresAggregation(spec)).toBe(true);
    });

    test('returns true for calculated measures', () => {
      const spec = {
        dimensions: ['product'],
        measures: [
          { expression: 'total_revenue / total_units', name: 'avg_price' }
        ]
      };

      expect(requiresAggregation(spec)).toBe(true);
    });
  });
});
