// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * @chartml/core - A declarative markup language for creating beautiful, interactive data visualizations
 *
 * This is the core ChartML library with built-in support for inline and HTTP data sources.
 * Visualization rendering is pure D3 with no external dependencies beyond the data layer.
 *
 * @example
 * import { renderChart } from '@chartml/core';
 *
 * const spec = `
 * data:
 *   - month: Jan
 *     revenue: 45000
 *   - month: Feb
 *     revenue: 52000
 *
 * visualize:
 *   type: bar
 *   columns: month
 *   rows: revenue
 *   style:
 *     title: "Monthly Revenue"
 * `;
 *
 * await renderChart(spec, document.getElementById('chart'));
 */

import * as yaml from 'js-yaml';
import { renderD3CartesianChart } from './d3CartesianChart.js';
import { renderD3ScatterPlot } from './d3ScatterPlot.js';
import { renderMetricCard } from './d3MetricCard.js';
import { renderPieChart } from './d3PieChart.js';
import { mapChartMLToD3Config } from './d3ChartMapper.js';
import { getChartColors } from './colorUtils.js';

/**
 * Built-in color palettes
 */
const DEFAULT_PALETTES = {
  autumn_forest: [
    '#d97706', '#f59e0b', '#dc2626', '#b91c1c',
    '#65a30d', '#84cc16', '#ca8a04', '#eab308',
    '#f97316', '#ea580c', '#16a34a', '#15803d'
  ],
  spectrum_pro: [
    '#3b82f6', '#8b5cf6', '#ec4899', '#f43f5e',
    '#f97316', '#f59e0b', '#eab308', '#84cc16',
    '#10b981', '#14b8a6', '#06b6d4', '#0ea5e9'
  ],
  horizon_suite: [
    '#0891b2', '#06b6d4', '#0ea5e9', '#3b82f6',
    '#6366f1', '#8b5cf6', '#a855f7', '#c026d3',
    '#d946ef', '#ec4899', '#f43f5e', '#fb7185'
  ]
};

/**
 * ChartML Renderer Class
 *
 * Main class for rendering ChartML specifications into interactive D3 visualizations.
 * Supports plugin system for extensible data sources and aggregate middleware.
 */
export class ChartML {
  constructor(options = {}) {
    this.dataSources = new Map();
    this.aggregateMiddleware = [];
    this.palettes = { ...DEFAULT_PALETTES, ...(options.palettes || {}) };

    // Register built-in data sources
    this._registerBuiltInDataSources();
  }

