// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * BigQuery Data Source Plugin for ChartML
 *
 * Provides BigQuery integration as a ChartML data source plugin.
 * Supports both JSON (Free/Basic) and Arrow (Team+) formats.
 *
 * Returns data in Result Object format: {data, metadata}
 * NO DuckDB operations - just fetches raw data.
 *
 * Usage in ChartML spec (preferred - using slug):
 * ```yaml
 * data:
 *   datasource: "production-bigquery"
 *   query: "SELECT * FROM sales WHERE region = '$params.region'"
 * ```
 *
 * Legacy usage (still supported):
 * ```yaml
 * data:
 *   provider: bigquery
 *   query: "SELECT * FROM sales WHERE region = '$params.region'"
 *   datasource_id: "ds-abc123"
 * ```
 *
 * Multi-Datasource Support:
 * - Uses datasource slug (e.g., "production-bigquery") to reference configured datasources
 * - BigQuery queries use Direct API (user's OAuth) for optimal performance
 * - Non-BigQuery datasources use unified backend endpoint
 */

import bigQueryDirectService from '../../../services/BigQueryDirectService.js';
import { globalRegistry } from '@chartml/core';

/**
 * Common error patterns for datasource accessibility.
 * These apply to datasource access/credential issues.
 *
 * Note: 'not found' is intentionally specific ("Datasource not found") to avoid
 * false positives for "Table not found" or "Column not found" SQL errors.
 */
const DATASOURCE_ERROR_PATTERNS = {
  // Backend returns these patterns for DatasourceNotAccessibleError
  'is disabled': 'This datasource is disabled. Go to Settings → Datasources to enable it.',
  'requires credentials': 'This datasource requires credentials. Go to Settings → Datasources to configure your credentials.',
  'have expired': 'Your credentials have expired. Go to Settings → Datasources to reconnect.',
  // Datasource-level "not found" - must NOT match "Table not found" or "Column not found"
  'Datasource not found': 'This datasource was not found. It may have been removed or renamed.',
  'datasource not found': 'This datasource was not found. It may have been removed or renamed.',
  'not accessible': 'This datasource is not accessible. Go to Settings → Datasources to check your configuration.',
  'Datasource is inactive': 'This datasource is inactive. Contact your workspace administrator.',
  // OAuth-specific patterns
  'OAuth token expired': 'Your Google account connection has expired. Go to Settings → Profile to reconnect your Google account.',
  'reconnect your Google account': 'Your Google account connection has expired. Go to Settings → Profile to reconnect your Google account.',
};

/**
 * BigQuery data source handler for ChartML
 *
 * OPTIMIZATION: Always submits query first, then chooses optimal retrieval method:
 * - For small datasets (≤10K rows): Use direct API (faster, single page fetch)
 * - For large datasets (>10K rows) with Arrow enabled: Use Arrow streaming (efficient binary format)
 * - Otherwise: Use direct API pagination
 *
 * This prevents duplicate query execution and minimizes server load.
 *
 * @param {Object} spec - Data source specification
 * @param {string} spec.query - SQL query to execute
 * @param {string} [spec.datasource] - Datasource slug (preferred, e.g., "production-bigquery")
 * @param {string} [spec.datasource_id] - Datasource ID (legacy, for backwards compatibility)
 * @param {Object} context - ChartML context
 * @param {Object} context.capabilities - User capabilities
 * @returns {Promise<Object>} Result Object with {data, metadata}
 */
export async function bigQueryDataSource(spec, context = {}) {
  // Support datasource (slug), _resolved_slug (from ChartML resolver), and datasource_id (legacy)
  const { query, datasource, datasource_id, _resolved_slug } = spec;

  // Use slug if available, fallback to datasource_id
  const datasourceIdentifier = datasource || _resolved_slug || datasource_id;

  const capabilities = context.capabilities || {};

  if (!query) {
    throw new Error('BigQuery data source requires a "query" field');
  }

  const DIRECT_API_THRESHOLD = 10000; // Rows threshold for switching to Arrow
  const arrowEnabled = capabilities.bigquery_mode === 'backend_proxy';

  // STEP 1: Submit query and get job metadata (including row count)
  // This doesn't fetch data yet, just executes the query and gets metadata
  let firstPage;
  try {
    firstPage = await bigQueryDirectService.executeQuery(query, datasourceIdentifier, {
      maxResults: 100 // Fetch first page to get totalRows (using 100 instead of 1 due to BigQuery bug with maxResults=1 and complex CTEs)
    });
  } catch (error) {

    // Parse error message to provide helpful context
    let errorMessage = error.message || 'Query failed';
    let enhancedMessage = null;

    // Check for datasource accessibility errors first (highest priority)
    // Backend already includes datasource name in error messages, so just use the replacement
    for (const [pattern, replacement] of Object.entries(DATASOURCE_ERROR_PATTERNS)) {
      if (errorMessage.includes(pattern)) {
        enhancedMessage = replacement;
        break;
      }
    }

    // If no datasource accessibility error, check for BigQuery permission errors
    if (!enhancedMessage) {
      if (errorMessage.includes('bigquery.jobs.create')) {
        enhancedMessage = 'BigQuery Error: You don\'t have permission to run queries on this project. Please go to Settings → Profile to configure a billing project with the required permissions.';
      } else if (errorMessage.includes('Access Denied') || errorMessage.includes('403') || errorMessage.includes('PERMISSION_DENIED')) {
        enhancedMessage = 'BigQuery Error: Access denied. Please check your BigQuery project permissions in Settings → Profile.';
      } else if (errorMessage.includes('billing')) {
        enhancedMessage = 'BigQuery Error: Billing project is not configured or you don\'t have access. Please go to Settings → Profile to configure your BigQuery projects.';
      }
    }

    throw new Error(enhancedMessage || errorMessage);
  }

  const { jobId, totalRows, columns } = firstPage;

  // STEP 2: Choose optimal retrieval method based on row count

  if (totalRows > DIRECT_API_THRESHOLD && arrowEnabled) {
    // LARGE DATASET + ARROW ENABLED: Use Arrow streaming with jobId

    // Import apiClient dynamically to use authenticated requests with token refresh
    const { default: apiClient } = await import('../../../api/apiClient.js');

    const response = await apiClient.post('/api/v1/bigquery/read-arrow',
      { job_id: jobId }, // Pass jobId, not SQL - no re-execution!
      { responseType: 'arraybuffer' } // We want raw binary Arrow data
    );

    // apiClient returns response.data for successful requests
    const arrowBuffer = response.data;

    return {
      data: arrowBuffer,
      metadata: {
        format: 'arrow',
        query: query,
        totalRows,
        datasource: datasourceIdentifier
      }
    };
  }

  // SMALL DATASET OR ARROW DISABLED: Use direct API

  // Fetch remaining pages using the existing jobId (no re-execution!)
  const result = await bigQueryDirectService.fetchJobResults(
    jobId,
    datasourceIdentifier,
    firstPage.rows,
    columns,
    firstPage.pageToken,
    {
      maxResults: 10000
    }
  );

  // Convert from BigQuery format to ChartML format (Array<Object>)
  if (!result.rows || !result.columns) {
    return {
      data: [],
      metadata: {
        format: 'json',
        columns: [],
        rowCount: 0
      }
    };
  }

  const data = result.rows.map(rowArray => {
    const obj = {};
    result.columns.forEach((col, idx) => {
      obj[col.name] = rowArray[idx];
    });
    return obj;
  });

  return {
    data: data,
    metadata: {
      format: 'json',
      columns: result.columns.map(c => c.name),
      rowCount: data.length,
      datasource: datasourceIdentifier
    }
  };
}

// Auto-register to global registry when imported
globalRegistry.registerDataSource('bigquery', bigQueryDataSource);
