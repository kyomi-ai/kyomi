// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * BigQuery Service - Centralized BigQuery operations
 *
 * Provides a single source of truth for all BigQuery-related API calls.
 *
 * SQL Editor now uses Direct API (saves massive backend bandwidth costs)
 * Charts still use backend for Arrow streaming (fast path)
 */

import apiClient from '../api/apiClient.js';
// REMOVED: import { executeQuery as executeQueryCached } from '../lib/queryExecutor';
import bigQueryDirectService from './BigQueryDirectService.js';

export const bigQueryService = {
  /**
   * Execute SQL query using Direct API (no backend proxy)
   * Used by SQL Editor for interactive query execution
   *
   * Benefits:
   * - Saves backend compute and bandwidth costs
   * - Direct to user's BigQuery account (no proxy)
   * - Token caching reduces backend calls
   * - Supports jumping to any page using startIndex
   *
   * Two modes:
   * 1. Initial query: Provide 'sql' to execute query and get first page + jobId
   * 2. Pagination: Provide 'jobId' and 'page' to fetch any page (uses startIndex)
   *
   * @param {string|null} sql - The SQL query to execute (for initial query)
   * @param {string} datasourceSlug - Datasource slug (required)
   * @param {number} page - Page number (1-indexed, default: 1)
   * @param {number} pageSize - Rows per page (default: 50)
   * @param {string|null} jobId - BigQuery job ID for pagination
   * @returns {Promise<Object>} Query results: {columns: [...], rows: [...], totalRows: ..., jobId: ...}
   * @throws {Error} If query execution fails
   */
  executeQuery: async (sql = null, datasourceSlug, page = 1, pageSize = 50, jobId = null) => {
    if (!datasourceSlug) {
      throw new Error('datasourceSlug is required for BigQuery query execution');
    }

    // Mode 1: Initial query execution
    if (sql && !jobId) {
      const result = await bigQueryDirectService.executeQuery(sql, datasourceSlug, {
        maxResults: pageSize,
        timeoutMs: 60000
      });

      return {
        columns: result.columns, // Keep full column metadata {name, type, mode}
        rows: result.rows,
        total_rows: result.totalRows,
        jobId: result.jobId,
        pageToken: result.pageToken
      };
    }

    // Mode 2: Pagination (jump to any page using startIndex)
    if (jobId) {
      const result = await bigQueryDirectService.fetchPage(jobId, datasourceSlug, page, pageSize);

      return {
        columns: null, // Columns already known from first page
        rows: result.rows,
        pageToken: result.pageToken
      };
    }

    throw new Error('Must provide either sql (for initial query) or jobId (for pagination)');
  },

  /**
   * Dry run SQL query validation using Direct API
   * Validates SQL syntax and estimates cost/bytes scanned without executing
   *
   * Now calls BigQuery directly - no backend proxy needed!
   *
   * @param {string} sql - The SQL query to validate
   * @param {string} datasourceSlug - Datasource slug (required)
   * @returns {Promise<Object>} Validation result: {bytesProcessed, estimatedCostUSD, canExecute}
   * @throws {Error} If validation request fails
   */
  dryRunQuery: async (sql, datasourceSlug) => {
    if (!datasourceSlug) {
      throw new Error('datasourceSlug is required for BigQuery dry run');
    }

    const result = await bigQueryDirectService.dryRunQuery(sql, datasourceSlug);

    // Format to match expected structure
    return {
      status: 'success',
      bytes_processed: result.bytesProcessed,
      estimated_cost_usd: result.estimatedCostUSD,
      can_execute: result.canExecute
    };
  }
};
