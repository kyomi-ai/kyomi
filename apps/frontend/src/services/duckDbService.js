// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * DuckDB Service - Coordinator Pattern (Notion Architecture)
 *
 * Implements multi-tab coordination:
 * - Each tab has a dedicated Worker with DuckDB
 * - SharedWorker coordinator routes queries to active tab
 * - Web Locks detect tab closure and elect new active tab
 *
 * Architecture:
 * Tab (this): [Worker] → [SharedWorker Coordinator] ← [Other tabs' Workers]
 *                                ↓
 *                         Routes to active Worker
 *
 * Fallback Mode (for browsers without SharedWorker support):
 * - Each tab runs in standalone mode with direct Worker communication
 * - No cross-tab coordination, but OPFS cache is still shared
 * - Same public API for consumers
 */

// =============================================================================
// FEATURE DETECTION
// =============================================================================

/**
 * Determine if we should use SharedWorker based on browser support and user settings
 * User can override via localStorage for testing purposes
 */
const getUserWorkerMode = () => {
    const setting = localStorage.getItem('kyomi_duckdb_worker_mode');

    if (setting === 'force_coordinator') {
        return true;
    }

    if (setting === 'force_standalone') {
        return false;
    }

    // Default: auto-detect browser support
    const browserSupport = typeof SharedWorker !== 'undefined';
    return browserSupport;
};

// Check if SharedWorker is supported (not available on Chrome Android, Samsung Internet, etc.)
const SUPPORTS_SHARED_WORKER = getUserWorkerMode();


// =============================================================================
// STATE
// =============================================================================

let dedicatedWorker = null;
let coordinatorWorker = null;
let coordinatorPort = null;
let requestIdCounter = 0;
const pendingRequests = new Map();
const eventListeners = new Map();
let heartbeatInterval = null;

// Heartbeat configuration - must match coordinator
const HEARTBEAT_INTERVAL = 5000; // Send heartbeat every 5s

// =============================================================================
// INITIALIZATION
// =============================================================================

function initializeWorkers() {
    if (dedicatedWorker) {
        return { dedicatedWorker, coordinatorPort };
    }


    // Route to appropriate initialization based on SharedWorker support
    if (SUPPORTS_SHARED_WORKER) {
        return initializeWithCoordinator();
    } else {
        return initializeStandalone();
    }
}

/**
 * Initialize with SharedWorker coordinator (multi-tab coordination)
 */
function initializeWithCoordinator() {

    // Create dedicated Worker for this tab
    try {
        dedicatedWorker = new Worker(
            new URL('../workers/duckdb-worker.js', import.meta.url),
            { type: 'module', name: 'duckdb-worker' }
        );


        // Listen for worker errors
        dedicatedWorker.addEventListener('error', (error) => {
        });

        dedicatedWorker.addEventListener('messageerror', (error) => {
        });
    } catch (error) {
        throw error;
    }

    // Handle events from dedicated worker (will forward to coordinator)
    dedicatedWorker.addEventListener('message', async (event) => {
        const { type, eventType, payload, requestId } = event.data;

        if (type === 'DEBUG') {
            console.log(event.data.message);
            return;
        }

        if (type === 'EVENT') {
            // Event from worker, send to coordinator for broadcast
            if (coordinatorPort) {
                coordinatorPort.postMessage({
                    type: 'EVENT',
                    payload: { eventType, payload }
                });
            } else {
            }
        }
    });

    // Create SharedWorker coordinator
    coordinatorWorker = new SharedWorker(
        new URL('../workers/duckdb-coordinator.js', import.meta.url),
        { type: 'module', name: 'duckdb-coordinator' }
    );


    // Listen for errors
    coordinatorWorker.addEventListener('error', (error) => {
    });

    coordinatorPort = coordinatorWorker.port;

    // Listen for port errors
    coordinatorPort.addEventListener('error', (error) => {
    });


    // Handle messages from coordinator
    coordinatorPort.addEventListener('message', (event) => {
        const { requestId, success, data, error, type, payload } = event.data;

        // Check message type
        if (type === 'ACK') {
            // Worker acknowledged receiving the message - just log it
            return; // Don't resolve yet, wait for final RESPONSE
        }

        if (type === 'RESPONSE') {
            // Final response from worker
            if (requestId && pendingRequests.has(requestId)) {
                const { resolve, reject, timeout } = pendingRequests.get(requestId);
                clearTimeout(timeout);
                pendingRequests.delete(requestId);

                if (success) {
                    resolve(data);
                } else {
                    // Only log errors that aren't expected cache misses
                    const isCacheMiss = error?.message?.includes('not found in cache');
                    if (!isCacheMiss) {
                    }
                    reject(new Error(error?.message || 'Unknown error'));
                }
            }
            return;
        }

        // Debug messages from worker - log to main console
        if (type === 'DEBUG') {
            console.log(event.data.message);
            return;
        }

        // Broadcast event from coordinator (EXTRACT_STARTED, etc.)
        if (type && !requestId) {
            handleBroadcastEvent(type, payload);
            return;
        }

        // Handle responses without type field (used by SET_ACTIVE in duckdb-worker.js:70)
        if (requestId && pendingRequests.has(requestId) && !type) {
            const { resolve, reject, timeout } = pendingRequests.get(requestId);
            clearTimeout(timeout);
            pendingRequests.delete(requestId);

            if (success) {
                resolve(data);
            } else {
                reject(new Error(error?.message || 'Unknown error'));
            }
        }
    });

    coordinatorPort.start();

    // Create a MessageChannel to pass worker to coordinator
    const channel = new MessageChannel();

    // Give one port to the dedicated worker (for coordinator to send messages)
    dedicatedWorker.postMessage({
        type: 'COORDINATOR_PORT',
        port: channel.port1
    }, [channel.port1]);

    // Register the other port with coordinator
    coordinatorPort.postMessage({
        type: 'REGISTER_WORKER',
        requestId: `register_${Date.now()}`,
        payload: { workerPort: channel.port2 }
    }, [channel.port2]);

    // HEARTBEAT SYSTEM DISABLED - Relying on ACK/RESPONSE protocol and graceful disconnect only
    // To re-enable: uncomment the startHeartbeat() call below
    // Start heartbeat to keep this tab alive in coordinator
    // startHeartbeat();

    // Set up sign-off message when tab closes
    setupTabCloseHandler();


    return { dedicatedWorker, coordinatorPort };
}

/**
 * Initialize in standalone mode (fallback for browsers without SharedWorker)
 * Each tab operates independently with direct Worker communication
 */
function initializeStandalone() {

    // Create dedicated Worker for this tab
    try {
        dedicatedWorker = new Worker(
            new URL('../workers/duckdb-worker.js', import.meta.url),
            { type: 'module', name: 'duckdb-worker' }
        );


        // Listen for worker errors
        dedicatedWorker.addEventListener('error', (error) => {
        });

        dedicatedWorker.addEventListener('messageerror', (error) => {
        });
    } catch (error) {
        throw error;
    }

    // Handle messages directly from worker (no coordinator routing)
    dedicatedWorker.addEventListener('message', async (event) => {
        const { type, eventType, payload, requestId, success, data, error } = event.data;

        // Handle different message types
        if (type === 'EVENT') {
            // Worker sent an event - broadcast to local listeners
            handleBroadcastEvent(eventType, payload);
        } else if (type === 'ACK') {
            // Worker acknowledged message receipt
            // Don't resolve yet, wait for final RESPONSE
        } else if (type === 'RESPONSE') {
            // Final response from worker
            if (requestId && pendingRequests.has(requestId)) {
                const { resolve, reject, timeout } = pendingRequests.get(requestId);
                clearTimeout(timeout);
                pendingRequests.delete(requestId);

                if (success) {
                    resolve(data);
                } else {
                    // Only log errors that aren't expected cache misses
                    const isCacheMiss = error?.message?.includes('not found in cache');
                    if (!isCacheMiss) {
                    }
                    reject(new Error(error?.message || 'Unknown error'));
                }
            }
        }
    });

    // Tell worker it's always active (no coordination needed in standalone mode)
    dedicatedWorker.postMessage({
        type: 'SET_ACTIVE',
        requestId: `set_active_${Date.now()}`,
        payload: { active: true }
    });


    return { dedicatedWorker, coordinatorPort: null };
}

/**
 * Set up handler to notify coordinator when tab is closing
 */
function setupTabCloseHandler() {
    // Use pagehide as it's more reliable than beforeunload
    window.addEventListener('pagehide', () => {

        if (coordinatorPort) {
            try {
                // Send synchronous disconnect message
                coordinatorPort.postMessage({
                    type: 'DISCONNECT',
                    requestId: `disconnect_${Date.now()}`,
                    payload: {}
                });
            } catch (error) {
            }
        }

        // Stop heartbeat
        if (heartbeatInterval) {
            clearInterval(heartbeatInterval);
            heartbeatInterval = null;
        }
    }, { capture: true });
}

/**
 * Start sending periodic heartbeats to coordinator
 */
function startHeartbeat() {
    // Clear any existing heartbeat
    if (heartbeatInterval) {
        clearInterval(heartbeatInterval);
    }


    heartbeatInterval = setInterval(() => {
        if (coordinatorPort) {
            try {
                coordinatorPort.postMessage({
                    type: 'HEARTBEAT',
                    requestId: `heartbeat_${Date.now()}`,
                    payload: {}
                });
            } catch (error) {
            }
        }
    }, HEARTBEAT_INTERVAL);
}

// =============================================================================
// MESSAGE PASSING
// =============================================================================

function sendMessage(type, payload, timeoutMs = 3600000) { // 60 minutes for large extractions
    return new Promise((resolve, reject) => {
        const { dedicatedWorker, coordinatorPort } = initializeWorkers();
        const requestId = `req_${++requestIdCounter}_${Date.now()}`;


        // Set timeout
        const timeout = setTimeout(() => {
            if (pendingRequests.has(requestId)) {
                pendingRequests.delete(requestId);
                reject(new Error(`Request ${type} timed out after ${timeoutMs}ms`));
            }
        }, timeoutMs);

        pendingRequests.set(requestId, { resolve, reject, timeout });

        // Route based on mode: coordinator (SharedWorker) or direct (standalone)
        if (SUPPORTS_SHARED_WORKER && coordinatorPort) {
            // SharedWorker mode: route through coordinator
            coordinatorPort.postMessage({ type, requestId, payload });
        } else {
            // Standalone mode: send directly to worker
            dedicatedWorker.postMessage({ type, requestId, payload });
        }
    });
}

function handleBroadcastEvent(type, payload) {

    const listeners = eventListeners.get(type);
    if (listeners) {
        for (const listener of listeners) {
            try {
                listener(payload);
            } catch (error) {
            }
        }
    }
}

// =============================================================================
// PUBLIC API
// =============================================================================

export function onEvent(eventType, listener) {

    if (!eventListeners.has(eventType)) {
        eventListeners.set(eventType, new Set());
    }
    eventListeners.get(eventType).add(listener);


    // Ensure workers are initialized
    initializeWorkers();

    return () => {
        const listeners = eventListeners.get(eventType);
        if (listeners) {
            listeners.delete(listener);
        }
    };
}


export function isInitialized() {
    // In standalone mode, coordinatorPort will be null, so only check dedicatedWorker
    return dedicatedWorker !== null;
}

export function isInMemoryMode() {
    // This would need to be communicated from the worker
    return false;
}

export function getMode() {
    // Return current mode for debugging/monitoring
    return SUPPORTS_SHARED_WORKER ? 'coordinator' : 'standalone';
}


export async function invalidateCache(cacheKey) {

    const result = await sendMessage('INVALIDATE_CACHE', { cacheKey });

    return result;
}

/**
 * Load pre-fetched data (Arrow buffer or JSON array) into DuckDB
 *
 * @param {ArrayBuffer|Array<Object>} data - Pre-fetched data
 * @param {string} format - Data format: 'arrow' or 'json'
 * @param {string} tableId - Table ID to create (e.g., '__extract_abc123')
 * @param {Object} [options] - Options
 * @param {number} [options.ttlHours] - Cache TTL in hours (default: 24)
 * @param {string} [options.query] - Original query for metadata (optional)
 * @returns {Promise<Object>} Result: {tableId, rowCount, columns, refreshedAt}
 */
export async function loadData(data, format, tableId, options = {}) {
    const result = await sendMessage('LOAD_DATA', {
        data,
        format,
        tableId,
        ttl: options.ttlHours || 24,
        query: options.query || '',
        replace: options.replace || false,  // Pass replace flag to worker
        columns: options.columns || null  // Column names for empty result sets
    });

    return result;
}

/**
 * Run SQL query on an existing DuckDB table
 *
 * @param {string|string[]} sql - SQL statement(s) to execute. If a string, executes
 *   as a single query. If an array, all but the last element are setup statements
 *   (e.g., CREATE MACRO for quackstats forecasting) executed sequentially before the
 *   final element, which is the query whose result is returned.
 * @param {string} tableId - Table ID to query (e.g., '__extract_abc123')
 * @param {Object} [options] - Options
 * @param {number} [options.ttlHours] - Expected TTL in hours (used to check cache freshness)
 * @returns {Promise<Object>} Result: {columns, rows, row_count, refreshedAt}
 */
export async function runSQL(sql, tableId, options = {}) {
    // Execute SQL directly on existing table
    const result = await sendMessage('RUN_SQL', {
        sql,
        tableId,
        ttlHours: options.ttlHours  // Pass requested TTL for freshness check
    });

    return result;
}

/**
 * Execute raw SQL against DuckDB — no cache lookup, no TTL check.
 * Used by transform pipeline stages for DDL (CREATE TABLE, DROP TABLE)
 * and queries against tables that are already known to exist.
 *
 * @param {string} sql - SQL statement to execute
 * @returns {Promise<Object>} Result: { columns, rows, row_count }
 */
export async function execute(sql) {
    return await sendMessage('EXECUTE_SQL', { sql });
}

export default {
    isInitialized,
    isInMemoryMode,
    onEvent,
    invalidateCache,
    loadData,
    runSQL,
    execute,
    getMode
};
