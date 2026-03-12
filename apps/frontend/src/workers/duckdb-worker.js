// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * DuckDB Dedicated Worker - Per-tab DuckDB instance
 *
 * This worker runs in each tab and manages a DuckDB-WASM instance.
 * Only ONE tab's worker is "active" at a time (coordinated by SharedWorker router).
 *
 * Architecture (Notion pattern):
 * - Each tab has its own Worker with DuckDB
 * - SharedWorker router coordinates which tab is active
 * - All queries route through SharedWorker → active tab's Worker
 * - Web Locks detect tab closure and elect new active tab
 *
 * Message Protocol:
 * - LOAD_DATA: Load pre-fetched data (Arrow/JSON) into DuckDB table
 * - RUN_SQL: Execute SQL query on existing table
 * - INVALIDATE_CACHE: Remove cached table
 */

import * as duckdb from '@duckdb/duckdb-wasm';
import { cleanupStaleTables } from './cleanupStaleTables.js';

// =============================================================================
// STATE MANAGEMENT
// =============================================================================

let dbInstance = null;
let initPromise = null;
let isActive = false; // Whether this worker is the active one



// =============================================================================
// WORKER MESSAGE HANDLING
// =============================================================================

self.addEventListener('message', async (event) => {
    await handleMessage(event.data);
});

// =============================================================================
// MESSAGE HANDLING
// =============================================================================

async function handleMessage(message, replyPort = null) {
    const { type, requestId, payload, port } = message;


    try {
        let result;

        switch (type) {
            case 'COORDINATOR_PORT': {
                // Main thread is giving us a MessagePort to communicate with coordinator

                // Get the port from the message
                const coordinatorPort = port;
                if (coordinatorPort) {
                    coordinatorPort.start();

                    // Listen for messages from coordinator
                    coordinatorPort.addEventListener('message', async (msgEvent) => {
                        await handleMessage(msgEvent.data, coordinatorPort);
                    });
                } else {
                }
                return;
            }

            case 'SET_ACTIVE':
                isActive = payload.active;
                self.postMessage({ requestId, success: true });
                break;

            // TOKEN_RESPONSE handling removed - Arrow streaming uses direct fetch with cookies

            case 'RUN_SQL': {
                if (!isActive) {
                    throw new Error('This worker is not active - query should be routed to active tab');
                }

                // Send immediate ACK to confirm message received
                const responsePort = replyPort || self;
                responsePort.postMessage({
                    type: 'ACK',
                    requestId,
                    success: true
                });

                // Execute SQL on existing table
                result = await handleRunSQL(payload);

                // Send final response
                responsePort.postMessage({
                    type: 'RESPONSE',
                    requestId,
                    success: true,
                    data: result
                });
                return; // Early return since we already sent response
            }

            case 'EXECUTE_SQL': {
                // Raw SQL execution — no cache lookup, no TTL check.
                // Used by transform pipeline stages for DDL (CREATE TABLE, DROP TABLE)
                // and queries against tables that are already known to exist.
                if (!isActive) {
                    throw new Error('This worker is not active - query should be routed to active tab');
                }

                const execResponsePort = replyPort || self;
                execResponsePort.postMessage({
                    type: 'ACK',
                    requestId,
                    success: true
                });

                result = await handleExecuteSQL(payload);

                execResponsePort.postMessage({
                    type: 'RESPONSE',
                    requestId,
                    success: true,
                    data: result
                });
                return;
            }

            case 'INVALIDATE_CACHE': {
                if (!isActive) {
                    throw new Error('This worker is not active - query should be routed to active tab');
                }

                // Send immediate ACK to confirm message received
                const responsePort = replyPort || self;
                responsePort.postMessage({
                    type: 'ACK',
                    requestId,
                    success: true
                });

                // Now process the invalidation
                result = await handleInvalidateCache(payload);

                // Send final response
                responsePort.postMessage({
                    type: 'RESPONSE',
                    requestId,
                    success: true,
                    data: result
                });
                return; // Early return since we already sent response
            }

            case 'LOAD_DATA': {
                if (!isActive) {
                    throw new Error('This worker is not active - query should be routed to active tab');
                }

                // Send immediate ACK to confirm message received
                const responsePort = replyPort || self;
                responsePort.postMessage({
                    type: 'ACK',
                    requestId,
                    success: true
                });

                // Now process the data loading
                result = await handleLoadData(payload);

                // Send final response
                responsePort.postMessage({
                    type: 'RESPONSE',
                    requestId,
                    success: true,
                    data: result
                });
                return; // Early return since we already sent response
            }

            default:
                throw new Error(`Unknown message type: ${type}`);
        }

        // Send success response (for non-query messages like SET_ACTIVE)
        const responsePort = replyPort || self;
        responsePort.postMessage({
            type: 'RESPONSE',
            requestId,
            success: true,
            data: result
        });

    } catch (error) {
        // Only log errors that aren't expected cache misses
        const isCacheMiss = error.message?.includes('not found in cache');
        if (!isCacheMiss) {
        }

        // Send error response
        const responsePort = replyPort || self;
        responsePort.postMessage({
            type: 'RESPONSE',
            requestId,
            success: false,
            error: {
                message: error.message,
                requiresReconnect: error.requiresReconnect,
                action: error.action
            }
        });
    }
}

