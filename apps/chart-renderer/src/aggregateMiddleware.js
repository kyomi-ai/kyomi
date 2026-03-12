// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Server-side DuckDB Transform Middleware for ChartML
 *
 * Handles the transform pipeline for server-side chart rendering (PDF export).
 * The Python backend resolves data sources to inline rows before sending specs
 * to the renderer. This middleware:
 *
 * 1. Detects named data sources in spec.data
 * 2. Loads inline rows into DuckDB tables (using content-hashed table IDs)
 * 3. Runs the shared transform pipeline (sql → aggregate → forecast stages)
 * 4. Returns result rows to ChartML for rendering
 *
 * Transform pipeline logic lives in @kyomi/chartml-transform (shared with the
 * frontend). This file provides the Node.js DuckDB adapter, data loading, and
 * result reading.
 *
 * Like the frontend middleware, source data tables are cached using content-based
 * hashing — same data produces the same table ID, avoiding redundant loads when
 * multiple charts in a PDF share the same source data. No data fetching or
 * deduplication is needed since the Python backend pre-resolves all data.
 *
 * Unlike the frontend, there is NO two-layer caching for transform results.
 * Each render processes fresh (the Python backend sends inline data for PDF export).
 * Intermediate __stage_* tables are cleaned up after pipeline completion.
 */

import { DuckDBInstance } from '@duckdb/node-api';
import { writeFileSync, unlinkSync, mkdirSync } from 'fs';
import { join } from 'path';
import { tmpdir } from 'os';
import { createHash } from 'crypto';

import { isNamedSources, runTransformPipeline } from '@kyomi/chartml-transform';

let dbInstance = null;
let quackstatsLoaded = false;

// QuackStats extension path — configurable via env var for Docker
const QUACKSTATS_EXTENSION_PATH = process.env.QUACKSTATS_EXTENSION_PATH || null;

/**
 * Initialize DuckDB instance (singleton — reused across renders).
 */
async function getDB() {
  if (!dbInstance) {
    dbInstance = await DuckDBInstance.create(':memory:', {
      allow_unsigned_extensions: 'true',
    });

    // Try loading quackstats extension for forecast() support
    if (QUACKSTATS_EXTENSION_PATH) {
      const conn = await dbInstance.connect();
      try {
        await conn.run(`LOAD '${QUACKSTATS_EXTENSION_PATH}'`);
        quackstatsLoaded = true;
        console.log(`[DuckDB] QuackStats extension loaded from ${QUACKSTATS_EXTENSION_PATH}`);
      } catch (e) {
        console.warn(`[DuckDB] QuackStats failed to load: ${e.message}`);
      } finally {
        conn.disconnectSync();
      }
    } else {
      console.log('[DuckDB] No QUACKSTATS_EXTENSION_PATH set — forecast() will not be available');
    }
  }
  return dbInstance;
}

/**
 * Generate a SHA-256 hash of content for deterministic table naming.
 * Used for source table (__extract_*) naming — matches the frontend's hashSQL()
 * approach so same data always produces the same table ID.
 */
function hashContent(content) {
  return createHash('sha256').update(content).digest('hex');
}

// Temp directory for JSON files fed to DuckDB's read_json_auto
const TEMP_DIR = join(tmpdir(), 'chartml-duckdb');
mkdirSync(TEMP_DIR, { recursive: true });

/**
 * Check if a table already exists in DuckDB (cache hit).
 */
async function tableExists(conn, tableName) {
  try {
    await conn.run(`SELECT 1 FROM "${tableName}" LIMIT 1`);
    return true;
  } catch (_) {
    return false;
  }
}

/**
 * Load an array of row objects into a DuckDB table.
 * Writes JSON to a temp file so DuckDB's read_json_auto can infer types properly.
 * Skips loading if the table already exists (cache hit).
 */
