// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * DuckDB Middleware Plugin for ChartML
 *
 * Provides high-performance data loading and transformation using DuckDB WASM with caching.
 * Handles both JSON and Arrow format data. Supports both unnamed (single) and named
 * (multiple) data sources.
 *
 * ARCHITECTURE:
 * - DuckDB is ONLY accessible via this middleware
 * - Data source plugins are "dumb" - they just fetch and return raw data
 * - This middleware handles ALL DuckDB operations (loading data, running SQL)
 * - No other code in the application can access DuckDB
 *
 * DUAL-MODE SOURCES:
 * - Unnamed sources: Single data source (legacy), passthrough only
 * - Named sources: Multiple data sources loaded into DuckDB, with optional transform pipeline
 *
 * TRANSFORM PIPELINE:
 * Named sources can have a `transform:` block with three optional stages:
 *   1. sql     — SQL preprocessing (joins, CTEs, window functions)
 *   2. aggregate — Declarative aggregation (Phase 4)
 *   3. forecast  — Time series forecasting (Task 2)
 *
 * Stage logic lives in @kyomi/chartml-transform (shared with chart-renderer).
 * This file provides the DuckDB WASM adapter and handles caching/cleanup.
 *
 * CACHING FLOW:
 * Two-layer cache:
 *   __extract_{hash}    — Source data (fetched from datasource, cached across renders)
 *   __transform_{hash}  — Pipeline output (cached, invalidated on spec change or refresh)
 *
 * REQUEST DEDUPLICATION:
 * - Tracks in-flight fetch requests by tableId
 * - If multiple charts request the same data simultaneously (with bypassCache),
 *   only ONE fetch happens - other requests wait for the same Promise
 * - Prevents duplicate BigQuery calls when refreshing multiple charts
 */

import * as duckDbService from '../../../services/duckDbService.js';
import ms from 'ms';
import { hashAsync } from './utils/hash.js';
import { isNamedSources, runTransformPipeline } from '@kyomi/chartml-transform';

/**
 * Parse TTL string (e.g., "6h", "1d", "30m") to hours
 * Uses the 'ms' library which handles various time formats
 * @param {string} ttlString - TTL value like "6h", "24h", "1d", "7d"
 * @param {number} defaultHours - Default value if parsing fails (default: 24)
 * @returns {number} TTL in hours
 */
function parseTTL(ttlString, defaultHours = 24) {
  if (!ttlString) return defaultHours;

  try {
    const milliseconds = ms(ttlString);
    if (typeof milliseconds !== 'number' || isNaN(milliseconds)) {
      return defaultHours;
    }
    // Convert milliseconds to hours
    return milliseconds / (1000 * 60 * 60);
  } catch (e) {
    return defaultHours;
  }
}

// In-flight request tracker for deduplication
// Map of tableId -> {promise, timestamp}
// IMPORTANT: This tracks the ENTIRE operation (fetch + load into DuckDB), not just fetch
// This prevents race conditions where multiple charts try to load the same data
const inFlightRequests = new Map();

// hashSQL is imported as hashAsync from utils/hash.js
// Alias for backward compatibility within this file
const hashSQL = hashAsync;

// ---------------------------------------------------------------------------
// DuckDB utility wrappers
// ---------------------------------------------------------------------------

/**
 * Check if a table exists in DuckDB.
 * @param {string} tableName - Table name to check
 * @returns {Promise<boolean>} true if table exists
 */
async function tableExists(tableName) {
  const result = await duckDbService.execute(
    `SELECT 1 FROM information_schema.tables WHERE table_name = '${tableName.replace(/'/g, "''")}'`
  );
  return result.rows.length > 0;
}

/**
 * Execute raw SQL against DuckDB — no cache lookup, no TTL check.
 * Used for DDL (CREATE TABLE, DROP TABLE) and queries against tables
 * that are already known to exist (loaded by the middleware before pipeline runs).
 * @param {string} sql - SQL statement
 */
async function executeSQL(sql) {
  await duckDbService.execute(sql);
}

