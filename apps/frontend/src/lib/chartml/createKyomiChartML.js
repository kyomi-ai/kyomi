// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Kyomi ChartML Factory
 *
 * Creates a ChartML instance configured with Kyomi-specific plugins:
 * - Data sources: BigQuery, PostgreSQL, ClickHouse, Snowflake, Databricks, Redshift
 * - DuckDB aggregate middleware (high-performance SQL aggregations)
 * - Chart renderers: metric, table, pie, doughnut, scatter
 *
 * BigQuery uses direct API calls (OAuth), other datasources use backend proxy.
 *
 * This factory ensures all Kyomi charts use a consistent ChartML configuration.
 */

import { ChartML, globalRegistry } from '@chartml/core';
import '@chartml/core/style.css';
import { createPieChartRenderer } from '@chartml/chart-pie';
import { createScatterPlotRenderer } from '@chartml/chart-scatter';
import { createMetricRenderer } from '@chartml/chart-metric';
// Import plugins to trigger auto-registration to globalRegistry
import { bigQueryDataSource } from './plugins/bigQueryDataSource.js';
// Generic proxy datasource auto-registers all backend-proxied datasources:
// postgres, mysql, clickhouse, snowflake, databricks, redshift
import './plugins/genericProxyDataSource.js';
import { duckDbMiddleware } from './plugins/duckDbMiddleware.js';
import { createKyomiTableRenderer } from './plugins/kyomiTableRenderer.jsx';
import apiClient from '../../api/apiClient.js';

// Set up Kyomi's datasource resolver on the global registry
// This enables slug-based datasource references across all ChartML instances:
//   data:
//     datasource: "production-postgres"  # User-friendly slug
//     query: SELECT * FROM users
globalRegistry.setDatasourceResolver(async (slug, context) => {
  // Call backend to resolve slug to datasource config
  // The backend's resolve_datasource() handles:
  // - Slug lookup (e.g., "production-postgres")
  // - UUID lookup (e.g., "ds-abc123") for backwards compatibility
  const response = await apiClient.get(`/api/v1/datasources/${slug}`);
  const datasource = response.data;

  return {
    provider: datasource.datasource_type,   // e.g., "postgres", "bigquery"
    _resolved_slug: datasource.slug,        // Slug for API calls (plugins use this)
    connection_config: datasource.connection_config,
    auto_refresh_allowed: datasource.auto_refresh_allowed  // Whether dashboard auto-refresh is allowed
  };
});

/**
 * Create a Kyomi-configured ChartML instance
 *
 * @param {Object} options - Configuration options
 * @param {Object} [options.capabilities] - User/workspace capabilities (e.g., arrow_streaming)
 * @param {Array} [options.defaultPalette] - Default color array for charts (from workspace settings)
 * @param {Object} [options.registry] - Optional component registry (for sources, styles, configs)
 * @returns {ChartML} Configured ChartML instance
 *
 * @example
 * const capabilities = { arrow_streaming: true };
 * const chartml = createKyomiChartML({
 *   capabilities,
 *   defaultPalette: ['#ff0000', '#00ff00', '#0000ff']  // Custom colors from workspace config
 * });
 *
 * await chartml.render(spec, container);
 */

/**
 * Create Kyomi-specific loading indicator with animated logo
 * @returns {HTMLElement} Loading indicator element
 */
function createKyomiLoadingIndicator() {
  const loader = document.createElement('div');
  loader.className = 'absolute inset-0 flex items-center justify-center bg-card/50 backdrop-blur-sm z-10';
  loader.innerHTML = '<img src="/kyomi_animated_logo.svg" alt="Loading chart" class="w-8 h-8" />';
  return loader;
}

export function createKyomiChartML(options = {}) {
  const { capabilities = {}, defaultPalette, registry } = options;

  // Create base ChartML instance with Kyomi-specific customizations
  const chartml = new ChartML({
    registry,
    defaultPalette,  // Array of color strings from workspace config
    loadingIndicator: createKyomiLoadingIndicator  // Kyomi animated logo for loading state
  });

  // Note: Datasource resolver is set on globalRegistry (at module level above)
  // This makes slug resolution work for all ChartML instances

  // Register BigQuery data source plugin
  // Supports both JSON (all tiers) and Arrow (Team+ tier)
  // BigQuery is special - it uses OAuth, not backend proxy
  chartml.registerDataSource('bigquery', async (spec, context) => {
    // Pass capabilities through context for tier-based features
    // Errors will bubble up through ChartML's error handling system
    return await bigQueryDataSource(spec, { ...context, capabilities });
  });

  // Other datasources (postgres, clickhouse, mysql, snowflake, databricks, redshift)
  // are auto-registered to globalRegistry by genericProxyDataSource.js
  // ChartML falls back to globalRegistry when not found on instance

  // Replace default d3Transform middleware with DuckDB middleware
  // setTransformMiddleware replaces the default instead of adding to it
  chartml.setTransformMiddleware(async (data, spec, context) => {
    // Pass capabilities through context for tier-based features
    // Errors will bubble up through ChartML's error handling system
    return await duckDbMiddleware(data, spec, { ...context, capabilities });
  });

  // Register metric card chart renderer
  chartml.registerChartRenderer('metric', createMetricRenderer());

  // Register table chart renderer (with sorting, pagination, resizing)
  chartml.registerChartRenderer('table', createKyomiTableRenderer());

  // Register pie and doughnut chart renderers
  chartml.registerChartRenderer('pie', createPieChartRenderer());
  chartml.registerChartRenderer('doughnut', createPieChartRenderer());

  // Register scatter plot chart renderer
  chartml.registerChartRenderer('scatter', createScatterPlotRenderer());

  return chartml;
}

/**
 * Create a Kyomi ChartML instance with context from React hooks
 *
 * This is a convenience helper for React components that need to create
 * a ChartML instance with capabilities from useCapabilities hook.
 *
 * @param {Object} context - React context values
 * @param {Object} context.capabilities - Capabilities from useCapabilities hook
 * @param {Object} [context.registry] - Optional registry
 * @param {Object} [context.palettes] - Optional palettes
 * @returns {ChartML} Configured ChartML instance
 *
 * @example
 * const { capabilities } = useCapabilities();
 * const chartml = useMemo(() =>
 *   createKyomiChartMLWithContext({ capabilities }),
 *   [capabilities]
 * );
 */
export function createKyomiChartMLWithContext(context) {
  return createKyomiChartML(context);
}

export default createKyomiChartML;