async function loadRowsIntoTable(conn, tableName, rows) {
  if (!rows || rows.length === 0) {
    return;
  }

  if (await tableExists(conn, tableName)) {
    return;
  }

  const tempFile = join(TEMP_DIR, `${tableName}_${Date.now()}.json`);
  try {
    writeFileSync(tempFile, JSON.stringify(rows));
    await conn.run(
      `CREATE OR REPLACE TABLE "${tableName}" AS SELECT * FROM read_json_auto('${tempFile}')`
    );
  } finally {
    try { unlinkSync(tempFile); } catch (_) { /* ignore cleanup errors */ }
  }
}

/**
 * Convert a DuckDB native value to a plain JS value.
 *
 * The native @duckdb/node-api returns wrapper objects for non-primitive types:
 *   DuckDBDateValue      {days}          → "2025-01-15"       (string, D3-parseable)
 *   DuckDBTimestampValue {micros}        → "2025-01-15 10:30:00" (string, D3-parseable)
 *   DuckDBTimestampTZValue {micros}      → "2025-01-15 10:30:00+00" (string)
 *   DuckDBTimeValue      {micros}        → "10:30:00"         (string)
 *   DuckDBIntervalValue  {months,days,micros} → "5 days"      (string)
 *   DuckDBListValue      {items}         → [1, 2, 3]          (array, recursively converted)
 *   BigInt                               → Number
 *
 * The frontend WASM DuckDB (with castBigIntToDouble) returns dates as JS Date
 * objects and BigInts as numbers. Using toString() for date/time types produces
 * ISO-style strings that D3 can parse identically.
 */
function convertValue(value) {
  if (value === null || value === undefined) return value;
  if (typeof value === 'bigint') return Number(value);
  if (typeof value !== 'object') return value;

  const ctor = value.constructor?.name;
  if (!ctor) return value;

  // Date/time wrapper types — toString() produces D3-parseable strings
  if (ctor === 'DuckDBDateValue' ||
      ctor === 'DuckDBTimestampValue' ||
      ctor === 'DuckDBTimestampTZValue' ||
      ctor === 'DuckDBTimeValue' ||
      ctor === 'DuckDBIntervalValue') {
    return value.toString();
  }

  // List type — recursively convert items
  if (ctor === 'DuckDBListValue') {
    return value.items.map(convertValue);
  }

  return value;
}

/**
 * Execute a query and return results as an array of row objects.
 * Converts DuckDB native wrapper types to plain JS values that
 * ChartML/D3 can consume (matching frontend WASM behavior).
 */
async function queryToObjects(conn, sql) {
  const reader = await conn.runAndReadAll(sql);
  const rows = reader.getRowObjects();
  return rows.map(row => {
    const converted = {};
    for (const [key, value] of Object.entries(row)) {
      converted[key] = convertValue(value);
    }
    return converted;
  });
}

// ---------------------------------------------------------------------------
// Standalone transform execution (used by /transform endpoint and middleware)
// ---------------------------------------------------------------------------

/**
 * Execute the DuckDB transform pipeline on named source data.
 *
 * Loads named sources into content-hashed DuckDB tables, runs the shared
 * transform pipeline (sql → aggregate → forecast stages), reads results,
 * and cleans up intermediate tables.
 *
 * @param {Object} data - Named sources: { sourceName: { rows: [...] } }
 * @param {Object} transform - Transform config: { sql?, aggregate?, forecast? }
 * @returns {Promise<Object>} Result with { rows: Array<Object>, metadata: Object }
 */
