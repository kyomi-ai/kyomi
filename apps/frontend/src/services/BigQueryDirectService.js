// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * BigQuery Direct Service - Unified BigQuery REST API Client
 *
 * Single source of truth for all direct BigQuery API calls.
 * Used by:
 * - SQL Editor (interactive queries with pagination)
 * - BigQuery data source plugin (bulk loading for charts)
 *
 * Features:
 * - In-memory token caching (reduces backend calls)
 * - Dry run support (cost estimation)
 * - True pagination (fetch one page at a time)
 * - Bulk mode (fetch all pages at once)
 */

const BIGQUERY_API_BASE = 'https://bigquery.googleapis.com/bigquery/v2';
const TOKEN_CACHE_DURATION_MS = 50 * 60 * 1000; // 50 minutes (tokens valid 1 hour, refresh early)

class BigQueryDirectService {
    constructor() {
        // Per-datasource token cache: { [slug]: { access_token, billing_project, expiry } }
        this.tokenCacheBySlug = {};
        // Per-datasource pending promises to prevent concurrent fetches
        this.tokenPromiseBySlug = {};
    }

    /**
     * Get OAuth access token with per-datasource caching
     *
     * @param {string} datasourceSlug - Datasource slug (required for multi-datasource support)
     * @returns {Promise<{access_token: string, billing_project: string}>}
     */
    async getAccessToken(datasourceSlug) {
        if (!datasourceSlug) {
            throw new Error('datasourceSlug is required for BigQuery token request');
        }

        const now = Date.now();
        const cached = this.tokenCacheBySlug[datasourceSlug];

        // Return cached token if still valid
        if (cached && cached.expiry > now) {
            return { access_token: cached.access_token, billing_project: cached.billing_project };
        }

        // If already fetching token for this datasource, wait for that request
        if (this.tokenPromiseBySlug[datasourceSlug]) {
            return await this.tokenPromiseBySlug[datasourceSlug];
        }

        // Fetch new token for this datasource
        this.tokenPromiseBySlug[datasourceSlug] = (async () => {
            try {
                const response = await fetch('/api/v1/bigquery/request-access-token', {
                    method: 'POST',
                    credentials: 'include',
                    headers: {
                        'Content-Type': 'application/json'
                    },
                    body: JSON.stringify({ datasource_slug: datasourceSlug })
                });

                if (!response.ok) {
                    const errorText = await response.text();
                    throw new Error(`Token request failed: ${response.status} ${errorText}`);
                }

                const data = await response.json();

                // Cache token for this datasource
                this.tokenCacheBySlug[datasourceSlug] = {
                    access_token: data.access_token,
                    billing_project: data.billing_project,
                    expiry: Date.now() + TOKEN_CACHE_DURATION_MS
                };

                return { access_token: data.access_token, billing_project: data.billing_project };

            } catch (error) {
                throw error;
            } finally {
                delete this.tokenPromiseBySlug[datasourceSlug];
            }
        })();

        return await this.tokenPromiseBySlug[datasourceSlug];
    }

    /**
     * Dry run query to estimate cost without executing
     *
     * @param {string} sql - SQL query to validate
     * @param {string} datasourceSlug - Datasource slug
     * @returns {Promise<{bytesProcessed: number, estimatedCostUSD: number, canExecute: boolean}>}
     */
    async dryRunQuery(sql, datasourceSlug) {
        const { access_token, billing_project } = await this.getAccessToken(datasourceSlug);

        const response = await fetch(
            `${BIGQUERY_API_BASE}/projects/${billing_project}/queries`,
            {
                method: 'POST',
                headers: {
                    'Authorization': `Bearer ${access_token}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    query: sql,
                    dryRun: true,
                    useLegacySql: false
                })
            }
        );

        if (!response.ok) {
            // Clear token cache on 401 errors so next request gets a fresh token
            if (response.status === 401) {
                this.tokenCache = null;
                this.tokenExpiry = null;
            }

            let errorMessage = `Dry run failed (${response.status})`;

            try {
                const errorData = await response.json();

                // BigQuery REST API error format:
                // { error: { code: 400, message: "...", errors: [{message, reason, location}] } }
                if (errorData.error && errorData.error.message) {
                    errorMessage = errorData.error.message;

                    // The message often contains line:column info like:
                    // "Syntax error: Expected end of input but got keyword SELECT at [2:1]"
                    // Keep the full message - frontend parser will extract location
                }
            } catch (e) {
                // If JSON parsing fails, use text response
                const errorText = await response.text();
                errorMessage = `${errorMessage}: ${errorText}`;
            }

            throw new Error(errorMessage);
        }

        const data = await response.json();

        const bytesProcessed = parseInt(data.totalBytesProcessed || '0', 10);
        const tiBProcessed = bytesProcessed / (1024 ** 4);
        const estimatedCostUSD = tiBProcessed * 6.25; // $6.25 per TiB

        return {
            bytesProcessed,
            estimatedCostUSD,
            canExecute: true // User controls their own billing
        };
    }

    /**
     * Execute query and return paginated results
     *
     * @param {string} sql - SQL query to execute
     * @param {string} datasourceSlug - Datasource slug
     * @param {Object} options - Pagination options
     * @param {number} options.maxResults - Results per page (default: 50)
     * @param {number} options.timeoutMs - Query timeout (default: 60000)
     * @returns {Promise<{jobId: string, rows: Array, schema: Array, totalRows: number, pageToken: string|null}>}
     */
    async executeQuery(sql, datasourceSlug, options = {}) {
        const { maxResults = 50, timeoutMs = 60000 } = options;

        const { access_token, billing_project } = await this.getAccessToken(datasourceSlug);

        // Step 1: Submit query job
        const jobResponse = await fetch(
            `${BIGQUERY_API_BASE}/projects/${billing_project}/queries`,
            {
                method: 'POST',
                headers: {
                    'Authorization': `Bearer ${access_token}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    query: sql,
                    useLegacySql: false,
                    maxResults,
                    timeoutMs
                })
            }
        );