// =============================================================================
// DUCKDB INITIALIZATION
// =============================================================================

/**
 * Check if OPFS (Origin Private File System) is available
 */
async function checkOPFSAvailable() {
    try {
        if (!navigator.storage || !navigator.storage.getDirectory) {
            return false;
        }
        await navigator.storage.getDirectory();
        return true;
    } catch (error) {
        return false;
    }
}

/**
 * Initialize DuckDB-WASM with OPFS persistence
 */
async function initializeDuckDB() {
    try {
        // Use locally bundled DuckDB files (no CDN dependency).
        const DUCKDB_WORKER_URL = new URL('/duckdb/duckdb-browser-eh.worker.js', self.location.origin).href;
        const DUCKDB_WASM_URL = new URL('/duckdb/duckdb-eh.wasm', self.location.origin).href;

        const worker_url = URL.createObjectURL(
            new Blob([`importScripts("${DUCKDB_WORKER_URL}");`], { type: 'text/javascript' })
        );
        const worker = new Worker(worker_url);
        const logger = new duckdb.VoidLogger();

        let db = new duckdb.AsyncDuckDB(logger, worker);
        await db.instantiate(DUCKDB_WASM_URL);

        const opfsAvailable = await checkOPFSAvailable();

        if (opfsAvailable) {
            const opfsConfig = {
                path: 'opfs://kyomi_cache.db',
                accessMode: duckdb.DuckDBAccessMode.READ_WRITE,
                allowUnsignedExtensions: true,
                query: { castBigIntToDouble: true, castDecimalToDouble: true }
            };

            let opfsOpened = false;
            try {
                await db.open(opfsConfig);

                // Verify the database is actually writable — a corrupt or
                // WAL-orphaned DB may open successfully but in read-only mode
                const testConn = await db.connect();
                try {
                    await testConn.query('CREATE TABLE IF NOT EXISTS __write_test (x INT)');
                    await testConn.query('DROP TABLE IF EXISTS __write_test');
                } finally {
                    await testConn.close();
                }
                opfsOpened = true;
            } catch (opfsError) {
                // Open failed or DB is read-only — wipe OPFS and start fresh.
                // We must terminate the inner worker and re-instantiate because
                // DuckDB-WASM retains read-only state across close/reopen.
                try { await db.close(); } catch { /* may not have opened */ }
                try { worker.terminate(); } catch { /* best effort */ }

                try {
                    const opfsRoot = await navigator.storage.getDirectory();
                    await opfsRoot.removeEntry('kyomi_cache.db').catch(() => {});
                    await opfsRoot.removeEntry('kyomi_cache.db.wal').catch(() => {});
                } catch { /* OPFS cleanup failed */ }

                // Re-instantiate DuckDB with a fresh inner worker
                const freshWorker = new Worker(worker_url);
                const freshDb = new duckdb.AsyncDuckDB(logger, freshWorker);
                await freshDb.instantiate(bundle.mainModule, bundle.pthreadWorker);

                try {
                    await freshDb.open(opfsConfig);
                    opfsOpened = true;
                    db = freshDb;
                } catch {
                    // OPFS still broken — fall through to in-memory
                    try { await freshDb.close(); } catch {}
                }
            }

            if (!opfsOpened) {
                await db.open({
                    path: ':memory:',
                    accessMode: duckdb.DuckDBAccessMode.READ_WRITE,
                    allowUnsignedExtensions: true,
                    query: { castBigIntToDouble: true, castDecimalToDouble: true }
                });
            }
        } else {
            await db.open({
                path: ':memory:',
                accessMode: duckdb.DuckDBAccessMode.READ_WRITE,
                allowUnsignedExtensions: true,
                query: {
                    castBigIntToDouble: true,    // Cast BigInt to Double to avoid string serialization issues
                    castDecimalToDouble: true    // Also cast Decimal types for consistency
                }
            });
        }

        // Set memory limit
        const conn = await db.connect();
        try {
            await conn.query('SET memory_limit=\'3GB\'');

            // Schema migration: cache_metadata table must have cache_key column
            // During development, old DuckDB files may have outdated schema - detect and recreate
            try {
                const checkResult = await conn.query(`
                    SELECT column_name FROM information_schema.columns
                    WHERE table_name = 'cache_metadata' AND column_name = 'cache_key'
                `);

                if (checkResult.numRows === 0) {
                    // Missing cache_key column - drop and recreate with current schema
                    await conn.query('DROP TABLE IF EXISTS cache_metadata');
                }
            } catch (error) {
                // Table doesn't exist or other error - will create new one below
            }

            // Create cache_metadata table with correct schema
            await conn.query(`
                CREATE TABLE IF NOT EXISTS cache_metadata (
                    cache_key TEXT PRIMARY KEY,
                    table_name TEXT NOT NULL,
                    sql TEXT NOT NULL,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    last_accessed TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    byte_size BIGINT DEFAULT 0,
                    row_count BIGINT DEFAULT 0
                )
            `);
        } finally {
            await conn.close();
        }

        // Load quackstats extension for time series forecasting.
        // The WASM extension is bundled with the frontend in public/duckdb-extensions/.
        try {
            const extConn = await db.connect();
            try {
                // Query DuckDB for its version and platform to construct the correct path
                const versionResult = await extConn.query("SELECT version() as ver");
                const platformResult = await extConn.query("SELECT platform FROM pragma_platform()");
                const duckdbVersion = versionResult.toArray()[0].ver;
                const platform = platformResult.toArray()[0].platform;

                // Post debug info to main thread so it shows in the regular console
                self.postMessage({ type: 'DEBUG', message: `[QuackStats] DuckDB version: ${duckdbVersion}, platform: ${platform}` });

                // Point DuckDB at our bundled extensions directory
                // (allow_unsigned_extensions is set via db.open() config above)
                const extensionRepo = `${self.location.origin}/duckdb-extensions`;
                self.postMessage({ type: 'DEBUG', message: `[QuackStats] Extension repo: ${extensionRepo}` });
                self.postMessage({ type: 'DEBUG', message: `[QuackStats] Expected URL: ${extensionRepo}/${duckdbVersion}/${platform}/quackstats.duckdb_extension.wasm` });
                await extConn.query(`SET custom_extension_repository = '${extensionRepo}'`);
                await extConn.query("INSTALL quackstats");
                await extConn.query("LOAD quackstats");
                self.postMessage({ type: 'DEBUG', message: '[QuackStats] Extension loaded successfully!' });
            } finally {
                await extConn.close();
            }
        } catch (e) {
            self.postMessage({ type: 'DEBUG', message: `[QuackStats] FAILED: ${e.message}` });
        }

        // Run stale table cleanup (fire-and-forget so init isn't blocked)
        try {
            const getConn = async () => db.connect();
            cleanupStaleTables(getConn).catch(() => {}); // fire-and-forget; errors handled internally
            setInterval(() => cleanupStaleTables(getConn), 60 * 60 * 1000); // hourly
        } catch (e) {
            console.warn('[DuckDB Cleanup] Failed to schedule cleanup (non-fatal):', e.message);
        }

        return db;

    } catch (error) {
        throw new Error(`Failed to initialize DuckDB: ${error.message}`);
    }
}