export async function executeTransform(data, transform) {
  const db = await getDB();
  const conn = await db.connect();

  try {
    const sourceNames = Object.keys(data);
    const sourceTableMap = {}; // { sourceName: tableId }

    // Load each named source into DuckDB with content-hashed table names
    let hasEmptySource = false;
    for (const sourceName of sourceNames) {
      const sourceSpec = data[sourceName];
      const rows = sourceSpec?.rows || [];
      const contentHash = hashContent(JSON.stringify(rows));
      const tableId = `__extract_${contentHash}`;
      sourceTableMap[sourceName] = tableId;

      if (rows.length === 0) {
        hasEmptySource = true;
      }

      await loadRowsIntoTable(conn, tableId, rows);
    }

    // If any source has empty data, the table won't exist in DuckDB —
    // return empty result instead of running the pipeline against a missing table.
    if (hasEmptySource) {
      return { rows: [], metadata: { empty: true } };
    }

    // Build context wrapping the Node DuckDB connection for stage use
    const pipelineContext = {
      runSQL: async (sql) => await queryToObjects(conn, sql),
      execute: async (sql) => { await conn.run(sql); },
    };

    const { finalTableId, intermediateTables } = await runTransformPipeline(
      sourceTableMap,
      transform,
      pipelineContext
    );

    try {
      // Read the final result table
      const resultRows = await queryToObjects(conn, `SELECT * FROM "${finalTableId}"`);

      return {
        rows: resultRows,
        metadata: { aggregated: true },
      };
    } finally {
      // Clean up intermediate __stage_* tables (not source __extract_* tables)
      const sourceTables = new Set(Object.values(sourceTableMap));
      for (const table of intermediateTables) {
        if (!sourceTables.has(table)) {
          try {
            await conn.run(`DROP TABLE IF EXISTS "${table}"`);
          } catch (_) { /* ignore cleanup errors */ }
        }
      }
    }
  } finally {
    conn.disconnectSync();
  }
}

// ---------------------------------------------------------------------------
// Middleware export
// ---------------------------------------------------------------------------

/**
 * Transform middleware for ChartML's setTransformMiddleware().
 *
 * Called by ChartML during the render pipeline when a spec has a transform section.
 * Uses content-based hashing for source table names (matching the frontend approach):
 *   - Same data → same hash → same table → cache hit (skip loading)
 *   - Different data → different hash → different table → no clash
 * User-written TEMP TABLEs in transform SQL are connection-scoped, so they
 * naturally don't clash between concurrent renders.
 *
 * @param {Object|null} data - Input data from data source resolution
 * @param {Object} spec - ChartML spec with data and transform sections
 * @param {Object} context - ChartML context
 * @returns {Promise<Object>} Result with { data: Array<Object>, metadata: Object }
 */
export async function aggregateMiddleware(data, spec, context = {}) {
  // Normalize unnamed source to named "source" when transform is present
  if (spec.transform && spec.data && !isNamedSources(spec.data) && typeof spec.data !== 'string') {
    spec = { ...spec, data: { source: spec.data } };
  }

  // Unnamed source or no named sources — passthrough
  // In ChartML v2, data=null on first call and fetchData is in context.
  // We must call fetchData to get the data before passing through.
  if (!spec.data || !isNamedSources(spec.data)) {
    if (!data && context.fetchData) {
      data = await context.fetchData();
    }
    const actualData = data?.data !== undefined ? data.data : data;
    return {
      data: actualData || [],
      metadata: data?.metadata || {},
    };
  }

  // Delegate to executeTransform if transform section is present
  if (spec.transform) {
    const result = await executeTransform(spec.data, spec.transform);
    return {
      data: result.rows,
      metadata: result.metadata,
    };
  }

  // Named source(s) without transform — load and return data from first source
  const db = await getDB();
  const conn = await db.connect();

  try {
    const sourceNames = Object.keys(spec.data);

    if (sourceNames.length === 1) {
      const sourceSpec = spec.data[sourceNames[0]];
      const rows = sourceSpec?.rows || [];
      const contentHash = hashContent(JSON.stringify(rows));
      const tableId = `__extract_${contentHash}`;

      await loadRowsIntoTable(conn, tableId, rows);
      const resultRows = await queryToObjects(conn, `SELECT * FROM "${tableId}"`);
      return {
        data: resultRows,
        metadata: {},
      };
    }

    // Multiple named sources without transform — shouldn't happen
    // (ChartML validation prevents it), but return empty gracefully
    return { data: [], metadata: {} };
  } finally {
    conn.disconnectSync();
  }
}
