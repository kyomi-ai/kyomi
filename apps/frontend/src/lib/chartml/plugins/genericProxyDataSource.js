// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Generic Proxy Data Source Factory for ChartML
 *
 * Creates data source handlers for backend-proxied datasources.
 * All non-BigQuery datasources use the same backend endpoint and response format.
 *
 * This factory eliminates duplication - adding a new datasource type requires
 * only adding its name to PROXY_DATASOURCES and optional error enhancements.
 *
 * BigQuery is NOT included here - it uses direct OAuth calls via bigQueryDataSource.js
 *
 * ## Two-Tier Error Enhancement System
 *
 * Error messages are enhanced using a two-tier priority system:
 *
 * ### Priority 1: COMMON_ERROR_PATTERNS (Datasource Accessibility)
 * These patterns indicate fundamental datasource access issues that apply to ALL
 * datasource types. They are checked FIRST because:
 * - They represent configuration/credential issues, not query problems
 * - They have consistent user-actionable solutions
 * - Backend already formats these messages well (null = pass through)
 *
 * ### Priority 2: ERROR_ENHANCEMENTS (Provider-Specific)
 * If no common pattern matches, provider-specific patterns are checked.
 * These handle database-specific errors like connection failures or auth issues
 * that vary by provider (PostgreSQL, ClickHouse, etc.).
 *
 * This ordering ensures users always see the most actionable message first.
 */

import apiClient from '../../../api/apiClient.js';
import { globalRegistry } from '@chartml/core';
import { parseDateColumns } from '../../../utils/dateParser.js';

/**
 * Common error patterns for datasource accessibility.
 * These apply to ALL datasource types and are checked FIRST (highest priority).
 *
 * Backend HTTP 403 messages (from execute_query endpoint):
 * - "Datasource '{id}' is disabled. Go to Settings to enable it."
 * - "Datasource '{id}' requires credentials. Go to Settings to configure your credentials."
 * - "Your credentials for datasource '{id}' have expired. Go to Settings to reconnect."
 *
 * Resolver error messages (from DatasourceNotAccessibleError):
 * - "Datasource '{id}' is not accessible. Reason: disabled"
 * - "Datasource '{id}' is not accessible. Reason: no_credentials"
 * - "Datasource '{id}' is not accessible. Reason: expired_credentials"
 *
 * @see ERROR_ENHANCEMENTS for provider-specific patterns (checked second)
 */
const COMMON_ERROR_PATTERNS = {
  // Match HTTP 403 messages from execute_query endpoint (these are user-friendly, pass through)
  'is disabled. Go to Settings': null,  // null = pass through the original message
  'requires credentials. Go to Settings': null,
  'have expired. Go to Settings': null,
  // Match resolver's Reason: patterns (convert to user-friendly)
  'Reason: disabled': 'This datasource is disabled. Go to Settings → Datasources to enable it.',
  'Reason: no_credentials': 'This datasource requires credentials. Go to Settings → Datasources to configure your credentials.',
  'Reason: expired_credentials': 'Your credentials have expired. Go to Settings → Datasources to reconnect.',
  // Datasource-level "not found" - must NOT match "Table not found" or "Column not found"
  // The backend uses "Datasource" prefix for datasource-level errors
  'Datasource not found': 'This datasource was not found. It may have been removed or renamed.',
  'datasource not found': 'This datasource was not found. It may have been removed or renamed.',
  // Generic accessibility pattern (careful: too broad patterns can match SQL errors)
  'not accessible': 'This datasource is not accessible. Go to Settings → Datasources to check your configuration.',
  // HTTP status-based patterns
  'Datasource is inactive': 'This datasource is inactive. Contact your workspace administrator.',
};

/**
 * Provider-specific error message enhancements (Priority 2).
 * These are checked ONLY if no COMMON_ERROR_PATTERNS match.
 *
 * Keys are substrings to match in error messages.
 * Values are user-friendly replacement messages.
 *
 * @see COMMON_ERROR_PATTERNS for datasource accessibility patterns (checked first)
 */
