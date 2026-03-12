// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * BigQuery Direct API Client
 *
 * Calls BigQuery REST API directly from browser (no backend proxy).
 * This is SLOWER than Arrow streaming but saves backend bandwidth.
 * Used for Free and Basic tier users.
 *
 * Flow:
 * 1. Submit query job to BigQuery REST API
 * 2. Poll for job completion
 * 3. Download results in JSON format (paginated)
 * 4. Convert JSON to typed arrays for DuckDB
 */

const BIGQUERY_API_BASE = 'https://bigquery.googleapis.com/bigquery/v2';

/**
 * Execute BigQuery query using direct REST API
 *
 * @param {string} sql - SQL query to execute
 * @param {string} accessToken - OAuth access token
 * @param {string} billingProject - GCP billing project ID
 * @returns {Promise<Object>} Result with columns and rows
 */
export async function executeQueryDirect(sql, accessToken, billingProject) {

    try {
        // Step 1: Submit query job
        const jobResponse = await fetch(
            `${BIGQUERY_API_BASE}/projects/${billingProject}/queries`,
            {
                method: 'POST',
                headers: {
                    'Authorization': `Bearer ${accessToken}`,
                    'Content-Type': 'application/json'
                },
                body: JSON.stringify({
                    query: sql,
                    useLegacySql: false,
                    maxResults: 10000,  // Max results per page
                    timeoutMs: 60000    // 60 second timeout for initial response
                })
            }
        );

        if (!jobResponse.ok) {
            const errorText = await jobResponse.text();
            throw new Error(`BigQuery API error (${jobResponse.status}): ${errorText}`);
        }

        let jobData = await jobResponse.json();

        // Step 2: Poll until job completes
        const startTime = Date.now();
        let pollCount = 0;

        while (!jobData.jobComplete) {
            pollCount++;

            // Wait before polling (exponential backoff)
            const delay = Math.min(1000 * Math.pow(1.5, pollCount), 5000); // Max 5s between polls
            await sleep(delay);

            // Poll job status
            const jobId = jobData.jobReference.jobId;
            const pollResponse = await fetch(
                `${BIGQUERY_API_BASE}/projects/${billingProject}/queries/${jobId}`,
                {
                    headers: {
                        'Authorization': `Bearer ${accessToken}`
                    }
                }
            );

            if (!pollResponse.ok) {
                const errorText = await pollResponse.text();
                throw new Error(`Poll failed (${pollResponse.status}): ${errorText}`);
            }

            jobData = await pollResponse.json();

            // Timeout after 5 minutes
            if (Date.now() - startTime > 300000) {
                throw new Error('Query timeout after 5 minutes');
            }

        }


        // Check for errors
        if (jobData.errors && jobData.errors.length > 0) {
            const error = jobData.errors[0];
            throw new Error(`BigQuery error: ${error.message}`);
        }

        // Step 3: Get schema
        const schema = jobData.schema;
        if (!schema || !schema.fields) {
            throw new Error('No schema returned from BigQuery');
        }

        const columns = schema.fields.map(f => ({
            name: f.name,
            type: f.type,
            mode: f.mode || 'NULLABLE'
        }));


        // Step 4: Download all rows (handle pagination)
        let allRows = [];
        let pageToken = jobData.pageToken;

        // Add first page of results
        if (jobData.rows && jobData.rows.length > 0) {
            allRows = allRows.concat(jobData.rows);
        }

        // Fetch remaining pages
        let pageNum = 2;
        while (pageToken) {
            const pageResponse = await fetch(
                `${BIGQUERY_API_BASE}/projects/${billingProject}/queries/${jobData.jobReference.jobId}?pageToken=${pageToken}&maxResults=10000`,
                {
                    headers: {
                        'Authorization': `Bearer ${accessToken}`
                    }
                }
            );

            if (!pageResponse.ok) {
                const errorText = await pageResponse.text();
                throw new Error(`Page fetch failed (${pageResponse.status}): ${errorText}`);
            }

            const pageData = await pageResponse.json();

            if (pageData.rows && pageData.rows.length > 0) {
                allRows = allRows.concat(pageData.rows);
            }

            pageToken = pageData.pageToken;
            pageNum++;
        }


        // Step 5: Convert JSON rows to typed format for DuckDB
        const typedData = convertBigQueryJsonToDuckDB(allRows, columns);

        return {
            columns: columns.map(c => c.name),
            rows: typedData,
            rowCount: allRows.length,
            schema: columns
        };

    } catch (error) {
        throw error;
    }
}

/**
 * Convert BigQuery JSON format to typed arrays for DuckDB
 *
 * BigQuery returns rows as: { f: [{ v: "value1" }, { v: "value2" }] }
 * We need to convert to: [[value1, value2], ...]
 *
 * @param {Array} rows - BigQuery rows
 * @param {Array} columns - Column schema
 * @returns {Array} Typed rows
 */
function convertBigQueryJsonToDuckDB(rows, columns) {
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

                case 'DATE':
                    return new Date(value);

                case 'STRING':
                default:
                    return String(value);
            }
        });
    });
}

/**
 * Sleep for specified milliseconds
 */
function sleep(ms) {
    return new Promise(resolve => setTimeout(resolve, ms));
}

export default {
    executeQueryDirect
};