/**
 * Get DuckDB instance (singleton pattern)
 */
async function getDuckDB() {
    if (dbInstance) {
        return dbInstance;
    }

    if (initPromise) {
        return await initPromise;
    }

    initPromise = initializeDuckDB();

    try {
        dbInstance = await initPromise;
        return dbInstance;
    } catch (error) {
        initPromise = null;
        throw error;
    }
}

// =============================================================================
// ARROW RESULT CONVERSION
// =============================================================================

/**
 * Convert an Arrow query result to plain arrays for postMessage serialization.
 *
 * Returns { columns: string[], rows: any[][] } where each row is an array
 * of JS-native values (Date objects for dates/timestamps, Number for BigInt, etc.)
 */
function arrowResultToArrays(result) {
    const columns = result.schema.fields.map(f => f.name);

    // Build type map for efficient Arrow type conversion
    const columnTypes = new Map();
    result.schema.fields.forEach((field) => {
        columnTypes.set(field.name, {
            typeId: field.type.typeId,
            typeName: field.type.constructor.name
        });
    });

    const rows = [];
    for (const batch of result.batches) {
        const batchData = batch.toArray();
        for (const row of batchData) {
            const plainRow = [];
            for (const columnName of columns) {
                const value = row[columnName];
                const typeInfo = columnTypes.get(columnName);

                if (value === null || value === undefined) {
                    plainRow.push(null);
                    continue;
                }

                // typeId 8 = Date (Date32/Date64), typeId 10 = Timestamp
                if (typeInfo.typeId === 8 || typeInfo.typeId === 10) {
                    const ms = typeof value.valueOf === 'function' ? value.valueOf() : value;
                    const date = new Date(ms);
                    plainRow.push(isNaN(date.getTime()) ? value : date);
                } else if (value && typeof value.valueOf === 'function') {
                    // Convert Arrow types (Uint32Array, etc.) to JavaScript primitives
                    plainRow.push(value.valueOf());
                } else {
                    plainRow.push(value);
                }
            }
            rows.push(plainRow);
        }
    }

    return { columns, rows };
}