        if (!jobResponse.ok) {
            const errorText = await jobResponse.text();
            // Clear token cache on 401 errors so next request gets a fresh token
            if (jobResponse.status === 401) {
                this.tokenCache = null;
                this.tokenExpiry = null;
            }
            throw new Error(`Query failed (${jobResponse.status}): ${errorText}`);
        }

        let jobData = await jobResponse.json();

        // Step 2: Poll if not complete
        const startTime = Date.now();
        let pollCount = 0;

        while (!jobData.jobComplete) {
            pollCount++;

            // Exponential backoff (1s, 1.5s, 2.25s, ..., max 5s)
            const delay = Math.min(1000 * Math.pow(1.5, pollCount), 5000);
            await sleep(delay);

            // Timeout after 5 minutes
            if (Date.now() - startTime > 300000) {
                throw new Error('Query timeout after 5 minutes');
            }

            const pollResponse = await fetch(
                `${BIGQUERY_API_BASE}/projects/${billing_project}/queries/${jobData.jobReference.jobId}`,
                {
                    headers: {
                        'Authorization': `Bearer ${access_token}`
                    }
                }
            );

            if (!pollResponse.ok) {
                const errorText = await pollResponse.text();
                // Clear token cache on 401 errors
                if (pollResponse.status === 401) {
                    this.tokenCache = null;
                    this.tokenExpiry = null;
                }
                throw new Error(`Poll failed (${pollResponse.status}): ${errorText}`);
            }

            jobData = await pollResponse.json();
        }

        // Check for errors
        if (jobData.errors && jobData.errors.length > 0) {
            const error = jobData.errors[0];
            throw new Error(`BigQuery error: ${error.message}`);
        }

        // Extract schema and results
        const schema = jobData.schema;
        if (!schema || !schema.fields) {
            throw new Error('No schema returned from BigQuery');
        }

        const columns = schema.fields.map(f => ({
            name: f.name,
            type: f.type,
            mode: f.mode || 'NULLABLE'
        }));

        // Convert rows from BigQuery format
        const rows = this._convertBigQueryRows(jobData.rows || [], columns);

