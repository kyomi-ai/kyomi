// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * @chartml/data-bigquery
 *
 * BigQuery data source plugin for ChartML
 * Executes SQL queries against Google BigQuery and returns results
 *
 * @example
 * import { createBigQueryDataSource } from '@chartml/data-bigquery';
 *
 * const bigquerySource = createBigQueryDataSource({
 *   projectId: 'my-project',
 *   getAccessToken: () => getOAuthToken()
 * });
 *
 * chartml.registerDataSource('bigquery', bigquerySource);
 */

/**
 * Create a BigQuery data source plugin
 *
 * @param {Object} options - Configuration options
 * @param {string} options.projectId - Google Cloud project ID
 * @param {Function} options.getAccessToken - Async function that returns OAuth access token
 * @param {string} [options.location='US'] - BigQuery dataset location (default: 'US')
 * @param {number} [options.maxResults=10000] - Maximum rows to return (default: 10000)
 * @param {boolean} [options.useLegacySQL=false] - Use legacy SQL instead of standard SQL
 * @returns {Function} Data source handler function
 *
 * @example
 * const bigquery = createBigQueryDataSource({
 *   projectId: 'my-gcp-project',
 *   getAccessToken: async () => {
 *     // Return valid OAuth 2.0 access token
 *     return 'ya29.a0AfH6SMBx...';
 *   },
 *   location: 'US',
 *   maxResults: 50000
 * });
 */
export function createBigQueryDataSource(options = {}) {
  // Validate required options
  if (!options.projectId) {
    throw new Error('@chartml/data-bigquery: projectId is required');
  }

  if (!options.getAccessToken || typeof options.getAccessToken !== 'function') {
    throw new Error('@chartml/data-bigquery: getAccessToken must be a function');
  }

  const {
    projectId,
    getAccessToken,
    location = 'US',
    maxResults = 10000,
    useLegacySQL = false
  } = options;

  /**
   * BigQuery data source handler
   *
   * @param {Object} spec - ChartML source specification
   * @param {string} spec.query - BigQuery SQL query
   * @param {string} [spec.projectId] - Override project ID for this query
   * @param {number} [spec.maxResults] - Override max results for this query
   * @param {number} [spec.timeoutMs] - Query timeout in milliseconds (default: 30000)
   * @returns {Promise<Array>} Array of result rows
   */
  return async function bigqueryDataSource(spec) {
    // Validate spec
    if (!spec.query || typeof spec.query !== 'string') {
      throw new Error('@chartml/data-bigquery: query field is required and must be a SQL string');
    }

    const queryProjectId = spec.projectId || projectId;
    const queryMaxResults = spec.maxResults || maxResults;
    const timeoutMs = spec.timeoutMs || 30000;

    console.log('[BigQuery Plugin] Executing query:', {
      projectId: queryProjectId,
      queryLength: spec.query.length,
      maxResults: queryMaxResults
    });

    try {
      // Get OAuth access token
      const accessToken = await getAccessToken();
      if (!accessToken) {
        throw new Error('@chartml/data-bigquery: getAccessToken() returned null/undefined');
      }

      // Execute query using BigQuery REST API
      const result = await executeBigQuerySQL(
        queryProjectId,
        spec.query,
        accessToken,
        {
          location,
          maxResults: queryMaxResults,
          timeoutMs,
          useLegacySQL
        }
      );

      console.log('[BigQuery Plugin] Query completed:', {
        rowCount: result.length,
        columns: result.length > 0 ? Object.keys(result[0]) : []
      });

      return result;
    } catch (error) {
      console.error('[BigQuery Plugin] Query execution failed:', error);

      // Enhance error message with context
      const enhancedError = new Error(
        `BigQuery query failed: ${error.message}\n\nQuery: ${spec.query.substring(0, 200)}...`
      );
      enhancedError.originalError = error;
      enhancedError.query = spec.query;

      throw enhancedError;
    }
  };
}

/**
 * Execute BigQuery SQL using the REST API
 *
 * @param {string} projectId - GCP project ID
 * @param {string} sql - SQL query
 * @param {string} accessToken - OAuth 2.0 access token
 * @param {Object} options - Query options
 * @returns {Promise<Array>} Array of result rows
 */
