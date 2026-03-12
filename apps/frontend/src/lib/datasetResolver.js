// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Dataset Resolver - Resolve dataset references in charts to SQL and TTL
 *
 * Handles two types of datasets:
 * 1. Named datasets: Defined in dashboard.datasets, referenced by name
 * 2. Implicit datasets: Embedded SQL in chart config
 *
 * Note: Cache keys are NOT generated here - the SharedWorker handles all caching internally.
 * Components only need SQL and TTL to execute queries.
 */

/**
 * Dataset resolution error
 */
export class DatasetResolutionError extends Error {
  constructor(message) {
    super(message);
    this.name = 'DatasetResolutionError';
  }
}

/**
 * Resolve a chart's dataset configuration to SQL and TTL
 *
 * @param {Object} chart - Chart configuration
 * @param {string} chart.dataset - Named dataset reference (optional)
 * @param {string} chart.sql - Embedded SQL (optional)
 * @param {Object} dashboardConfig - Dashboard configuration
 * @param {string} dashboardConfig.id - Dashboard ID
 * @param {Object} dashboardConfig.datasets - Named datasets definition
 * @returns {Promise<Object>} Resolved dataset: {sql, ttl, datasetType}
 * @throws {DatasetResolutionError} If dataset reference is invalid
 */
export async function resolveChartDataset(chart, dashboardConfig) {
  // Validate input
  if (!chart) {
    throw new DatasetResolutionError('Chart configuration is required');
  }

  // Case 1: Named dataset reference
  if (chart.dataset) {
    return await resolveNamedDataset(chart.dataset, dashboardConfig);
  }

  // Case 2: Embedded SQL (implicit dataset)
  if (chart.sql) {
    return await resolveImplicitDataset(chart.sql);
  }

  // Case 3: No dataset or SQL specified
  throw new DatasetResolutionError(
    'Chart must specify either "dataset" (named dataset reference) or "sql" (embedded SQL)'
  );
}

/**
 * Resolve a named dataset reference
 *
 * @param {string} datasetName - Name of dataset to resolve
 * @param {Object} dashboardConfig - Dashboard configuration
 * @returns {Promise<Object>} Resolved dataset
 * @throws {DatasetResolutionError} If dataset not found or invalid
 */
async function resolveNamedDataset(datasetName, dashboardConfig) {
  // Validate dashboard config
  if (!dashboardConfig) {
    throw new DatasetResolutionError('Dashboard configuration is required for named datasets');
  }

  if (!dashboardConfig.id) {
    throw new DatasetResolutionError('Dashboard ID is required for named datasets');
  }

  // Check if datasets are defined
  if (!dashboardConfig.datasets || typeof dashboardConfig.datasets !== 'object') {
    throw new DatasetResolutionError(
      `Dataset "${datasetName}" not found - dashboard has no datasets defined`
    );
  }

  // Find the dataset definition
  const datasetDef = dashboardConfig.datasets[datasetName];
  if (!datasetDef) {
    const availableDatasets = Object.keys(dashboardConfig.datasets).join(', ');
    throw new DatasetResolutionError(
      `Dataset "${datasetName}" not found in dashboard. Available datasets: ${availableDatasets || 'none'}`
    );
  }

  // Validate dataset definition
  if (!datasetDef.sql) {
    throw new DatasetResolutionError(
      `Dataset "${datasetName}" is missing required field: sql`
    );
  }

  // Get TTL (default: 1 hour)
  const ttl = datasetDef.ttl || 1;

  return {
    sql: datasetDef.sql,
    ttl,
    datasetType: 'named',
    datasetName
  };
}

/**
 * Resolve an implicit dataset (embedded SQL in chart)
 *
 * @param {string} sql - SQL query from chart config
 * @returns {Promise<Object>} Resolved dataset
 */
