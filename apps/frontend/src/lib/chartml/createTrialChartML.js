// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Trial ChartML Factory
 *
 * Creates a ChartML instance configured for trial mode.
 * Uses the trial query endpoint instead of authenticated API calls.
 *
 * Key differences from createKyomiChartML:
 * - Uses /api/v1/trial/query endpoint (no auth required, uses trial_access_token)
 * - Only supports the sample "acme-analytics" ClickHouse datasource
 * - Simplified error handling
 */

import { ChartML } from '@chartml/core';
import { createPieChartRenderer } from '@chartml/chart-pie';
import { createScatterPlotRenderer } from '@chartml/chart-scatter';
import { createMetricRenderer } from '@chartml/chart-metric';
import { duckDbMiddleware } from './plugins/duckDbMiddleware.js';
import { createKyomiTableRenderer } from './plugins/kyomiTableRenderer.jsx';
import { executeTrialQuery } from '../../api/trialApi.js';
import { parseDateColumns } from '../../utils/dateParser.js';

/**
 * Trial datasource plugin - uses /api/v1/trial/query endpoint
 */
async function trialDataSource(spec, context = {}) {
  const { query } = spec;

  if (!query) {
    throw new Error('Trial data source requires a "query" field');
  }

  try {
    const result = await executeTrialQuery(query);

    if (!result.columns || !result.rows || result.columns.length === 0) {
      return {
        data: [],
        metadata: {
          format: 'json',
          columns: [],
          rowCount: 0,
          datasource: 'acme-analytics',
          datasource_type: 'clickhouse'
        }
      };
    }

    const columnNames = result.columns.map(col => col.name);

    // Parse datetime columns
    const parsedRows = parseDateColumns(result.columns, result.rows);

    // Convert row arrays to objects
    const rows = parsedRows.map(row => {
      const obj = {};
      columnNames.forEach((colName, idx) => {
        obj[colName] = row[idx];
      });
      return obj;
    });

    return {
      data: rows,
      metadata: {
        format: 'json',
        columns: columnNames,
        rowCount: rows.length,
        datasource: 'acme-analytics',
        datasource_type: 'clickhouse'
      }
    };

  } catch (error) {
    // Enhance error messages for trial mode
    let errorMessage = error.message || 'Query failed';

    if (errorMessage.includes('expired') || errorMessage.includes('token')) {
      errorMessage = 'Your trial session has expired. Please refresh the page to continue.';
    }

    throw new Error(errorMessage);
  }
}

/**
 * Create Kyomi-specific loading indicator with animated logo
 */
function createKyomiLoadingIndicator() {
  const loader = document.createElement('div');
  loader.className = 'absolute inset-0 flex items-center justify-center bg-card/50 backdrop-blur-sm z-10';
  loader.innerHTML = '<img src="/kyomi_animated_logo.svg" alt="Loading chart" class="w-8 h-8" />';
  return loader;
}

/**
 * Create a trial-mode ChartML instance
 *
 * @param {Object} options - Configuration options
 * @param {Array} [options.defaultPalette] - Default color array for charts
 * @returns {ChartML} Configured ChartML instance for trial mode
 */
export function createTrialChartML(options = {}) {
  const { defaultPalette } = options;

  // Create base ChartML instance
  const chartml = new ChartML({
    defaultPalette,
    loadingIndicator: createKyomiLoadingIndicator
  });

  // Override datasource resolver for trial mode
  // Always resolve to our trial datasource, regardless of slug
  chartml.setDatasourceResolver(async (slug, context) => {
    // In trial mode, all datasources resolve to the sample ClickHouse
    return {
      provider: 'trial',
      _resolved_slug: 'acme-analytics',
    };
  });

  // Register trial datasource (handles all queries via /api/v1/trial/query)
  chartml.registerDataSource('trial', trialDataSource);

  // Also register clickhouse to use trial datasource (in case agent specifies it)
  chartml.registerDataSource('clickhouse', trialDataSource);

  // Use DuckDB middleware for transform pipeline (same as regular Kyomi)
  chartml.setTransformMiddleware(async (data, spec, context) => {
    return await duckDbMiddleware(data, spec, { ...context });
  });

  // Register chart renderers (same as regular Kyomi)
  chartml.registerChartRenderer('metric', createMetricRenderer());
  chartml.registerChartRenderer('table', createKyomiTableRenderer());
  chartml.registerChartRenderer('pie', createPieChartRenderer());
  chartml.registerChartRenderer('doughnut', createPieChartRenderer());
  chartml.registerChartRenderer('scatter', createScatterPlotRenderer());

  return chartml;
}

export default createTrialChartML;