const ERROR_ENHANCEMENTS = {
  postgres: {
    'credentials not configured': 'PostgreSQL Error: Credentials not configured. Please go to Settings → Datasources.',
    'Connection refused': 'PostgreSQL Error: Connection refused. Check host and port settings.',
    'connection refused': 'PostgreSQL Error: Connection refused. Please verify the host, port, and firewall settings.',
    'password authentication failed': 'PostgreSQL Error: Authentication failed. Check username and password.',
    'authentication failed': 'PostgreSQL Error: Authentication failed. Please check your username and password.',
    'SSH tunnel': 'PostgreSQL Error: SSH tunnel connection failed. Please verify your SSH configuration and ensure the public key is added to your bastion host.',
    'not installed': 'PostgreSQL Error: Server connector not available. Please contact your administrator.',
  },
  clickhouse: {
    'credentials not configured': 'ClickHouse Error: Credentials not configured. Please go to Settings → Datasources.',
    'Connection refused': 'ClickHouse Error: Connection refused. Check host and port settings.',
    'not installed': 'ClickHouse Error: Server connector not available. Please contact your administrator.',
  },
  mysql: {
    'credentials not configured': 'MySQL Error: Credentials not configured. Please go to Settings → Datasources.',
    'Access denied': 'MySQL Error: Authentication failed. Check username and password.',
    'Unknown database': 'MySQL Error: Database not found. Check database name.',
    'not installed': 'MySQL Error: Server connector not available. Please contact your administrator.',
  },
  snowflake: {
    'credentials not configured': 'Snowflake Error: Credentials not configured. Please go to Settings → Datasources.',
    'not installed': 'Snowflake Error: Server connector not available. Please contact your administrator.',
    'account': 'Snowflake Error: Invalid account identifier. Please check your datasource configuration.',
  },
  databricks: {
    'credentials not configured': 'Databricks Error: Credentials not configured. Please go to Settings → Datasources to add your Databricks access token.',
    'not installed': 'Databricks Error: Server connector not available. Please contact your administrator.',
    'warehouse': 'Databricks Error: SQL Warehouse not accessible. Please check your datasource configuration.',
    'http_path': 'Databricks Error: SQL Warehouse not accessible. Please check your datasource configuration.',
  },
  redshift: {
    'credentials not configured': 'Redshift Error: Credentials not configured. Please go to Settings → Datasources.',
    'not installed': 'Redshift Error: Server connector not available. Please contact your administrator.',
    'cluster': 'Redshift Error: Cluster not accessible. Please check your datasource configuration and network settings.',
    'endpoint': 'Redshift Error: Cluster not accessible. Please check your datasource configuration and network settings.',
  },
};

/**
 * Create a proxy data source handler for any backend-proxied datasource.
 *
 * @param {string} datasourceType - The datasource type (e.g., 'postgres', 'clickhouse')
 * @returns {Function} Data source handler function for ChartML
 */
export function createProxyDataSource(datasourceType) {
  return async function proxyDataSource(spec, context = {}) {
    const { query, datasource, datasource_id, _resolved_slug } = spec;
    const datasourceIdentifier = datasource || _resolved_slug || datasource_id;

    if (!query) {
      throw new Error(`${datasourceType} data source requires a "query" field`);
    }

    if (!datasourceIdentifier) {
      throw new Error(`${datasourceType} data source requires a "datasource" field`);
    }

    try {
      const response = await apiClient.post('/api/v1/datasources/query/execute', {
        sql: query,
        datasource: datasourceIdentifier,
        limit: 10000,
        dry_run: false
      });

      const result = response.data;

      if (result.status === 'error') {
        throw new Error(result.error || `${datasourceType} query failed`);
      }

      const { columns: columnsMeta, rows: rowData } = result;

      if (!columnsMeta || !rowData || columnsMeta.length === 0) {
        return {
          data: [],
          metadata: {
            format: 'json',
            columns: [],
            rowCount: 0,
            datasource: datasourceIdentifier,
            datasource_type: datasourceType
          }
        };
      }

      const columnNames = columnsMeta.map(col =>
        typeof col === 'string' ? col : col.name
      );

      // Parse datetime columns from ISO strings to Date objects
      const parsedRows = parseDateColumns(columnsMeta, rowData);

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
          bytesProcessed: result.bytes_processed,
          datasource: datasourceIdentifier,
          datasource_type: datasourceType
        }
      };

    } catch (error) {

      let errorMessage = error.response?.data?.detail || error.message || 'Query failed';

      // Check common datasource accessibility patterns first (higher priority)
      // These apply to ALL datasource types
      let enhancedMessage = null;
      for (const [pattern, replacement] of Object.entries(COMMON_ERROR_PATTERNS)) {
        if (errorMessage.includes(pattern)) {
          if (replacement === null) {
            // null = pass through the original message (it's already user-friendly from backend)
            enhancedMessage = errorMessage;
          } else {
            enhancedMessage = replacement;
          }
          break;
        }
      }

      // If no common pattern matched, try provider-specific enhancements
      if (!enhancedMessage) {
        const enhancements = ERROR_ENHANCEMENTS[datasourceType] || {};
        for (const [pattern, replacement] of Object.entries(enhancements)) {
          if (errorMessage.includes(pattern)) {
            enhancedMessage = replacement;
            break;
          }
        }
      }

      throw new Error(enhancedMessage || errorMessage);
    }
  };
}

/**
 * Datasource types that use the backend proxy.
 * BigQuery is NOT included - it uses direct OAuth calls.
 */
const PROXY_DATASOURCES = [
  'postgres',
  'mysql',
  'clickhouse',
  'snowflake',
  'databricks',
  'redshift',
  'sqlserver',
  'synapse',
];

// Auto-register all proxy datasources to global registry
for (const type of PROXY_DATASOURCES) {
  globalRegistry.registerDataSource(type, createProxyDataSource(type));
}

// Named exports for backwards compatibility and direct imports
export const postgresDataSource = createProxyDataSource('postgres');
export const mysqlDataSource = createProxyDataSource('mysql');
export const clickHouseDataSource = createProxyDataSource('clickhouse');
export const snowflakeDataSource = createProxyDataSource('snowflake');
export const databricksDataSource = createProxyDataSource('databricks');
export const redshiftDataSource = createProxyDataSource('redshift');
export const sqlserverDataSource = createProxyDataSource('sqlserver');
export const synapseDataSource = createProxyDataSource('synapse');
