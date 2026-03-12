// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * DuckDB Coordinator SharedWorker - Routes queries to active tab
 *
 * Implements the Notion pattern for multi-tab DuckDB coordination:
 * - Tracks which tab is "active" (has the working DuckDB instance)
 * - Routes all queries from any tab → active tab's Worker
 * - Uses Web Locks to detect tab closure and elect new active tab
 * - Broadcasts events from active tab to all tabs
 *
 * Architecture:
 * Tab 1: [Worker] ←─┐
 * Tab 2: [Worker] ←─┼─→ [This SharedWorker Router]
 * Tab 3: [Worker] ←─┘     - Picks active tab
 *                          - Routes LOAD_DATA, RUN_SQL → active tab
 */


// =============================================================================
// STATE
// =============================================================================

const connectedTabs = new Map(); // tabId -> { port, workerId, isActive, lastHeartbeat }
let activeTabId = null;
let tabIdCounter = 0;

// Track pending ACKs: requestId -> { requestingTabId, originalMessage, ackTimeout }
const pendingAcks = new Map();

// Configuration
const HEARTBEAT_INTERVAL = 5000; // Tabs send heartbeat every 5s
const HEARTBEAT_TIMEOUT = 15000; // Consider tab dead after 15s without heartbeat
const ACK_TIMEOUT = 3000; // Wait 3s for worker to acknowledge message receipt

// HEARTBEAT SYSTEM DISABLED - Relying on ACK/RESPONSE protocol only
// To re-enable: uncomment the setInterval below
// Start heartbeat monitor
// setInterval(() => {
//     checkForDeadTabs();
// }, HEARTBEAT_INTERVAL);


// =============================================================================
// SHARED WORKER LIFECYCLE
// =============================================================================

self.addEventListener('connect', (e) => {
    const port = e.ports[0];
    const tabId = `tab_${++tabIdCounter}_${Date.now()}`;


    connectedTabs.set(tabId, {
        port,
        workerId: null,
        isActive: false,
        lastHeartbeat: Date.now()
    });

    port.addEventListener('message', async (event) => {
        await handleMessage(tabId, port, event.data);
    });

    port.addEventListener('close', () => {
        handleTabDisconnect(tabId);
    });

    port.start();

    // If this is the first tab, make it active
    if (connectedTabs.size === 1) {
        electActiveTab(tabId);
    } else {
    }
});

// =============================================================================
// MESSAGE HANDLING
// =============================================================================

async function handleMessage(tabId, port, message) {
    const { type, requestId, payload } = message;


    try {
        switch (type) {
            case 'REGISTER_WORKER': {
                // Tab is telling us about its Worker (via MessagePort)
                const workerPort = payload.workerPort;
                connectedTabs.get(tabId).workerPort = workerPort;

                // Set up listener for messages from this worker
                workerPort.addEventListener('message', (event) => {
                    const { type, requestId } = event.data;

                    if (type === 'ACK') {
                        // Worker acknowledged receiving the message
                        handleWorkerAck(requestId, tabId);
                    } else if (type === 'RESPONSE') {
                        // Worker sent final response
                    } else {
                    }

                    // Broadcast all messages to all tabs (they'll filter by requestId)
                    for (const [_reqTabId, tab] of connectedTabs.entries()) {
                        tab.port.postMessage(event.data);
                    }
                });

                workerPort.start();

                // If this tab should be active, notify its worker
                if (activeTabId === tabId) {
                    notifyWorkerActive(tabId, true);
                }

                port.postMessage({ requestId, success: true });
                break;
            }

            case 'RUN_SQL': {
                // Route SQL query to active tab's worker
                const success = await routeToActiveWorker(tabId, requestId, 'RUN_SQL', payload);
                if (!success) {
                    // Routing failed, error already sent to requesting tab
                    return;
                }
                // ACK and RESPONSE will be handled by the persistent worker listener
                break;
            }

            case 'EXECUTE_SQL': {
                // Route raw SQL execution to active tab's worker
                const success = await routeToActiveWorker(tabId, requestId, 'EXECUTE_SQL', payload);
                if (!success) {
                    return;
                }
                break;
            }

            case 'INVALIDATE_CACHE': {
                // Route cache invalidation to active tab's worker
                const success = await routeToActiveWorker(tabId, requestId, 'INVALIDATE_CACHE', payload);
                if (!success) {
                    // Routing failed, error already sent to requesting tab
                    return;
                }
                // ACK and RESPONSE will be handled by the persistent worker listener
                break;
            }

            case 'LOAD_DATA': {
                // Route data loading to active tab's worker
                const success = await routeToActiveWorker(tabId, requestId, 'LOAD_DATA', payload);
                if (!success) {
                    // Routing failed, error already sent to requesting tab
                    return;
                }
                // ACK and RESPONSE will be handled by the persistent worker listener
                break;
            }

            case 'EVENT':
                // Worker sent an event, broadcast to all tabs
                broadcastToAllTabs({
                    type: payload.eventType,
                    payload: payload.payload
                });
                break;

            case 'HEARTBEAT': {
                // Tab is sending heartbeat to indicate it's still alive
                const tab = connectedTabs.get(tabId);
                if (tab) {
                    tab.lastHeartbeat = Date.now();
                }
                // Send quick ack (no need to log on every heartbeat, too noisy)
                port.postMessage({ requestId, success: true });
                break;
            }

            case 'DISCONNECT': {
                // Tab is explicitly disconnecting (closing)
                handleTabDisconnect(tabId);
                // Send ack (though tab might already be gone)
                try {
                    port.postMessage({ requestId, success: true });
                } catch (error) {
                    // Tab might already be closed, ignore
                }
                break;
            }

            default:
                throw new Error(`Unknown message type: ${type}`);
        }
    } catch (error) {
        port.postMessage({
            requestId,
            success: false,
            error: { message: error.message }
        });
    }
}

