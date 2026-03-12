// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Backend Adapter for QueryService
 *
 * Routes non-BigQuery queries through the backend API.
 * Uses LIMIT/OFFSET pagination (re-executes query for each page).
 *
 * This adapter:
 * - Works with PostgreSQL, ClickHouse, Snowflake, Databricks, Redshift
 * - Uses the unified /datasources/query/execute endpoint
 * - Returns row-based format matching the QueryService interface
 */

import apiClient from '../../api/apiClient.js';
import { parseDateColumns } from '../../utils/dateParser.js';

/**
 * Adapter for non-BigQuery datasources.
 * Routes through backend with LIMIT/OFFSET pagination.
 */
export const backendAdapter = {
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

    const response = await apiClient.post('/api/v1/datasources/query/execute', {
      sql,
      datasource: datasource.slug,
      limit: pageSize,
      offset: 0,
      page_size: pageSize,
      include_total: true,
    });

    const result = response.data;

    if (result.status === 'error') {
      throw new Error(result.error || 'Query execution failed');
    }

    // Parse datetime columns from ISO strings to Date objects
    const parsedRows = parseDateColumns(result.columns || [], result.rows || []);

    return {
      columns: result.columns || [],
      rows: parsedRows,
      totalRows: result.total_rows,
      pageSize,
      hasMore: result.has_more,
      queryHandle: {
        datasourceType: datasource.type,
        datasourceSlug: datasource.slug,
        sql,
      },
      executionTimeMs: result.execution_time_ms,
      bytesProcessed: result.bytes_processed,
    };
  },

  /**
   * Fetch a specific page of results.
   *
   * Note: Non-BigQuery datasources use LIMIT/OFFSET pagination,
   * which means the query is re-executed for each page. This is
   * less efficient than BigQuery's jobId-based pagination but
   * is the standard approach for most databases.
   *
   * @param {QueryHandle} queryHandle - From previous executeQuery result
   * @param {number} page - 1-indexed page number
   * @param {number} pageSize - Rows per page
   * @returns {Promise<QueryResult>}
   */
  async fetchPage(queryHandle, page, pageSize) {
    const offset = (page - 1) * pageSize;

    const response = await apiClient.post('/api/v1/datasources/query/execute', {
      sql: queryHandle.sql,
      datasource: queryHandle.datasourceSlug,
      limit: pageSize,
      offset,
      page_size: pageSize,
      include_total: false, // Already have total from first query
    });

    const result = response.data;

    if (result.status === 'error') {
      throw new Error(result.error || 'Query execution failed');
    }

    // Parse datetime columns from ISO strings to Date objects
    const parsedRows = parseDateColumns(result.columns || [], result.rows || []);

    return {
      columns: result.columns || [],
      rows: parsedRows,
      totalRows: null, // Preserve from original query
      pageSize,
      hasMore: result.has_more,
      queryHandle,
      executionTimeMs: result.execution_time_ms,
      bytesProcessed: result.bytes_processed,
    };
  },

  /**
   * Validate query without executing (dry run).
   *
   * Uses the backend's dry_run mode which:
   * - BigQuery: Returns cost estimate message with line/column on errors
   * - Other databases: Uses EXPLAIN for syntax validation with line on errors
   *
   * @param {string} sql - SQL query to validate
   * @param {Object} datasource - {slug, type}
   * @returns {Promise<{valid: boolean, message: string, line?: number, column?: number}>}
   */
  async dryRun(sql, datasource) {
    try {
      const response = await apiClient.post('/api/v1/datasources/query/execute', {
        sql,
        datasource: datasource.slug,
        dry_run: true,
      });

      const result = response.data;

      // Backend must always return a message - no fallbacks
      if (!result.message) {
        return {
          valid: false,
          message: 'Backend returned no message - this is a bug',
        };
      }

      return {
        valid: result.status === 'success',
        message: result.message,
        line: result.line,      // Error line number (1-indexed, from backend)
        column: result.column,  // Error column number (from backend)
      };
    } catch (error) {
      // Handle API errors (network, 4xx, 5xx)
      // Check each source explicitly - no silent fallbacks
      const errorMessage =
        error.response?.data?.detail ||
        error.response?.data?.error ||
        error.message ||
        `Unknown error: ${String(error)}`;

      return {
        valid: false,
        message: errorMessage,
        // No line/column for network errors
      };
    }
  },

  /**
   * Start a streaming query. Returns request_id for WebSocket correlation.
   * Use useQueryStream hook to receive progressive results.
   *
   * @param {string} sql - SQL query to execute
   * @param {Object} datasource - {slug, type}
   * @param {Object} options - {limit, offset, includeTotal}
   * @returns {Promise<{requestId: string}>}
   */
  async startStream(sql, datasource, options = {}) {
    const response = await apiClient.post('/api/v1/datasources/query/stream', {
      sql,
      datasource: datasource.slug,
      limit: options.limit || 10000,
      offset: options.offset || 0,
      include_total: options.includeTotal !== false,
    });

    return {
      requestId: response.data.request_id,
    };
  },
};