/**
 * Execute a SQL query via DuckDB and return result rows as objects.
 * @param {string} sql - SQL SELECT statement
 * @param {string} tableId - Reference table for cache management
 * @param {number} ttlHours - TTL for cache freshness
 * @returns {Promise<Object>} Result: { columns, rows, row_count, refreshedAt }
 */
async function querySQL(sql, tableId, ttlHours) {
  return await duckDbService.runSQL(sql, tableId, { ttlHours });
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/**
 * DuckDB middleware handler for ChartML
 *
 * Dispatches to the appropriate handler based on whether spec.data
 * uses named sources (multiple) or unnamed sources (single/legacy).
 *
 * @param {Object|Array|null} data - Input data (null on first call, Result Object from data source)
 * @param {Object} spec - Pipeline spec from ChartML
 * @param {Object} spec.transform - Transform specification (pipeline stages)
 * @param {Object} spec.data - Data source specification
 * @param {Object} context - ChartML context
 * @param {Function} context.fetchData - Lazy callback to fetch data from source
 * @param {boolean} context.bypassCache - If true, skip cache and fetch fresh data
 * @returns {Promise<Object>} Result Object: {data: Array<Object>, metadata: Object}
 */
export async function duckDbMiddleware(data, spec, context = {}) {
  // Normalize unnamed source to named "source" when transform is present.
  // This lets unnamed data sources flow through the transform pipeline
  // (aggregate, forecast, sql stages all work with any sourceMap key).
  if (spec.transform && spec.data && !isNamedSources(spec.data) && typeof spec.data !== 'string') {
    spec = { ...spec, data: { source: spec.data } };
  }

  const ttlHours = parseTTL(spec.data?.cache?.ttl);

  // Detect whether we're dealing with named or unnamed sources
  if (isNamedSources(spec.data)) {
    return await handleNamedSources(data, spec, context, ttlHours);
  } else {
    return await handleUnnamedSource(data, spec, context, ttlHours);
  }
}

/**
 * Handle unnamed (legacy) data sources — passthrough only.
 * When transform is present, unnamed sources are normalized to named before
 * reaching this point, so this path only handles non-transform unnamed sources.
 */
async function handleUnnamedSource(data, spec, context, ttlHours) {
  // Generate tableId from query hash
  let tableId = null;
  let query = '';

  if (spec.data?.query || context.resolvedDataSource?.query) {
    query = spec.data?.query || context.resolvedDataSource?.query || '';
    const sourceHash = await hashSQL(query);
    tableId = `__extract_${sourceHash}`;
  } else if (spec.data?.provider === 'inline' && spec.data?.rows) {
    const sourceString = JSON.stringify(spec.data.rows);
    const sourceHash = await hashSQL(sourceString);
    tableId = `__extract_${sourceHash}`;
  }

  // Try cache first (unless bypass requested)
  if (!context.bypassCache && tableId) {
    try {
      const result = await duckDbService.runSQL(`SELECT * FROM ${tableId}`, tableId, { ttlHours });
      return {
        data: convertToObjects(result),
        metadata: {
          refreshedAt: result.refreshedAt || Date.now(),
          cacheHit: true,
          tableId: tableId
        }
      };
    } catch (error) {
      // Cache miss - continue to fetch
    }
  }

  // Fetch data (with deduplication)
  if (!data && context.fetchData) {
    if (tableId && inFlightRequests.has(tableId)) {
      const inFlight = inFlightRequests.get(tableId);
      await inFlight.promise;
      const result = await duckDbService.runSQL(`SELECT * FROM ${tableId}`, tableId, { ttlHours });
      return {
        data: convertToObjects(result),
        metadata: { refreshedAt: Date.now(), cacheHit: true, tableId }
      };
    } else if (tableId) {
      const fetchAndLoadPromise = (async () => {
        const fetchedData = await context.fetchData();
        const isArrow = fetchedData?.metadata?.format === 'arrow';
        const isJSON = fetchedData?.metadata?.format === 'json';
        const loadOptions = {
          ttlHours, query, replace: context.bypassCache || false,
          columns: fetchedData?.metadata?.columns || null
        };

        if (isArrow) {
          await duckDbService.loadData(fetchedData.data, 'arrow', tableId, loadOptions);
        } else if (isJSON) {
          await duckDbService.loadData(fetchedData.data, 'json', tableId, loadOptions);
        }
        return fetchedData;
      })();

      inFlightRequests.set(tableId, { promise: fetchAndLoadPromise, timestamp: Date.now() });
      try {
        data = await fetchAndLoadPromise;
      } finally {
        inFlightRequests.delete(tableId);
      }
    } else {
      data = await context.fetchData();
    }
  }

  // Handle empty data
  const fetchedRows = data?.data !== undefined ? data.data : data;
  if (Array.isArray(fetchedRows) && fetchedRows.length === 0) {
    return {
      data: [],
      metadata: { ...data?.metadata, refreshedAt: Date.now(), tableId }
    };
  }

  // For Arrow/JSON data already loaded into DuckDB, query it
  const isArrow = data?.metadata?.format === 'arrow';
  const isJSON = data?.metadata?.format === 'json';

  if ((isArrow || isJSON) && tableId) {
    const result = await duckDbService.runSQL(`SELECT * FROM ${tableId}`, tableId, { ttlHours });
    return {
      data: convertToObjects(result),
      metadata: { ...data.metadata, refreshedAt: Date.now(), tableId }
    };
  }

  // Inline data — return as-is
  const actualData = data?.data !== undefined ? data.data : data;
  return {
    data: actualData,
    metadata: data?.metadata || {}
  };
}

/**
 * Handle named data sources — load each into DuckDB, optionally run transform pipeline.
 */
async function handleNamedSources(data, spec, context, ttlHours) {
  const sourceNames = Object.keys(spec.data);
  const sourceTableMap = {}; // { sourceName: tableId }
  let refreshedAt = Date.now();
  // Load each named source into DuckDB
  for (const sourceName of sourceNames) {
    const sourceSpec = spec.data[sourceName];

    // Generate tableId from source spec
    let tableId;
    let query = '';

    if (typeof sourceSpec === 'string') {
      // String reference - hash the reference name
      const sourceHash = await hashSQL(`ref:${sourceSpec}`);
      tableId = `__extract_${sourceHash}`;
    } else if (sourceSpec?.query) {
      query = sourceSpec.query;
      const sourceHash = await hashSQL(query);
      tableId = `__extract_${sourceHash}`;
    } else if (sourceSpec?.provider === 'inline' && sourceSpec?.rows) {
      const sourceString = JSON.stringify(sourceSpec.rows);
      const sourceHash = await hashSQL(sourceString);
      tableId = `__extract_${sourceHash}`;
    } else {
      const sourceHash = await hashSQL(JSON.stringify(sourceSpec));
      tableId = `__extract_${sourceHash}`;
    }

    sourceTableMap[sourceName] = tableId;

    // Try cache first
    if (!context.bypassCache) {
      try {
        await duckDbService.runSQL(`SELECT 1 FROM ${tableId} LIMIT 1`, tableId, { ttlHours });
        // Cache hit — table exists and is valid
        continue;
      } catch (error) {
        // Cache miss — need to fetch
      }
    }

    // Check for in-flight request
    if (inFlightRequests.has(tableId)) {
      await inFlightRequests.get(tableId).promise;
      continue;
    }

    // Fetch and load this source
    const fetchAndLoadPromise = (async () => {
      // fetchData(sourceName) fetches a specific named source
      const fetchedData = await context.fetchData(sourceName);

      const isArrow = fetchedData?.metadata?.format === 'arrow';
      const isJSON = fetchedData?.metadata?.format === 'json';
      const loadOptions = {
        ttlHours, query, replace: context.bypassCache || false,
        columns: fetchedData?.metadata?.columns || null
      };

      let loadResult;
      if (isArrow) {
        loadResult = await duckDbService.loadData(fetchedData.data, 'arrow', tableId, loadOptions);
      } else if (isJSON) {
        loadResult = await duckDbService.loadData(fetchedData.data, 'json', tableId, loadOptions);
      } else {
        // Inline or other — load as JSON
        const actualData = fetchedData?.data !== undefined ? fetchedData.data : fetchedData;
        loadResult = await duckDbService.loadData(
          Array.isArray(actualData) ? actualData : [actualData],
          'json', tableId, loadOptions
        );
      }

      return { fetchedData, loadResult };
    })();

    inFlightRequests.set(tableId, { promise: fetchAndLoadPromise, timestamp: Date.now() });
    let loadResult;
    try {
      ({ loadResult } = await fetchAndLoadPromise);
    } finally {
      inFlightRequests.delete(tableId);
    }

    // If the source returned empty data and no column metadata was available,
    // the worker returns success but doesn't create the table. Detect this
    // via rowCount=0 + empty columns and return empty result immediately —
    // running the transform pipeline against a non-existent table would error.
    if (loadResult?.rowCount === 0 && (!loadResult?.columns || loadResult.columns.length === 0)) {
      return { data: [], metadata: { refreshedAt, cacheHit: false, empty: true } };
    }
  }

  // If no transform and single source — passthrough (read directly from extract table)
  if (!spec.transform && sourceNames.length === 1) {
    const tableId = sourceTableMap[sourceNames[0]];
    const result = await duckDbService.runSQL(`SELECT * FROM ${tableId}`, tableId, { ttlHours });
    return {
      data: convertToObjects(result),
      metadata: { refreshedAt: result.refreshedAt || refreshedAt, cacheHit: false, tableId }
    };
  }

  // If no transform and multiple sources — shouldn't happen (validation prevents it)
  if (!spec.transform) {
    throw new Error('Named data sources require a transform block when multiple sources are defined');
  }

  // Build context for pipeline stages — raw DuckDB execution, no cache involvement.
  // Source cache validation already happened during source loading above.
  const pipelineContext = {
    /** Execute a SQL query and return result rows */
    runSQL: async (sql) => {
      return await duckDbService.execute(sql);
    },
    /** Execute a DDL/DML statement (no result expected) */
    execute: async (sql) => {
      await duckDbService.execute(sql);
    }
  };

  // Run the shared transform pipeline
  const bypassCache = context.bypassCache || false;
  const transformHash = await hashSQL(JSON.stringify(spec.transform));
  const transformTableId = `__transform_${transformHash}`;

  // Check cache — if result already exists and not bypassing, return it
  if (!bypassCache && await tableExists(transformTableId)) {
    const result = await duckDbService.execute(`SELECT * FROM "${transformTableId}"`);
    return {
      data: convertToObjects(result),
      metadata: { refreshedAt, cacheHit: true, tableId: transformTableId }
    };
  }

  const { finalTableId, intermediateTables } = await runTransformPipeline(
    sourceTableMap,
    spec.transform,
    pipelineContext
  );

  try {
    // Store final result as cached transform table
    await executeSQL(
      `CREATE OR REPLACE TABLE "${transformTableId}" AS SELECT * FROM "${finalTableId}"`
    );

    // Read rows from the cached transform table
    const result = await duckDbService.execute(`SELECT * FROM "${transformTableId}"`);

    return {
      data: convertToObjects(result),
      metadata: {
        refreshedAt: refreshedAt,
        cacheHit: false,
        tableId: transformTableId
      }
    };
  } finally {
    // Clean up intermediate tables (not source __extract_* tables or the transform cache table)
    const sourceTables = new Set(Object.values(sourceTableMap));
    for (const table of intermediateTables) {
      if (!sourceTables.has(table) && table !== transformTableId) {
        try {
          await executeSQL(`DROP TABLE IF EXISTS "${table}"`);
        } catch (_) { /* ignore cleanup errors */ }
      }
    }
  }
}

/**
 * Convert queryExecutor result to ChartML format
 * @param {Object} result - Query result with columns and rows arrays
 * @returns {Array<Object>} Array of row objects
 */
function convertToObjects(result) {
  if (!result.rows || !result.columns) {
    return [];
  }

  return result.rows.map(rowArray => {
    const obj = {};
    result.columns.forEach((col, idx) => {
      obj[col] = rowArray[idx];
    });
    return obj;
  });
}
