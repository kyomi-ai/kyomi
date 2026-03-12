// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Kyomi Proprietary ChartML Plugins
 *
 * These plugins leverage Kyomi's advanced infrastructure and are NOT open sourced.
 * They validate that the ChartML plugin API supports sophisticated features.
 *
 * NOTE: Direct plugin imports happen in createKyomiChartML.js:
 * - bigQueryDataSource.js: Direct OAuth calls to Google BigQuery API
 * - genericProxyDataSource.js: Backend proxy for all other datasources
 */

// BigQuery: Direct OAuth API calls (frontend → Google)
export { bigQueryDataSource } from './bigQueryDataSource.js';

// Backend-proxied datasources (frontend → Kyomi backend → database)
// These are auto-registered to globalRegistry when genericProxyDataSource.js is imported
export {
  postgresDataSource,
  mysqlDataSource,
  clickHouseDataSource,
  snowflakeDataSource,
  databricksDataSource,
  redshiftDataSource,
  sqlserverDataSource,
  synapseDataSource,
} from './genericProxyDataSource.js';

// Middleware and renderers
export { duckDbMiddleware } from './duckDbMiddleware.js';
export { createKyomiTableRenderer } from './kyomiTableRenderer.jsx';