async function executeBigQuerySQL(projectId, sql, accessToken, options = {}) {
  const {
    location = 'US',
    maxResults = 10000,
    timeoutMs = 30000,
    useLegacySQL = false
  } = options;

  // BigQuery jobs.query API endpoint
  const apiUrl = `https://bigquery.googleapis.com/bigquery/v2/projects/${projectId}/queries`;

  // Prepare request body
  const requestBody = {
    query: sql,
    useLegacySQL,
    location,
    maxResults,
    timeoutMs,
    useQueryCache: true // Enable query result caching
  };

  // Execute query
  const response = await fetch(apiUrl, {
    method: 'POST',
    headers: {
      'Authorization': `Bearer ${accessToken}`,
      'Content-Type': 'application/json'
    },
    body: JSON.stringify(requestBody)
  });

  if (!response.ok) {
    const errorData = await response.json().catch(() => ({}));
    throw new Error(
      `BigQuery API error (${response.status}): ${errorData.error?.message || response.statusText}`
    );
  }

  const data = await response.json();

  // Check for query errors
  if (data.errors && data.errors.length > 0) {
    throw new Error(`BigQuery query errors: ${JSON.stringify(data.errors)}`);
  }

  // If query is not complete, poll for results
  if (!data.jobComplete) {
    return await pollForResults(projectId, data.jobReference.jobId, accessToken, location, maxResults);
  }

  // Parse and return results
  return parseQueryResults(data);
}

/**
 * Poll for query results if job is not immediately complete
 *
 * @param {string} projectId - GCP project ID
 * @param {string} jobId - BigQuery job ID
 * @param {string} accessToken - OAuth access token
 * @param {string} location - Dataset location
 * @param {number} maxResults - Maximum results to return
 * @returns {Promise<Array>} Array of result rows
 */
async function pollForResults(projectId, jobId, accessToken, location, maxResults) {
  const maxAttempts = 30; // 30 seconds max
  const pollInterval = 1000; // 1 second

  for (let attempt = 0; attempt < maxAttempts; attempt++) {
    // Wait before polling
    await new Promise(resolve => setTimeout(resolve, pollInterval));

    // Get job results
    const apiUrl = `https://bigquery.googleapis.com/bigquery/v2/projects/${projectId}/queries/${jobId}`;
    const response = await fetch(`${apiUrl}?location=${location}&maxResults=${maxResults}`, {
      headers: {
        'Authorization': `Bearer ${accessToken}`
      }
    });

    if (!response.ok) {
      const errorData = await response.json().catch(() => ({}));
      throw new Error(
        `BigQuery polling error (${response.status}): ${errorData.error?.message || response.statusText}`
      );
    }

    const data = await response.json();

    if (data.jobComplete) {
      return parseQueryResults(data);
    }

    console.log(`[BigQuery Plugin] Polling attempt ${attempt + 1}/${maxAttempts}...`);
  }

  throw new Error('BigQuery query timeout: Job did not complete within 30 seconds');
}

/**
 * Parse BigQuery query results into array of objects
 *
 * @param {Object} data - BigQuery API response
 * @returns {Array} Array of row objects
 */
function parseQueryResults(data) {
  if (!data.schema || !data.schema.fields) {
    return [];
  }

  const schema = data.schema.fields;
  const rows = data.rows || [];

  return rows.map(row => {
    const obj = {};

    row.f.forEach((cell, index) => {
      const field = schema[index];
      const value = cell.v;

      // Type conversion based on BigQuery schema
      obj[field.name] = convertBigQueryValue(value, field.type);
    });

    return obj;
  });
}

/**
 * Convert BigQuery value to JavaScript type
 *
 * @param {*} value - Raw value from BigQuery
 * @param {string} type - BigQuery field type
 * @returns {*} Converted value
 */
function convertBigQueryValue(value, type) {
  if (value === null || value === undefined) {
    return null;
  }

  switch (type.toUpperCase()) {
    case 'INTEGER':
    case 'INT64':
      return parseInt(value, 10);

    case 'FLOAT':
    case 'FLOAT64':
    case 'NUMERIC':
    case 'BIGNUMERIC':
      return parseFloat(value);

    case 'BOOLEAN':
    case 'BOOL':
      return value === 'true' || value === true;

    case 'TIMESTAMP':
      return new Date(parseFloat(value) * 1000); // BigQuery timestamps are in seconds

    case 'DATE':
      return value; // Keep as string (YYYY-MM-DD format)

    case 'STRING':
    default:
      return String(value);
  }
}

export default createBigQueryDataSource;
