// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Test parsing the actual mock-dashboards-v2.md file
 */

import { describe, test, expect } from 'vitest';
import { parseMarkdownChartML, validateMarkdownChartML } from './markdownChartMLParser.js';
import { readFileSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';

const __dirname = dirname(fileURLToPath(import.meta.url));

describe('Mock Dashboards v2 Markdown Parsing', () => {
  test('parse mock-dashboards-v2.md', () => {
    const mockDashboardPath = join(__dirname, '../../../mock-dashboards-v2.md');
    const markdown = readFileSync(mockDashboardPath, 'utf-8');

    const result = parseMarkdownChartML(markdown);

    // Should have 1 dataset defined (q1_sales)
    expect(Object.keys(result.datasets)).toHaveLength(1);
    expect(result.datasets).toHaveProperty('q1_sales');
    expect(result.datasets.q1_sales.source).toBe('inline');
    expect(result.datasets.q1_sales.data).toHaveLength(9); // 3 months * 3 regions

    // Should have multiple charts
    expect(result.charts.length).toBeGreaterThan(0);

    // Verify first chart references the dataset
    const firstChart = result.charts[0];
    expect(firstChart.title).toBe('Monthly Revenue Trend');
    expect(firstChart.extract.dataset).toBe('q1_sales');
    expect(firstChart.transform).toBeDefined();
    expect(firstChart.visualize.type).toBe('bar');
  });

  test('validate mock-dashboards-v2.md', () => {
    const mockDashboardPath = join(__dirname, '../../../mock-dashboards-v2.md');
    const markdown = readFileSync(mockDashboardPath, 'utf-8');

    const validation = validateMarkdownChartML(markdown);


    expect(validation.valid).toBe(true);
    expect(validation.errors).toHaveLength(0);
  });

  test('all charts in mock dashboards have required sections', () => {
    const mockDashboardPath = join(__dirname, '../../../mock-dashboards-v2.md');
    const markdown = readFileSync(mockDashboardPath, 'utf-8');

    const { charts } = parseMarkdownChartML(markdown);

    charts.forEach((chart, index) => {
      expect(chart.extract, `Chart ${index + 1} missing extract`).toBeDefined();
      expect(chart.visualize, `Chart ${index + 1} missing visualize`).toBeDefined();
      expect(chart.visualize.type, `Chart ${index + 1} missing visualize type`).toBeDefined();
    });
  });

  test('charts using dataset reference resolve correctly', () => {
    const mockDashboardPath = join(__dirname, '../../../mock-dashboards-v2.md');
    const markdown = readFileSync(mockDashboardPath, 'utf-8');

    const { datasets, charts } = parseMarkdownChartML(markdown);

    const chartsUsingDataset = charts.filter(chart => chart.extract?.dataset);

    expect(chartsUsingDataset.length).toBeGreaterThan(0);

    chartsUsingDataset.forEach(chart => {
      const datasetName = chart.extract.dataset;
      expect(datasets[datasetName], `Dataset ${datasetName} not found`).toBeDefined();
    });
  });
});
