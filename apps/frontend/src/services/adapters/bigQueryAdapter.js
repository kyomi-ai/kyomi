// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * BigQuery Adapter for QueryService
 *
 * Routes BigQuery queries through the direct REST API (OAuth-based).
 * Supports random page access via BigQuery jobId.
 *
 * This adapter:
 * - Uses BigQueryDirectService for direct API calls (no backend proxy)
 * - Supports true pagination with jobId (jump to any page)
 * - Provides dry run support for query validation
 */

import { bigQueryService } from '../bigQueryService.js';

/**
 * Format bytes to human-readable string with appropriate unit.
 * @param {number} bytes - Number of bytes
 * @returns {string} Formatted string (e.g., "2.93 MB", "1.5 GB")
 */
function formatBytes(bytes) {
  if (bytes === 0) return '0 B';

  const units = ['B', 'KB', 'MB', 'GB', 'TB', 'PB'];
  const k = 1024;
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  const value = bytes / Math.pow(k, i);

  // Use 2 decimal places, but trim trailing zeros
  return `${value.toFixed(2).replace(/\.?0+$/, '')} ${units[i]}`;
}

/**
 * Adapter for BigQuery datasources.
 * Uses direct REST API with OAuth for performance.
 * Supports random page access via jobId.
 */
export const bigQueryAdapter = {
  /**
   * Execute a SQL query and return the first page of results.
   *
   * @param {string} sql - SQL query to execute
   * @param {Object} datasource - {slug, type}
   * @param {Object} options - {pageSize: number}
   * @returns {Promise<QueryResult>}
   */
  async executeQuery(sql, datasource, options = {}) {
    const pageSize = options.pageSize || 50;

    // Use existing bigQueryService with datasource slug for auth_mode-aware token
    const result = await bigQueryService.executeQuery(sql, datasource.slug, 1, pageSize);

    // Map columns to unified format {name, type}
    const columns = (result.columns || []).map((col) => ({
      name: col.name,
      type: mapBigQueryType(col.type),
    }));

    return {
      columns,
      rows: result.rows || [],
      totalRows: result.total_rows,
      pageSize,
      hasMore: (result.rows?.length || 0) === pageSize,
      queryHandle: {
        jobId: result.jobId,
        datasourceType: 'bigquery',
        datasourceSlug: datasource.slug,
        sql,
      },
      executionTimeMs: result.execution_time,
      bytesProcessed: result.bytes_processed,
    };
  },

  /**
   * Fetch a specific page of results.
   *
   * @param {QueryHandle} queryHandle - From previous executeQuery result
   * @param {number} page - 1-indexed page number
   * @param {number} pageSize - Rows per page
   * @returns {Promise<QueryResult>}
   */
  async fetchPage(queryHandle, page, pageSize) {
    // BigQuery supports random page access via jobId using startIndex
    const result = await bigQueryService.executeQuery(
      null, // No SQL needed - uses cached job
      queryHandle.datasourceSlug, // Pass datasource slug for auth_mode-aware token
      page,
      pageSize,
      queryHandle.jobId
    );

    return {
      columns: null, // Columns already known from first page
      rows: result.rows || [],
      totalRows: null, // Preserve from original query
      pageSize,
      hasMore: (result.rows?.length || 0) === pageSize,
      queryHandle, // Preserve handle
      executionTimeMs: result.execution_time,
      bytesProcessed: result.bytes_processed,
    };
  },

  /**
   * Validate query without executing (dry run).
   *
   * @param {string} sql - SQL query to validate
   * @param {Object} datasource - {slug, type}
   * @returns {Promise<DryRunResult>}
   */
  async dryRun(sql, datasource) {
    try {
      const result = await bigQueryService.dryRunQuery(sql, datasource.slug);

      // Format message with appropriate size unit
      const bytesProcessed = result.bytes_processed || 0;
      const sizeStr = formatBytes(bytesProcessed);
      const estimatedCostUSD = (bytesProcessed / (1024 ** 4)) * 5.0;
      const message = `Will scan: ${sizeStr} • Est. cost: $${estimatedCostUSD.toFixed(4)}`;

      return {
        valid: result.status === 'success',
        message,
        bytesProcessed: result.bytes_processed,
        estimatedCostUSD: result.estimated_cost_usd,
      };
    } catch (error) {
      // Parse BigQuery error for line/column: "at [line:column]"
      const errorMessage = error.message || 'Query validation failed';
      const match = errorMessage.match(/at\s+\[(\d+):(\d+)\]/);
      const line = match ? parseInt(match[1], 10) : null;
      const column = match ? parseInt(match[2], 10) : null;

      return {
        valid: false,
        message: errorMessage,
        line,
        column,
        bytesProcessed: null,
        estimatedCostUSD: null,
      };
    }
  },
};

/**
 * Map BigQuery types to simplified type names.
 * Used for consistent column type reporting across all adapters.
 *
 * @param {string} bqType - BigQuery type (e.g., 'STRING', 'INT64')
 * @returns {string} Simplified type ('string', 'number', 'boolean', 'datetime')
 */
function mapBigQueryType(bqType) {
  if (!bqType) return 'string';

  const typeUpper = bqType.toUpperCase();

  switch (typeUpper) {
    case 'INTEGER':
    case 'INT64':
    case 'FLOAT':
    case 'FLOAT64':
    case 'NUMERIC':
    case 'BIGNUMERIC':
      return 'number';

    case 'BOOLEAN':
    case 'BOOL':
      return 'boolean';

    case 'TIMESTAMP':
    case 'DATETIME':
    case 'DATE':
    case 'TIME':
      return 'datetime';

    case 'STRING':
    case 'BYTES':
    case 'GEOGRAPHY':
    default:
      return 'string';
  }
}