  /**
   * Register built-in data sources (inline and HTTP)
   */
  _registerBuiltInDataSources() {
    // Inline data source
    this.registerDataSource('inline', async (spec) => {
      if (Array.isArray(spec.data)) {
        return spec.data;
      }
      throw new Error('Inline data source requires data to be an array');
    });

    // HTTP data source
    this.registerDataSource('http', async (spec) => {
      if (typeof spec.data === 'string' && (spec.data.startsWith('http://') || spec.data.startsWith('https://'))) {
        const response = await fetch(spec.data);
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}: ${response.statusText}`);
        }
        const data = await response.json();
        if (!Array.isArray(data)) {
          throw new Error('HTTP data source must return a JSON array');
        }
        return data;
      }
      throw new Error('HTTP data source requires data to be a URL string');
    });
  }

  /**
   * Register a custom data source plugin
   *
   * @param {string} name - Data source name (e.g., 'bigquery', 'postgres')
   * @param {Function} handler - Async function that returns data array
   *
   * @example
   * chartml.registerDataSource('bigquery', async (spec) => {
   *   // Execute BigQuery and return rows
   *   return rows;
   * });
   */
  registerDataSource(name, handler) {
    this.dataSources.set(name, handler);
  }

  /**
   * Register aggregate middleware plugin
   *
   * @param {Function} middleware - Async function that transforms data
   *
   * @example
   * chartml.registerAggregateMiddleware(async (data, aggregateSpec) => {
   *   // Transform data using DuckDB or other engine
   *   return transformedData;
   * });
   */
  registerAggregateMiddleware(middleware) {
    this.aggregateMiddleware.push(middleware);
  }

  /**
   * Resolve data source - determine which handler to use
   */
  async _resolveDataSource(spec) {
    // Inline data (array)
    if (Array.isArray(spec.data)) {
      const handler = this.dataSources.get('inline');
      return await handler(spec);
    }

    // HTTP data (URL string)
    if (typeof spec.data === 'string' && (spec.data.startsWith('http://') || spec.data.startsWith('https://'))) {
      const handler = this.dataSources.get('http');
      return await handler(spec);
    }

    // Object with type property - plugin data source
    if (spec.data && typeof spec.data === 'object' && spec.data.type) {
      const handler = this.dataSources.get(spec.data.type);
      if (!handler) {
        throw new Error(`Unknown data source type: ${spec.data.type}`);
      }
      return await handler(spec);
    }

    throw new Error('Unable to resolve data source. Data must be an array, URL string, or object with type property.');
  }

  /**
   * Apply aggregate middleware if aggregate block exists
   */
  async _applyAggregate(data, aggregateSpec) {
    if (!aggregateSpec || this.aggregateMiddleware.length === 0) {
      return data;
    }

    // Apply each middleware in order
    let result = data;
    for (const middleware of this.aggregateMiddleware) {
      result = await middleware(result, aggregateSpec);
    }
    return result;
  }

  /**
   * Resolve style with palette colors
   */
  _resolveStyle(spec) {
    const style = spec.visualize?.style || {};

    // If colors not specified, use default palette
    if (!style.colors) {
      const paletteKey = spec.style?.palette || Object.keys(this.palettes)[0];
      const palette = this.palettes[paletteKey] || this.palettes[Object.keys(this.palettes)[0]];
      style.colors = palette;
    }

    return {
      ...style,
      width: style.width || 600,
      height: style.height || 400
    };
  }

  /**
   * Render ChartML specification into a DOM container
   *
   * @param {string|object} spec - ChartML YAML string or parsed object
   * @param {HTMLElement} container - DOM element to render into
   *
   * @example
   * await chartml.render(spec, document.getElementById('chart'));
   */
  async render(spec, container) {
    // Parse YAML if string
    const parsedSpec = typeof spec === 'string' ? yaml.load(spec) : spec;

    // Resolve data source
    const data = await this._resolveDataSource(parsedSpec);

    // Apply aggregate middleware if present
    const processedData = await this._applyAggregate(data, parsedSpec.aggregate);

    // Resolve style with palette
    const resolvedStyle = this._resolveStyle(parsedSpec);
    const visualizeSpec = {
      ...parsedSpec.visualize,
      style: resolvedStyle
    };

    // Map ChartML to D3 config
    const { chartType, config } = mapChartMLToD3Config(visualizeSpec, processedData);

    // Render based on chart type
    switch (chartType) {
      case 'cartesian':
        renderD3CartesianChart(container, config.data, config.config);
        break;

      case 'scatter':
        renderD3ScatterPlot(container, processedData, config);
        break;

      case 'pie':
      case 'doughnut':
        renderPieChart(container, config, processedData);
        break;

      case 'metric':
        renderMetricCard(container, config, processedData);
        break;

      case 'table':
        // Table rendering would go here (currently handled by Kyomi's ResizableTable)
        throw new Error('Table rendering not yet implemented in @chartml/core');

      default:
        throw new Error(`Unknown chart type: ${chartType}`);
    }
  }
}

/**
 * Convenience function to render a chart without creating a ChartML instance
 *
 * @param {string|object} spec - ChartML YAML string or parsed object
 * @param {HTMLElement} container - DOM element to render into
 *
 * @example
 * await renderChart(spec, document.getElementById('chart'));
 */
export async function renderChart(spec, container) {
  const chartml = new ChartML();
  await chartml.render(spec, container);
}

// Export utilities for advanced usage
export { getChartColors } from './colorUtils.js';
export { createFormatter } from './formatters.js';