async function resolveImplicitDataset(sql) {
  if (!sql || typeof sql !== 'string') {
    throw new DatasetResolutionError('SQL query must be a non-empty string');
  }

  // Default TTL: 1 hour
  const ttl = 1;

  return {
    sql,
    ttl,
    datasetType: 'implicit'
  };
}

/**
 * Resolve dataset for chat bubble chart (always implicit)
 *
 * @param {string} sql - SQL query from chart-ml
 * @returns {Promise<Object>} Resolved dataset
 */
export async function resolveChatDataset(sql) {
  // Chat bubbles always use implicit datasets
  return await resolveImplicitDataset(sql);
}

/**
 * Generate a hash of SQL for progress tracking
 * Components use this to identify which extract events belong to their query
 *
 * @param {string} sql - SQL query text
 * @returns {Promise<string>} Hash identifier
 */
export async function hashSQL(sql) {
  if (typeof crypto === 'undefined' || !crypto.subtle) {
    // Fallback for non-secure contexts (HTTP over LAN IP)
    let h1 = 0xdeadbeef, h2 = 0x41c6ce57;
    for (let i = 0; i < sql.length; i++) {
      const ch = sql.charCodeAt(i);
      h1 = Math.imul(h1 ^ ch, 2654435761);
      h2 = Math.imul(h2 ^ ch, 1597334677);
    }
    h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507);
    h1 ^= Math.imul(h2 ^ (h2 >>> 13), 3266489909);
    h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507);
    h2 ^= Math.imul(h1 ^ (h1 >>> 13), 3266489909);
    const combined = 4294967296 * (2097151 & h2) + (h1 >>> 0);
    return combined.toString(16).padStart(16, '0');
  }
  const encoder = new TextEncoder();
  const data = encoder.encode(sql);
  const hashBuffer = await crypto.subtle.digest('SHA-256', data);
  const hashArray = Array.from(new Uint8Array(hashBuffer));
  return hashArray.map(b => b.toString(16).padStart(2, '0')).join('');
}

/**
 * Validate dataset definition
 *
 * @param {string} datasetName - Dataset name
 * @param {Object} datasetDef - Dataset definition
 * @throws {DatasetResolutionError} If dataset definition is invalid
 */
export function validateDatasetDefinition(datasetName, datasetDef) {
  if (!datasetDef || typeof datasetDef !== 'object') {
    throw new DatasetResolutionError(
      `Dataset "${datasetName}" must be an object`
    );
  }

  if (!datasetDef.sql) {
    throw new DatasetResolutionError(
      `Dataset "${datasetName}" is missing required field: sql`
    );
  }

  if (typeof datasetDef.sql !== 'string') {
    throw new DatasetResolutionError(
      `Dataset "${datasetName}": sql must be a string`
    );
  }

  if (datasetDef.ttl !== undefined && typeof datasetDef.ttl !== 'number') {
    throw new DatasetResolutionError(
      `Dataset "${datasetName}": ttl must be a number (hours)`
    );
  }
}

/**
 * Validate all datasets in a dashboard configuration
 *
 * @param {Object} dashboardConfig - Dashboard configuration
 * @returns {Object[]} Array of validation errors (empty if valid)
 */
export function validateDashboardDatasets(dashboardConfig) {
  const errors = [];

  if (!dashboardConfig.datasets) {
    return errors; // No datasets is valid
  }

  if (typeof dashboardConfig.datasets !== 'object') {
    errors.push({
      message: 'datasets must be an object',
      field: 'datasets'
    });
    return errors;
  }

  // Validate each dataset
  for (const [datasetName, datasetDef] of Object.entries(dashboardConfig.datasets)) {
    try {
      validateDatasetDefinition(datasetName, datasetDef);
    } catch (error) {
      errors.push({
        message: error.message,
        dataset: datasetName,
        field: 'datasets'
      });
    }
  }

  return errors;
}

export default {
  resolveChartDataset,
  resolveChatDataset,
  hashSQL,
  validateDatasetDefinition,
  validateDashboardDatasets,
  DatasetResolutionError
};
