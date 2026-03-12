// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Integration tests for ChartML v2 Pipeline
 */

import { describe, test, expect, beforeAll, vi } from 'vitest';
import {
  executeChartmlPipeline,
  validateChartmlSpec,
  loadChartmlFromYaml,
  saveChartmlToYaml
} from './chartmlPipeline.js';

describe('ChartML v2 Pipeline Integration', () => {
  let mockDuckDB;

  beforeAll(() => {
    // Mock DuckDB instance
    mockDuckDB = {
      connect: vi.fn().mockResolvedValue({
        query: vi.fn(),
        insertArrowTable: vi.fn(),
        close: vi.fn()
      })
    };
  });

  test('dataset reference from local datasets', async () => {
    const spec = {
      version: 2,
      title: 'Sales Chart',
      extract: {
        dataset: 'sales_data'  // Reference to local dataset
      },
      visualize: {
        type: 'bar',
        columns: 'product',
        rows: 'sales'
      }
    };

    const localDatasets = {
      sales_data: {
        source: 'inline',
        data: [
          { product: 'A', sales: 100 },
          { product: 'B', sales: 150 }
        ]
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB, { datasets: localDatasets });

    expect(result.data).toHaveLength(2);
    expect(result.data[0]).toEqual({ product: 'A', sales: 100 });
  });

  test('dataset reference from external resolver', async () => {
    const spec = {
      version: 2,
      extract: {
        dataset: 'external_sales_data'
      },
      visualize: {
        type: 'bar',
        columns: 'region',
        rows: 'revenue'
      }
    };

    const datasetResolver = vi.fn().mockResolvedValue({
      source: 'inline',
      data: [
        { region: 'North', revenue: 1000 },
        { region: 'South', revenue: 1500 }
      ]
    });

    const result = await executeChartmlPipeline(spec, mockDuckDB, { datasetResolver });

    expect(datasetResolver).toHaveBeenCalledWith('external_sales_data');
    expect(result.data).toHaveLength(2);
  });

  test('local dataset takes precedence over external resolver', async () => {
    const spec = {
      version: 2,
      extract: {
        dataset: 'my_data'
      },
      visualize: {
        type: 'bar',
        columns: 'x',
        rows: 'y'
      }
    };

    const localDatasets = {
      my_data: {
        source: 'inline',
        data: [{ x: 'local', y: 100 }]
      }
    };

    const datasetResolver = vi.fn().mockResolvedValue({
      source: 'inline',
      data: [{ x: 'external', y: 200 }]
    });

    const result = await executeChartmlPipeline(spec, mockDuckDB, {
      datasets: localDatasets,
      datasetResolver
    });

    expect(datasetResolver).not.toHaveBeenCalled();
    expect(result.data[0].x).toBe('local');
  });

  test('dataset not found throws error', async () => {
    const spec = {
      version: 2,
      extract: {
        dataset: 'nonexistent_dataset'
      },
      visualize: {
        type: 'bar'
      }
    };

    await expect(executeChartmlPipeline(spec, mockDuckDB)).rejects.toThrow('Dataset not found: nonexistent_dataset');
  });

  test('inline data passthrough pipeline', async () => {
    const spec = {
      version: 2,
      title: 'Simple Sales Chart',
      extract: {
        source: 'inline',
        data: [
          { product: 'A', sales: 100 },
          { product: 'B', sales: 150 },
          { product: 'C', sales: 80 }
        ]
      },
      // No transform - passthrough mode
      visualize: {
        type: 'bar',
        columns: 'product',
        rows: 'sales'
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.data).toHaveLength(3);
    expect(result.data[0]).toEqual({ product: 'A', sales: 100 });
    expect(result.config.x.label).toBe('product');
    expect(result.config.y.label).toBe('sales');
    expect(result.metadata.extractedRows).toBe(3);
    expect(result.metadata.transformedRows).toBe(3);
  });

  test('inline data with transform pipeline', async () => {
    const spec = {
      version: 2,
      title: 'Regional Sales Summary',
      extract: {
        source: 'inline',
        data: [
          { region: 'North', product: 'A', revenue: 100 },
          { region: 'North', product: 'B', revenue: 150 },
          { region: 'South', product: 'A', revenue: 120 },
          { region: 'South', product: 'B', revenue: 180 }
        ]
      },
      transform: {
        dimensions: ['region'],
        measures: [
          { column: 'revenue', aggregation: 'sum', name: 'total_revenue' }
        ]
      },
      visualize: {
        type: 'bar',
        columns: 'region',
        rows: 'total_revenue'
      }
    };

    // Mock DuckDB response for aggregated data
    const mockConn = {
      query: vi.fn().mockResolvedValue({
        toArray: () => [
          { toJSON: () => ({ region: 'North', total_revenue: 250 }) },
          { toJSON: () => ({ region: 'South', total_revenue: 300 }) }
        ]
      }),
      insertArrowTable: vi.fn(),
      close: vi.fn()
    };

    mockDuckDB.connect.mockResolvedValue(mockConn);

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.data).toHaveLength(2);
    expect(result.data[0]).toEqual({ region: 'North', total_revenue: 250 });
    expect(result.data[1]).toEqual({ region: 'South', total_revenue: 300 });
    expect(result.config.type).toBeUndefined(); // Plot configs don't include type
    expect(result.metadata.extractedRows).toBe(4);
  });

  test('multi-series line chart pipeline', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { month: 'Jan', revenue: 1000, cost: 600 },
          { month: 'Feb', revenue: 1200, cost: 700 },
          { month: 'Mar', revenue: 1100, cost: 650 }
        ]
      },
      visualize: {
        type: 'line',
        columns: 'month',
        rows: [
          { field: 'revenue', color: '#4285f4' },
          { field: 'cost', color: '#ea4335' }
        ]
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.config.marks).toHaveLength(2);
    expect(result.data).toHaveLength(3);
  });

  test('grouped bar chart with color encoding', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { month: 'Jan', region: 'North', sales: 100 },
          { month: 'Jan', region: 'South', sales: 120 },
          { month: 'Feb', region: 'North', sales: 110 },
          { month: 'Feb', region: 'South', sales: 130 }
        ]
      },
      visualize: {
        type: 'bar',
        mode: 'grouped',
        columns: 'month',
        rows: 'sales',
        marks: { color: 'region' }
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.config.color.legend).toBe(true);
    expect(result.data).toHaveLength(4);
  });

  test('scatter plot with multiple encodings', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { x: 10, y: 20, category: 'A', size: 100 },
          { x: 15, y: 25, category: 'B', size: 150 }
        ]
      },
      visualize: {
        type: 'scatter',
        columns: 'x',
        rows: 'y',
        marks: {
          color: 'category',
          size: 'size'
        }
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.config.color.legend).toBe(true);
    expect(result.data).toHaveLength(2);
  });

  test('pie chart pipeline', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { category: 'Electronics', total_sales: 5000 },
          { category: 'Clothing', total_sales: 3000 },
          { category: 'Food', total_sales: 2000 }
        ]
      },
      visualize: {
        type: 'pie',
        columns: 'category',
        rows: 'total_sales'
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.config.type).toBe('pie');
    expect(result.config.categoryField).toBe('category');
    expect(result.config.valueField).toBe('total_sales');
  });

  test('table visualization pipeline', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { product: 'A', category: 'Electronics', sales: 1000 },
          { product: 'B', category: 'Clothing', sales: 800 }
        ]
      },
      visualize: {
        type: 'table',
        columns: ['product', 'category', 'sales']
      }
    };

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.config.type).toBe('table');
    expect(result.config.data).toBe(result.data);
  });

  test('pipeline with calculated measures', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { product: 'A', quantity: 10, unit_price: 50 },
          { product: 'B', quantity: 15, unit_price: 40 }
        ]
      },
      transform: {
        dimensions: ['product'],
        measures: [
          { column: 'quantity * unit_price', aggregation: 'sum', name: 'total_revenue' },
          { column: 'quantity', aggregation: 'sum', name: 'total_units' },
          { expression: 'total_revenue / total_units', name: 'avg_price' }
        ]
      },
      visualize: {
        type: 'bar',
        columns: 'product',
        rows: 'total_revenue'
      }
    };

    // Mock DuckDB response
    const mockConn = {
      query: vi.fn().mockResolvedValue({
        toArray: () => [
          { toJSON: () => ({ product: 'A', total_revenue: 500, total_units: 10, avg_price: 50 }) },
          { toJSON: () => ({ product: 'B', total_revenue: 600, total_units: 15, avg_price: 40 }) }
        ]
      }),
      insertArrowTable: vi.fn(),
      close: vi.fn()
    };

    mockDuckDB.connect.mockResolvedValue(mockConn);

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.data).toHaveLength(2);
    expect(result.data[0].avg_price).toBe(50);
    expect(result.data[1].avg_price).toBe(40);
  });

  test('pipeline with filters', async () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [
          { category: 'Electronics', product: 'A', sales: 1000 },
          { category: 'Clothing', product: 'B', sales: 800 },
          { category: 'Electronics', product: 'C', sales: 1200 }
        ]
      },
      transform: {
        dimensions: ['product'],
        measures: [
          { column: 'sales', aggregation: 'sum', name: 'total_sales' }
        ],
        filters: {
          combinator: 'and',
          rules: [
            { field: 'category', operator: '=', value: 'Electronics' }
          ]
        }
      },
      visualize: {
        type: 'bar',
        columns: 'product',
        rows: 'total_sales'
      }
    };

    // Mock DuckDB response (filtered to Electronics only)
    const mockConn = {
      query: vi.fn().mockResolvedValue({
        toArray: () => [
          { toJSON: () => ({ product: 'A', total_sales: 1000 }) },
          { toJSON: () => ({ product: 'C', total_sales: 1200 }) }
        ]
      }),
      insertArrowTable: vi.fn(),
      close: vi.fn()
    };

    mockDuckDB.connect.mockResolvedValue(mockConn);

    const result = await executeChartmlPipeline(spec, mockDuckDB);

    expect(result.data).toHaveLength(2);
    expect(result.data.every(row => row.product === 'A' || row.product === 'C')).toBe(true);
  });
});