        return {
            jobId: jobData.jobReference.jobId,
            rows,
            columns,
            totalRows: parseInt(jobData.totalRows || '0', 10),
            pageToken: jobData.pageToken || null
        };
    }

    /**
     * Fetch specific page of results using startIndex (supports jumping to any page)
     *
     * @param {string} jobId - Job ID from initial query
     * @param {string} datasourceSlug - Datasource slug
     * @param {number} page - Page number (1-indexed)
     * @param {number} maxResults - Results per page (default: 50)
     * @returns {Promise<{rows: Array, pageToken: string|null}>}
     */
    async fetchPage(jobId, datasourceSlug, page, maxResults = 50) {
        const startIndex = (page - 1) * maxResults;

        const { access_token, billing_project } = await this.getAccessToken(datasourceSlug);

        const response = await fetch(
            `${BIGQUERY_API_BASE}/projects/${billing_project}/queries/${jobId}?startIndex=${startIndex}&maxResults=${maxResults}`,
            {
                headers: {
                    'Authorization': `Bearer ${access_token}`
                }
            }
        );

        if (!response.ok) {
            const errorText = await response.text();
            throw new Error(`Page fetch failed (${response.status}): ${errorText}`);
        }

        const data = await response.json();

        // Use cached columns from initial query
        const columns = data.schema.fields.map(f => ({
            name: f.name,
            type: f.type,
            mode: f.mode || 'NULLABLE'
        }));

        const rows = this._convertBigQueryRows(data.rows || [], columns);

        return {
            rows,
            pageToken: data.pageToken || null
        };
    }

    /**
     * Fetch next page of results from a completed query (sequential pagination)
     *
     * @param {string} jobId - Job ID from initial query
     * @param {string} datasourceSlug - Datasource slug
     * @param {string} pageToken - Page token from previous response
     * @param {number} maxResults - Results per page (default: 50)
     * @returns {Promise<{rows: Array, pageToken: string|null}>}
     */
    async fetchNextPage(jobId, datasourceSlug, pageToken, maxResults = 50) {
        const { access_token, billing_project } = await this.getAccessToken(datasourceSlug);

        const response = await fetch(
            `${BIGQUERY_API_BASE}/projects/${billing_project}/queries/${jobId}?pageToken=${pageToken}&maxResults=${maxResults}`,
            {
                headers: {
                    'Authorization': `Bearer ${access_token}`
                }
            }
        );

        if (!response.ok) {
            const errorText = await response.text();
            throw new Error(`Page fetch failed (${response.status}): ${errorText}`);
        }

        const data = await response.json();

        // Use cached columns from initial query
        const columns = data.schema.fields.map(f => ({
            name: f.name,
            type: f.type,
            mode: f.mode || 'NULLABLE'
        }));

        const rows = this._convertBigQueryRows(data.rows || [], columns);

        return {
            rows,
            pageToken: data.pageToken || null
        };
    }

    /**
     * Fetch all remaining pages from an existing job using jobId
     *
     * This method is designed to be called AFTER executeQuery() to fetch remaining pages
     * without re-executing the query. Useful when you already have a jobId and first page.
     *
     * @param {string} jobId - Job ID from initial executeQuery call
     * @param {string} datasourceSlug - Datasource slug
     * @param {Array} firstPageRows - Rows from the first page (already fetched)
     * @param {Array} columns - Column schema from first page
     * @param {string|null} pageToken - Page token from first page (null if no more pages)
     * @param {Object} options - Fetch options
     * @param {number} options.maxResults - Results per page (default: 10000)
     * @param {Function} options.onProgress - Progress callback (rowsStreamed, totalRows)
     * @param {number} options.totalRows - Total row count (for progress reporting)
     * @returns {Promise<{rows: Array, columns: Array, rowCount: number}>}
     */
    async fetchJobResults(jobId, datasourceSlug, firstPageRows, columns, pageToken, options = {}) {
        const { maxResults = 10000, onProgress, totalRows } = options;

        let allRows = [...firstPageRows];

        // Notify initial progress
        if (onProgress && totalRows) {
            onProgress(allRows.length, totalRows);
        }

        // Fetch remaining pages using jobId (no re-execution!)
        while (pageToken) {
            const nextPage = await this.fetchNextPage(jobId, datasourceSlug, pageToken, maxResults);
            allRows = allRows.concat(nextPage.rows);
            pageToken = nextPage.pageToken;

            // Notify progress
            if (onProgress && totalRows) {
                onProgress(allRows.length, totalRows);
            }
        }

        return {
            rows: allRows,
            columns: columns,
            rowCount: allRows.length
        };
    }

    /**
     * Convert BigQuery JSON format to typed arrays
     *
     * BigQuery returns: { f: [{ v: "value1" }, { v: "value2" }] }
     * We convert to: [value1, value2]
     *
     * @private
     */
    _convertBigQueryRows(rows, columns) {
        if (!rows || rows.length === 0) {
            return [];
        }

        return rows.map(row => {
            if (!row.f) {
                return columns.map(() => null);
            }

            return row.f.map((cell, idx) => {
                const columnType = columns[idx].type;
                const value = cell.v;

                // Handle null values
                if (value === null || value === undefined) {
                    return null;
                }

                // Convert based on BigQuery type
                return this._convertValue(value, columnType);
            });
        });
    }

    /**
     * Convert single BigQuery value to appropriate JavaScript type
     *
     * @private
     */
    _convertValue(value, columnType) {
        switch (columnType) {
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
                // BigQuery timestamps are in seconds since epoch
                return new Date(parseFloat(value) * 1000);

            case 'DATETIME':
                // Format: "YYYY-MM-DD HH:MM:SS[.SSSSSS]"
                return new Date(value);

            case 'DATE':
                // Format: "YYYY-MM-DD"
                return new Date(value);

            case 'TIME':
                // Format: "HH:MM:SS[.SSSSSS]"
                // Keep as string - no Date equivalent for time-only
                return value;

            case 'BYTES':
                // Base64 encoded - keep as string
                return value;

            case 'GEOGRAPHY':
                // WKT string - keep as string
                return value;

            case 'ARRAY':
            case 'RECORD':
            case 'STRUCT':
                // Nested types - keep as-is (object/array)
                // TODO: Could add recursive conversion if needed
                return value;

            case 'STRING':
            default:
                return String(value);
        }
    }

    /**
     * Clear token cache (useful for testing, logout, or datasource config changes)
     *
     * @param {string} [datasourceSlug] - If provided, clear only that datasource's cache. Otherwise clear all.
     */
    clearCache(datasourceSlug) {
        if (datasourceSlug) {
            delete this.tokenCacheBySlug[datasourceSlug];
            delete this.tokenPromiseBySlug[datasourceSlug];
        } else {
            this.tokenCacheBySlug = {};
            this.tokenPromiseBySlug = {};
        }
    }
}

/**
 * Sleep utility
 */
function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

// Export singleton instance
const bigQueryDirectService = new BigQueryDirectService();
export default bigQueryDirectService;
