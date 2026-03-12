// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Tests for Markdown ChartML Parser
 */

import { describe, test, expect } from 'vitest';
import {
  parseMarkdownChartML,
  extractDatasets,
  extractCharts,
  getChartContext,
  validateMarkdownChartML
} from './markdownChartMLParser.js';

describe('Markdown ChartML Parser', () => {
  test('parse markdown with dataset and chart blocks', () => {
    const markdown = `
# Sales Dashboard

## Datasets

\`\`\`dataset
name: sales_data
source: inline
data:
  - product: A
    revenue: 100
  - product: B
    revenue: 150
\`\`\`

## Revenue Chart

\`\`\`chartml
version: 2
title: "Revenue by Product"

extract:
  dataset: sales_data

visualize:
  type: bar
  columns: product
  rows: revenue
\`\`\`
`;

    const result = parseMarkdownChartML(markdown);

    expect(result.datasets).toHaveProperty('sales_data');
    expect(result.datasets.sales_data.source).toBe('inline');
    expect(result.datasets.sales_data.data).toHaveLength(2);

    expect(result.charts).toHaveLength(1);
    expect(result.charts[0].title).toBe('Revenue by Product');
    expect(result.charts[0].extract.dataset).toBe('sales_data');
  });

  test('parse multiple datasets', () => {
    const markdown = `
\`\`\`dataset
name: dataset1
source: inline
data: []
\`\`\`

\`\`\`dataset
name: dataset2
source: bigquery
query: "SELECT * FROM table"
\`\`\`
`;

    const { datasets } = parseMarkdownChartML(markdown);

    expect(Object.keys(datasets)).toHaveLength(2);
    expect(datasets.dataset1.source).toBe('inline');
    expect(datasets.dataset2.source).toBe('bigquery');
  });

  test('parse multiple charts', () => {
    const markdown = `
\`\`\`chartml
version: 2
title: "Chart 1"
extract:
  source: inline
  data: []
visualize:
  type: bar
\`\`\`

\`\`\`chartml
version: 2
title: "Chart 2"
extract:
  source: inline
  data: []
visualize:
  type: line
\`\`\`
`;

    const { charts } = parseMarkdownChartML(markdown);

    expect(charts).toHaveLength(2);
    expect(charts[0].title).toBe('Chart 1');
    expect(charts[1].title).toBe('Chart 2');
  });

  test('extractDatasets helper', () => {
    const markdown = `
\`\`\`dataset
name: my_data
source: inline
data: []
\`\`\`
`;

    const datasets = extractDatasets(markdown);

    expect(datasets).toHaveProperty('my_data');
    expect(datasets.my_data.source).toBe('inline');
  });

  test('extractCharts helper', () => {
    const markdown = `
\`\`\`chartml
version: 2
title: "Test Chart"
extract:
  source: inline
  data: []
visualize:
  type: bar
\`\`\`
`;

    const charts = extractCharts(markdown);

    expect(charts).toHaveLength(1);
    expect(charts[0].title).toBe('Test Chart');
  });

  test('getChartContext extracts surrounding markdown', () => {
    const markdown = `
# Dashboard

## Sales Performance

This chart shows monthly sales trends.

\`\`\`chartml
version: 2
title: "Monthly Sales"
extract:
  source: inline
  data: []
visualize:
  type: line
\`\`\`

## Regional Analysis

\`\`\`chartml
version: 2
title: "Regional Breakdown"
extract:
  source: inline
  data: []
visualize:
  type: pie
\`\`\`
`;

    const context0 = getChartContext(markdown, 0);
    expect(context0.title).toBe('Sales Performance');
    expect(context0.description).toContain('This chart shows monthly sales trends');
    expect(context0.chart.title).toBe('Monthly Sales');

    const context1 = getChartContext(markdown, 1);
    expect(context1.title).toBe('Regional Analysis');
    expect(context1.chart.title).toBe('Regional Breakdown');
  });

  test('inline extract (no dataset reference)', () => {
    const markdown = `
\`\`\`chartml
version: 2
title: "Inline Data Chart"

extract:
  source: inline
  data:
    - x: 1
      y: 2
    - x: 2
      y: 4

visualize:
  type: scatter
  columns: x
  rows: y
\`\`\`
`;

    const { charts } = parseMarkdownChartML(markdown);

    expect(charts).toHaveLength(1);
    expect(charts[0].extract.source).toBe('inline');
    expect(charts[0].extract.data).toHaveLength(2);
  });

  test('markdown with mixed content', () => {
    const markdown = `
# My Dashboard

This is a regular paragraph.

- Bullet point 1
- Bullet point 2

\`\`\`javascript
// This is a JavaScript code block, not ChartML
const x = 10;
\`\`\`

\`\`\`dataset
name: real_data
source: inline
data:
  - value: 100
\`\`\`

Some more text.

\`\`\`chartml
version: 2
extract:
  dataset: real_data
visualize:
  type: bar
\`\`\`

Final paragraph.
`;

    const result = parseMarkdownChartML(markdown);

    // Should only extract dataset and chartml blocks, ignore JavaScript
    expect(Object.keys(result.datasets)).toHaveLength(1);
    expect(result.charts).toHaveLength(1);
  });
});

describe('Markdown ChartML Validation', () => {
  test('valid markdown passes validation', () => {
    const markdown = `
\`\`\`dataset
name: data1
source: inline
data: []
\`\`\`

\`\`\`chartml
version: 2
extract:
  dataset: data1
visualize:
  type: bar
\`\`\`
`;

    const result = validateMarkdownChartML(markdown);

    expect(result.valid).toBe(true);
    expect(result.errors).toHaveLength(0);
  });

  test('missing visualize section fails validation', () => {
    const markdown = `
\`\`\`chartml
version: 2
extract:
  source: inline
  data: []
\`\`\`
`;

    const result = validateMarkdownChartML(markdown);

    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('missing required "visualize"'))).toBe(true);
  });

  test('missing extract section fails validation', () => {
    const markdown = `
\`\`\`chartml
version: 2
visualize:
  type: bar
\`\`\`
`;

    const result = validateMarkdownChartML(markdown);

    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('missing required "extract"'))).toBe(true);
  });

  test('undefined dataset reference warns', () => {
    const markdown = `
\`\`\`chartml
version: 2
extract:
  dataset: nonexistent_dataset
visualize:
  type: bar
\`\`\`
`;

    const result = validateMarkdownChartML(markdown);

    expect(result.warnings.some(w => w.includes('nonexistent_dataset'))).toBe(true);
  });

  test('no charts warns but does not error', () => {
    const markdown = `
# Just a regular markdown document

No charts here.
`;

    const result = validateMarkdownChartML(markdown);

    expect(result.valid).toBe(true);
    expect(result.warnings.some(w => w.includes('No chartml blocks found'))).toBe(true);
  });

  test('duplicate dataset names fail validation', () => {
    const markdown = `
\`\`\`dataset
name: duplicate
source: inline
data: []
\`\`\`

\`\`\`dataset
name: duplicate
source: bigquery
query: "SELECT 1"
\`\`\`
`;

    const result = validateMarkdownChartML(markdown);

    expect(result.valid).toBe(false);
    expect(result.errors.some(e => e.includes('Duplicate dataset names'))).toBe(true);
  });

  test('dataset without name throws error during parse', () => {
    const markdown = `
\`\`\`dataset
source: inline
data: []
\`\`\`
`;

    // Should not throw, just skip the invalid block
    const result = parseMarkdownChartML(markdown);
    expect(Object.keys(result.datasets)).toHaveLength(0);
  });
});