// =============================================================================
// EXECUTE QUERY HANDLER (Main Entry Point)
// =============================================================================

/**
 * Handle RUN_SQL request
 * Executes SQL query on an existing DuckDB table
 *
 * @param {Object} payload
 * @param {string|string[]} payload.sql - SQL statement(s) to execute. If array,
 *   all but last are setup statements (e.g., CREATE MACRO), and the final element
 *   is the query whose result is returned.
 * @param {string} payload.tableId - Cache key for the data table
 */
async function handleRunSQL(payload) {
    const { sql, tableId } = payload;

    if (!sql) {
        throw new Error('RUN_SQL requires sql field');
    }

    if (!tableId) {
        throw new Error('RUN_SQL requires tableId field');
    }

    // Normalize sql: extract setup statements and the final query
    const isMultiStatement = Array.isArray(sql);
    const setupStatements = isMultiStatement ? sql.slice(0, -1) : [];
    const finalSQL = isMultiStatement ? sql[sql.length - 1] : sql;

    if (isMultiStatement && sql.length === 0) {
        throw new Error('RUN_SQL sql array must not be empty');
    }

    // Check if table exists in cache
    const cached = await getCachedTable(tableId);
    if (!cached) {
        throw new Error(`Table ${tableId} not found in cache. Load data first using LOAD_DATA.`);
    }

    // Check if cache is expired
    // Use requested TTL if provided, otherwise fall back to stored TTL
    const cacheAge = (Date.now() - new Date(cached.created_at).getTime()) / (1000 * 60 * 60); // hours
    const requestedTTL = payload.ttlHours;
    const storedTTL = cached.ttl_hours || 24;
    // If a specific TTL was requested, use it; otherwise use stored TTL
    const ttlHours = requestedTTL !== undefined ? requestedTTL : storedTTL;

    if (cacheAge >= ttlHours) {
        // Cache expired - clean it up and throw error to force reload

        try {
            const db = await getDuckDB();
            const conn = await db.connect();
            try {
                // Drop the expired table
                await conn.query(`DROP TABLE IF EXISTS ${cached.table_name}`);

                // Remove from cache metadata
                await conn.query(`DELETE FROM cache_metadata WHERE cache_key = '${tableId.replace(/'/g, "''")}'`);

            } finally {
                await conn.close();
            }
        } catch (cleanupError) {
        }

        // Throw error to signal cache miss - middleware will fetch fresh data
        throw new Error(`Table ${tableId} cache expired (age: ${cacheAge.toFixed(2)}h >= ttl: ${ttlHours}h). Load fresh data using LOAD_DATA.`);
    }

    // Execute setup statements if present (e.g., CREATE MACRO for quackstats forecasting)
    // These run on a separate connection - macros/functions are database-scoped in DuckDB
    if (setupStatements.length > 0) {
        const db = await getDuckDB();
        const conn = await db.connect();
        try {
            for (let i = 0; i < setupStatements.length; i++) {
                try {
                    await conn.query(setupStatements[i]);
                } catch (err) {
                    throw new Error(`Setup statement ${i + 1} failed: ${err.message}`);
                }
            }
        } finally {
            await conn.close();
        }
    }

    // Execute the final query through the normal path
    const result = await executeDuckDBQuery(finalSQL, cached.table_name);
    result.refreshedAt = cached.created_at;

    return result;
}

