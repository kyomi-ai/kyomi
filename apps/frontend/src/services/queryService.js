// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Query Service - Unified Query Execution Interface
 *
 * Provides a single API for executing SQL queries against any datasource type.
 * Routes to the appropriate adapter based on datasource type:
 * - BigQuery: Direct REST API with OAuth (supports random page access via jobId)
 * - Other: Backend proxy with LIMIT/OFFSET pagination
 *
 * Usage:
 *   import { queryService } from './services/queryService';
 *
 *   // Execute query
 *   const result = await queryService.executeQuery(sql, { slug: 'my-postgres', type: 'postgres' });
 *
 *   // Fetch next page
 *   const page2 = await queryService.fetchPage(result.queryHandle, 2, 50);
 *
 *   // Validate query
 *   const validation = await queryService.dryRun(sql, { slug: 'my-postgres', type: 'postgres' });
 *
 * @typedef {Object} QueryResult
 * @property {Array<{name: string, type: string}>} columns - Column metadata
 * @property {Array<Array<any>>} rows - Row data as arrays
 * @property {number|null} totalRows - Total result count (may be estimated)
 * @property {number} pageSize - Rows per page
 * @property {boolean} hasMore - More pages available
 * @property {QueryHandle} queryHandle - Opaque handle for pagination
 * @property {number|null} executionTimeMs - Query execution time
 * @property {number|null} bytesProcessed - Bytes scanned (BigQuery only)
 *
 * @typedef {Object} QueryHandle
 * @property {string} datasourceType - 'bigquery', 'postgres', etc.
 * @property {string} datasourceSlug - 'production-postgres'
 * @property {string} sql - Original query (for re-execution)
 * @property {string} [jobId] - BigQuery job ID (enables random page access)
 *
 * @typedef {Object} DryRunResult
 * @property {boolean} valid - Whether query is syntactically valid
 * @property {boolean} supported - Whether datasource supports dry run
 * @property {string|null} error - Error message if invalid
 * @property {number|null} bytesProcessed - Estimated bytes (BigQuery only)
 * @property {number|null} estimatedCostUSD - Estimated cost (BigQuery only)
 */

import { bigQueryAdapter } from './adapters/bigQueryAdapter.js';
import { backendAdapter } from './adapters/backendAdapter.js';

/**
 * Get the appropriate adapter for a datasource type.
 *
 * @param {string} datasourceType - 'bigquery', 'postgres', 'clickhouse', etc.
 * @returns {Object} Adapter with executeQuery, fetchPage, dryRun methods
 */
const getAdapter = (datasourceType) => {
  if (datasourceType === 'bigquery') {
    return bigQueryAdapter;
  }
  // All non-BigQuery datasources use the backend adapter
  return backendAdapter;
};

export const queryService = {
  /**
   * Execute a SQL query and return the first page of results.
   *
   * @param {string} sql - SQL query to execute
   * @param {Object} datasource - Datasource info
   * @param {string} datasource.slug - Datasource slug (e.g., 'production-postgres')
   * @param {string} datasource.type - Datasource type (e.g., 'postgres', 'bigquery')
   * @param {Object} [options] - Execution options
   * @param {number} [options.pageSize=50] - Rows per page
   * @returns {Promise<QueryResult>} Query results with pagination info
   */
  async executeQuery(sql, datasource, options = {}) {
    if (!datasource?.slug || !datasource?.type) {
      throw new Error('Datasource must have slug and type properties');
    }

    const adapter = getAdapter(datasource.type);
    const pageSize = options.pageSize || 50;

    const result = await adapter.executeQuery(sql, datasource, { pageSize });

    // Ensure queryHandle has all required fields for pagination
    result.queryHandle = {
      ...result.queryHandle,
      datasourceType: datasource.type,
      datasourceSlug: datasource.slug,
      sql,
    };

    return result;
  },

  /**
   * Fetch a specific page of results.
   *
   * For BigQuery: Uses jobId to jump to any page instantly.
   * For others: Re-executes query with LIMIT/OFFSET.
   *
   * @param {QueryHandle} queryHandle - Handle from previous executeQuery result
   * @param {number} page - 1-indexed page number
   * @param {number} pageSize - Rows per page
   * @returns {Promise<QueryResult>} Page results
   */
  async fetchPage(queryHandle, page, pageSize) {
    if (!queryHandle?.datasourceType) {
      throw new Error('Invalid queryHandle: missing datasourceType');
    }

    const adapter = getAdapter(queryHandle.datasourceType);
    return adapter.fetchPage(queryHandle, page, pageSize);
  },

  /**
   * Validate query without executing (dry run).
   *
   * For BigQuery: Returns cost estimate (bytes processed, USD cost).
   * For others: Uses EXPLAIN for syntax validation.
   *
   * @param {string} sql - SQL query to validate
   * @param {Object} datasource - Datasource info
   * @param {string} datasource.slug - Datasource slug
   * @param {string} datasource.type - Datasource type
   * @returns {Promise<DryRunResult>} Validation result
   */
  async dryRun(sql, datasource) {
    if (!datasource?.slug || !datasource?.type) {
      throw new Error('Datasource must have slug and type properties');
    }

    const adapter = getAdapter(datasource.type);
    return adapter.dryRun(sql, datasource);
  },
};