describe('ChartML Validation', () => {
  test('valid spec passes validation', () => {
    const spec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [{ x: 1, y: 2 }]
      },
      visualize: {
        type: 'bar',
        columns: 'x',
        rows: 'y'
      }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  test('missing version fails validation', () => {
    const spec = {
      extract: { source: 'inline', data: [] },
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('ChartML version must be 2');
  });

  test('missing extract fails validation', () => {
    const spec = {
      version: 2,
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('Missing required "extract" section');
  });

  test('missing visualize fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'inline', data: [] }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('Missing required "visualize" section');
  });

  test('invalid extract source fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'mongodb', query: 'db.collection.find()' },
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('Invalid extract source'))).toBe(true);
  });

  test('inline source without data fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'inline' },
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('Inline extract source requires "data" property');
  });

  test('bigquery source without query fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'bigquery' },
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('bigquery source requires "query" property');
  });

  test('file source without path or format fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'file', path: '/data/sales.csv' },
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('File source requires "path" and "format" properties');
  });

  test('http source without url fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'http' },
      visualize: { type: 'bar' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors).toContain('HTTP source requires "url" property');
  });

  test('invalid visualize type fails validation', () => {
    const spec = {
      version: 2,
      extract: { source: 'inline', data: [] },
      visualize: { type: 'bubble3d' }
    };

    const result = validateChartmlSpec(spec);

    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('Invalid visualize type'))).toBe(true);
  });
});