/**
 * Execute raw SQL against DuckDB — no cache lookup, no TTL check.
 *
 * Used by transform pipeline stages for:
 * - DDL: CREATE TABLE, DROP TABLE (materializing stage output, cleanup)
 * - Queries: SELECT against tables already loaded by the middleware
 *
 * Returns { columns, rows, row_count } for SELECT statements,
 * or { columns: [], rows: [], row_count: 0 } for DDL.
 */
async function handleExecuteSQL(payload) {
    const { sql } = payload;

    if (!sql) {
        throw new Error('EXECUTE_SQL requires sql field');
    }

    const db = await getDuckDB();
    const conn = await db.connect();

    try {
        const result = await conn.query(sql);
        const { columns, rows } = arrowResultToArrays(result);
        return { columns, rows, row_count: rows.length };
    } finally {
        await conn.close();
    }
}

/**
 * Handle cache invalidation request
 * Drops the cached table and removes metadata entry
 */
async function handleInvalidateCache(payload) {
    const { cacheKey } = payload;


    try {
        const db = await getDuckDB();
        const conn = await db.connect();

        try {
            // Drop the cached table if it exists
            const tableName = getCacheTableName(cacheKey);
            await conn.query(`DROP TABLE IF EXISTS ${tableName}`);

            // Remove from cache metadata
            await conn.query(`DELETE FROM cache_metadata WHERE cache_key = '${cacheKey.replace(/'/g, "''")}'`);

            return { success: true, cacheKey };

        } finally {
            await conn.close();
        }
    } catch (error) {
        throw error;
    }
}

/**
 * Handle LOAD_DATA request
 * Loads pre-fetched data (Arrow buffer or JSON array) into DuckDB
 *
 * @param {Object} payload - Load data payload
 * @param {ArrayBuffer|Array<Object>} payload.data - Pre-fetched data
 * @param {string} payload.format - Data format: 'arrow' or 'json'
 * @param {string} payload.tableId - Table name to create (e.g., '__extract_abc123')
 * @param {number} [payload.ttl] - Cache TTL in hours (default: 24)
 * @param {string} [payload.query] - Original query for metadata (optional)
 * @returns {Promise<Object>} Result metadata: {tableId, rowCount, columns, refreshedAt}
 */