// =============================================================================
// TAB COORDINATION
// =============================================================================

function electActiveTab(tabId) {

    // Deactivate old active tab
    if (activeTabId && connectedTabs.has(activeTabId)) {
        notifyWorkerActive(activeTabId, false);
    }

    // Activate new tab
    activeTabId = tabId;
    const tab = connectedTabs.get(tabId);
    if (tab) {
        tab.isActive = true;
        notifyWorkerActive(tabId, true);
    } else {
    }
}

function notifyWorkerActive(tabId, isActive) {
    const tab = connectedTabs.get(tabId);
    if (!tab || !tab.workerPort) {
        return;
    }

    tab.workerPort.postMessage({
        type: 'SET_ACTIVE',
        requestId: `set_active_${Date.now()}`,
        payload: { active: isActive }
    });
}

function handleTabDisconnect(tabId) {
    connectedTabs.delete(tabId);

    // If the active tab disconnected, elect a new one
    if (activeTabId === tabId) {
        activeTabId = null;

        // Pick first available tab
        const firstTab = connectedTabs.keys().next().value;
        if (firstTab) {
            electActiveTab(firstTab);
        } else {
        }
        // If no tabs left, activeTabId remains null
    } else {
    }
}

function broadcastToAllTabs(message) {
    for (const [tabId, tab] of connectedTabs.entries()) {
        try {
            tab.port.postMessage(message);
        } catch (error) {
        }
    }
}

/**
 * Check for tabs that haven't sent a heartbeat recently and disconnect them
 */
function checkForDeadTabs() {
    const now = Date.now();
    const deadTabs = [];

    for (const [tabId, tab] of connectedTabs.entries()) {
        const timeSinceHeartbeat = now - tab.lastHeartbeat;
        if (timeSinceHeartbeat > HEARTBEAT_TIMEOUT) {
            deadTabs.push(tabId);
        }
    }

    // Disconnect dead tabs
    for (const tabId of deadTabs) {
        handleTabDisconnect(tabId);
    }

    // Log active tab count periodically (every 30s)
    if (now % 30000 < HEARTBEAT_INTERVAL) {
    }
}

/**
 * Route message to active worker with ACK timeout
 * Returns true if message was sent successfully, false if routing failed
 */
async function routeToActiveWorker(requestingTabId, requestId, messageType, payload) {

    if (!activeTabId) {
        sendErrorToTab(requestingTabId, requestId, 'No active tab available');
        return false;
    }

    const activeTab = connectedTabs.get(activeTabId);
    if (!activeTab || !activeTab.workerPort) {
        sendErrorToTab(requestingTabId, requestId, 'Active tab has no worker');
        return false;
    }


    // Set up ACK timeout
    const ackTimeout = setTimeout(() => {
        if (pendingAcks.has(requestId)) {
            pendingAcks.delete(requestId);

            // Mark active tab as dead
            handleTabDisconnect(activeTabId);

            // Send error to requesting tab
            sendErrorToTab(requestingTabId, requestId, 'Active worker failed to acknowledge request - tab may be unresponsive');
        }
    }, ACK_TIMEOUT);

    // Track this pending ACK
    pendingAcks.set(requestId, {
        requestingTabId,
        originalMessage: { type: messageType, requestId, payload },
        ackTimeout
    });

    // Forward to active tab's worker
    try {
        activeTab.workerPort.postMessage({
            type: messageType,
            requestId,
            payload
        });
        return true;
    } catch (error) {
        clearTimeout(ackTimeout);
        pendingAcks.delete(requestId);
        sendErrorToTab(requestingTabId, requestId, `Failed to send message: ${error.message}`);
        return false;
    }
}

/**
 * Handle ACK received from worker
 */
function handleWorkerAck(requestId, tabId) {
    const pendingAck = pendingAcks.get(requestId);
    if (!pendingAck) {
        return;
    }

    // Clear the ACK timeout
    clearTimeout(pendingAck.ackTimeout);
    pendingAcks.delete(requestId);

}

/**
 * Send error message to a specific tab
 */
function sendErrorToTab(tabId, requestId, errorMessage) {
    const tab = connectedTabs.get(tabId);
    if (!tab) {
        return;
    }

    try {
        tab.port.postMessage({
            type: 'RESPONSE',
            requestId,
            success: false,
            error: { message: errorMessage }
        });
    } catch (error) {
    }
}

// =============================================================================
// DEBUG
// =============================================================================

self.getConnectedTabs = () => connectedTabs.size;
self.getActiveTab = () => activeTabId;