describe('YAML Import/Export', () => {
  test('load ChartML from YAML', async () => {
    const yaml = `
version: 2
title: Sales Dashboard

extract:
  source: inline
  data:
    - product: A
      sales: 100
    - product: B
      sales: 150

visualize:
  type: bar
  columns: product
  rows: sales
`;

    const spec = await loadChartmlFromYaml(yaml);

    expect(spec.version).toBe(2);
    expect(spec.title).toBe('Sales Dashboard');
    expect(spec.extract.source).toBe('inline');
    expect(spec.extract.data).toHaveLength(2);
    expect(spec.visualize.type).toBe('bar');
  });

  test('save ChartML to YAML', async () => {
    const spec = {
      version: 2,
      title: 'Revenue Chart',
      extract: {
        source: 'inline',
        data: [
          { month: 'Jan', revenue: 1000 },
          { month: 'Feb', revenue: 1200 }
        ]
      },
      visualize: {
        type: 'line',
        columns: 'month',
        rows: 'revenue'
      }
    };

    const yaml = await saveChartmlToYaml(spec);

    expect(yaml).toContain('version: 2');
    expect(yaml).toContain('title: Revenue Chart');
    expect(yaml).toContain('source: inline');
    expect(yaml).toContain('type: line');
    expect(yaml).toContain('month: Jan');
    expect(yaml).toContain('revenue: 1000');
  });

  test('round-trip YAML conversion', async () => {
    const originalSpec = {
      version: 2,
      extract: {
        source: 'inline',
        data: [{ x: 1, y: 2 }]
      },
      transform: {
        dimensions: ['x'],
        measures: [{ column: 'y', aggregation: 'sum', name: 'total_y' }]
      },
      visualize: {
        type: 'bar',
        columns: 'x',
        rows: 'total_y'
      }
    };

    const yaml = await saveChartmlToYaml(originalSpec);
    const loadedSpec = await loadChartmlFromYaml(yaml);

    expect(loadedSpec).toEqual(originalSpec);
  });
});