async function handleLoadData(payload) {
    const { data, format, tableId, ttl = 24, query = '', replace = false } = payload;


    if (!data) {
        throw new Error('LOAD_DATA requires data field');
    }

    if (!format || (format !== 'arrow' && format !== 'json')) {
        throw new Error('LOAD_DATA requires format field ("arrow" or "json")');
    }

    if (!tableId) {
        throw new Error('LOAD_DATA requires tableId field');
    }

    // Check if table already exists in cache (prevents race condition)
    // SKIP cache check if replace=true (force reload with fresh data)
    const existing = !replace && await getCachedTable(tableId);
    if (existing) {
        // Check if cache is still valid (not expired)
        const cacheAge = (Date.now() - new Date(existing.created_at).getTime()) / (1000 * 60 * 60); // hours
        const ttlHours = existing.ttl_hours || 24;

        if (cacheAge < ttlHours) {
            // Verify the actual table exists (not just metadata)
            try {
                const db = await getDuckDB();
                const conn = await db.connect();
                try {
                    const tableName = getCacheTableName(tableId);
                    await conn.query(`SELECT 1 FROM ${tableName} LIMIT 1`);


                    // Get actual row count and columns
                    const countResult = await conn.query(`SELECT COUNT(*) as count FROM ${tableName}`);
                    const rowCount = Number(countResult.toArray()[0].count);

                    const schemaResult = await conn.query(`
                        SELECT column_name FROM information_schema.columns
                        WHERE table_name = '${tableName.replace(/'/g, "''")}'
                        ORDER BY ordinal_position
                    `);
                    const columns = schemaResult.toArray().map(row => row.column_name);

                    await conn.close();

                    return {
                        success: true,
                        tableId,
                        rowCount,
                        columns,
                        refreshedAt: existing.created_at,
                        message: 'Table already exists in cache'
                    };
                } catch (tableError) {
                    await conn.close();
                    // Table doesn't actually exist - fall through to reload
                }
            } catch (error) {
                // Fall through to reload
            }
        }
    }

    const tableName = getCacheTableName(tableId);

    try {
        const db = await getDuckDB();
        const conn = await db.connect();

        try {
            let rowCount = 0;
            let columns = [];

            try {
                // Drop existing table if it exists
                await conn.query(`DROP TABLE IF EXISTS ${tableName}`);

                if (format === 'arrow') {
                    // ARROW FORMAT: Load Arrow IPC buffer into DuckDB

                    // Append EOS (End-of-Stream) marker if not present
                    const hasEOS = data.byteLength >= 8 &&
                        data[data.byteLength - 8] === 255 &&
                        data[data.byteLength - 7] === 255 &&
                        data[data.byteLength - 6] === 255 &&
                        data[data.byteLength - 5] === 255;

                    let completeStream;
                    if (hasEOS) {
                        completeStream = new Uint8Array(data);
                    } else {
                        const EOS = new Uint8Array([255, 255, 255, 255, 0, 0, 0, 0]);
                        completeStream = new Uint8Array(data.byteLength + EOS.length);
                        completeStream.set(new Uint8Array(data), 0);
                        completeStream.set(EOS, data.byteLength);
                    }

                    // Insert Arrow data directly into DuckDB
                    await conn.insertArrowFromIPCStream(completeStream, { name: tableName });

                // Get row count and columns
                const countResult = await conn.query(`SELECT COUNT(*) as count FROM ${tableName}`);
                rowCount = Number(countResult.toArray()[0].count);

                // Get columns from table schema
                const schemaResult = await conn.query(`
                    SELECT column_name FROM information_schema.columns
                    WHERE table_name = '${tableName.replace(/'/g, "''")}'
                    ORDER BY ordinal_position
                `);
                columns = schemaResult.toArray().map(row => row.column_name);

            } else {
                // JSON FORMAT: Load array of objects into DuckDB

                if (!Array.isArray(data)) {
                    throw new Error('JSON format data must be an array of objects');
                }

                if (data.length === 0) {
                    // Empty result set - create table using column metadata if available
                    const metaColumns = payload.columns;
                    if (!metaColumns || metaColumns.length === 0) {
                        // No column metadata available - return empty result
                        // ChartML core will render the empty state
                        return { tableId, rowCount: 0, columns: [], refreshedAt: Date.now() };
                    }

                    columns = metaColumns;
                    const columnDefs = columns.map(col => `"${col}" VARCHAR`).join(', ');
                    await conn.query(`CREATE TABLE ${tableName} (${columnDefs})`);
                    rowCount = 0;

                    // Store in cache (inline SQL - same pattern as lines 687-698)
                    const escapedCacheKey = tableId.replace(/'/g, "''");
                    const escapedTableName = tableName.replace(/'/g, "''");
                    const escapedQuery = query.replace(/'/g, "''");
                    await conn.query(`
                        INSERT OR REPLACE INTO cache_metadata
                        (cache_key, table_name, sql, created_at, last_accessed, byte_size, row_count)
                        VALUES ('${escapedCacheKey}', '${escapedTableName}', '${escapedQuery}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, 0, 0)
                    `);
                    await conn.query('CHECKPOINT');

                    return { tableId, rowCount: 0, columns, refreshedAt: Date.now() };
                }

                // Get column names from first row
                columns = Object.keys(data[0]);

                // Infer column types from first row
                const columnDefs = columns.map(col => {
                    const val = data[0][col];
                    let type = 'VARCHAR';
                    if (typeof val === 'number') {
                        type = Number.isInteger(val) ? 'HUGEINT' : 'DOUBLE';
                    } else if (typeof val === 'boolean') {
                        type = 'BOOLEAN';
                    } else if (val instanceof Date) {
                        // JavaScript Date object - store as TIMESTAMP
                        type = 'TIMESTAMP';
                    } else if (typeof val === 'string') {
                        // Try to detect date/timestamp strings
                        if (/^\d{4}-\d{2}-\d{2}$/.test(val)) {
                            type = 'DATE';
                        } else if (/^\d{4}-\d{2}-\d{2}[T\s]\d{2}:\d{2}:\d{2}/.test(val)) {
                            type = 'TIMESTAMP';
                        }
                    }
                    return `"${col}" ${type}`;
                }).join(', ');

                // Create table
                await conn.query(`CREATE TABLE ${tableName} (${columnDefs})`);

                // Insert data in batches
                const batchSize = 1000;
                for (let i = 0; i < data.length; i += batchSize) {
                    const batch = data.slice(i, Math.min(i + batchSize, data.length));

                    const values = batch.map(row =>
                        '(' + columns.map(col => {
                            const val = row[col];
                            if (val === null || val === undefined) return 'NULL';
                            if (typeof val === 'number') return val;
                            if (typeof val === 'boolean') return val ? 'TRUE' : 'FALSE';
                            if (val instanceof Date) {
                                // Convert Date to ISO string for DuckDB TIMESTAMP parsing
                                return `'${val.toISOString()}'`;
                            }
                            // Strip NULL bytes and other control characters that break SQL parsing
                            const strVal = String(val).replace(/[\x00-\x1F\x7F]/g, '');
                            return `'${strVal.replace(/'/g, "''")}'`;
                        }).join(', ') + ')'
                    ).join(', ');

                    try {
                        await conn.query(`INSERT INTO ${tableName} VALUES ${values}`);
                    } catch (insertError) {
                        // Show full SQL and raw batch data
                        const fullSQL = `INSERT INTO ${tableName} VALUES ${values}`;
                        const debugInfo = `\n\nDEBUG - Full SQL (${fullSQL.length} chars):\n${fullSQL}\n\nRaw batch:\n${JSON.stringify(batch, null, 2)}`;
                        insertError.message = insertError.message + debugInfo;
                        throw insertError;
                    }
                }

                rowCount = data.length;
            }
            } catch (createError) {
                // Handle race condition: if another concurrent request already created the table
                if (createError.message && createError.message.includes('already exists')) {

                    // Verify the table actually exists and get its info
                    const countResult = await conn.query(`SELECT COUNT(*) as count FROM ${tableName}`);
                    rowCount = Number(countResult.toArray()[0].count);

                    const schemaResult = await conn.query(`
                        SELECT column_name FROM information_schema.columns
                        WHERE table_name = '${tableName.replace(/'/g, "''")}'
                        ORDER BY ordinal_position
                    `);
                    columns = schemaResult.toArray().map(row => row.column_name);

                } else {
                    // Different error - re-throw
                    throw createError;
                }
            }

            // Get approximate byte size
            let byteSize = 0;
            try {
                const sizeResult = await conn.query(`
                    SELECT estimated_size as bytes
                    FROM duckdb_tables()
                    WHERE table_name = '${tableName.replace(/'/g, "''")}'
                `);
                byteSize = Number(sizeResult.toArray()[0]?.bytes || 0);
            } catch (error) {
                // Fallback estimate
                byteSize = rowCount * 100;
            }

            // Update cache metadata
            const escapedCacheKey = tableId.replace(/'/g, "''");
            const escapedTableName = tableName.replace(/'/g, "''");
            const escapedQuery = query.replace(/'/g, "''");

            await conn.query(`
                INSERT OR REPLACE INTO cache_metadata
                (cache_key, table_name, sql, created_at, last_accessed, byte_size, row_count)
                VALUES ('${escapedCacheKey}', '${escapedTableName}', '${escapedQuery}', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP, ${byteSize}, ${rowCount})
            `);

            // Flush to disk
            await conn.query('CHECKPOINT');

            const refreshedAt = new Date().toISOString();

            return {
                tableId: tableId,
                rowCount: rowCount,
                columns: columns,
                refreshedAt: refreshedAt
            };

        } finally {
            await conn.close();
        }

    } catch (error) {

        // Clean up partial state
        try {
            const db = await getDuckDB();
            const conn = await db.connect();
            try {
                await conn.query(`DROP TABLE IF EXISTS ${tableName}`);
                await conn.query(`DELETE FROM cache_metadata WHERE cache_key = '${tableId.replace(/'/g, "''")}'`);
            } finally {
                await conn.close();
            }
        } catch (cleanupError) {
        }

        throw error;
    }
}

// =============================================================================
// CACHE OPERATIONS (Internal Helpers)
// =============================================================================

/**
 * Generate SHA-256 hash of SQL
 */
async function hashSQL(sql) {
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
 * Get cached table metadata
 */
async function getCachedTable(cacheKey) {
    try {
        const db = await getDuckDB();
        const conn = await db.connect();

        try {
            const result = await conn.query(`
                SELECT * FROM cache_metadata WHERE cache_key = '${cacheKey.replace(/'/g, "''")}'
            `);

            if (result.numRows === 0) {
                return null;
            }

            const metadata = result.toArray()[0];
            return metadata;
        } finally {
            await conn.close();
        }
    } catch (error) {
        // Returning null will trigger a re-extract, which might not be desired if it's a temporary error
        return null;
    }
}

/**
 * Check if cache entry is expired
 */
function isExpired(cached, ttl) {
    const createdAt = new Date(cached.created_at);
    const now = new Date();
    const ageHours = (now - createdAt) / (1000 * 60 * 60);
    return ageHours > ttl;
}

/**
 * Execute DuckDB query against cached table
 * Replaces "base" with actual table name
 */
async function executeDuckDBQuery(sql, tableName, options = {}) {
    const { limit, offset } = options;

    // Replace "base" with actual table name
    let finalSQL = sql.replace(/\bbase\b/gi, tableName);

    // Apply pagination if specified
    if (limit !== undefined) {
        finalSQL += ` LIMIT ${limit}`;
        if (offset !== undefined) {
            finalSQL += ` OFFSET ${offset}`;
        }
    }

    const db = await getDuckDB();
    const conn = await db.connect();

    try {
        // Get total row count from the table (for pagination)
        const countResult = await conn.query(`SELECT COUNT(*) as total FROM ${tableName}`);
        const totalRows = Number(countResult.toArray()[0].total);

        // Execute the actual query (may be paginated)
        const result = await conn.query(finalSQL);
        const { columns, rows } = arrowResultToArrays(result);

        return { columns, rows, row_count: totalRows };

    } catch (error) {
        // Self-healing: Check if this is a "table does not exist" error with old cache_ prefix
        if (error.message && error.message.includes('does not exist') && error.message.includes('Did you mean "cache_')) {

            // Extract the old table name from the error message
            const oldTableMatch = error.message.match(/Did you mean "([^"]+)"/);
            if (oldTableMatch) {
                const oldTableName = oldTableMatch[1];

                // Drop the old table
                try {
                    await conn.query(`DROP TABLE IF EXISTS ${oldTableName}`);
                } catch (dropError) {
                }

                // Also clean up the cache_metadata entry for it
                try {
                    await conn.query(`DELETE FROM cache_metadata WHERE table_name = '${oldTableName.replace(/'/g, "''")}'`);
                } catch (cleanupError) {
                }
            }

            // Re-throw the original error so the caller knows table doesn't exist
            throw new Error('Cache table not found - data needs to be loaded first');
        }

        throw error;
    } finally {
        await conn.close();
    }
}

/**
 * Get cache table name from cache key
 */
function getCacheTableName(cacheKey) {
    // cacheKey is already in format __extract_<hash>, just sanitize any special chars
    const sanitized = cacheKey.replace(/[^a-zA-Z0-9_]/g, '_');
    return sanitized;
}

// =============================================================================
// EVENT BROADCASTING
// =============================================================================

/**
 * Send event to main thread (will be forwarded by router to all tabs)
 */
function broadcastEvent(type, payload) {
    self.postMessage({
        type: 'EVENT',
        eventType: type,
        payload
    });
}

// =============================================================================
// DEBUG EXPORTS
// =============================================================================

self.getDuckDB = () => dbInstance;
