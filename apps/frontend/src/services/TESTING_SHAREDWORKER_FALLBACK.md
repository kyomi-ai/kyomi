# Testing SharedWorker Fallback Implementation

## Overview

The DuckDB service now supports two modes:
- **Coordinator Mode** (default): Uses SharedWorker for multi-tab coordination
- **Standalone Mode** (fallback): Direct worker communication when SharedWorker is unavailable

## Browser Support

### Browsers WITH SharedWorker support:
- ✅ Chrome Desktop 4+
- ✅ Firefox Desktop 29+
- ✅ Edge 79+
- ✅ Safari Desktop 16+
- ✅ Safari iOS 16+

### Browsers WITHOUT SharedWorker support (will use fallback):
- ❌ Chrome Android
- ❌ Samsung Internet
- ❌ Android Browser
- ❌ Safari iOS < 16
- ❌ Opera Mini

## How to Test

### Test 1: Verify Coordinator Mode (default)

1. Open the app in a modern desktop browser (Chrome/Firefox/Edge)
2. Open browser console
3. Look for the log message:
   ```
   [DuckDB Service] SharedWorker support: YES
   [DuckDB Service] Initializing with SharedWorker coordinator...
   ```
4. Execute a query (any chart or SQL query)
5. Verify multi-tab coordination works:
   - Open the same page in a second tab
   - Both tabs should share the same DuckDB cache via OPFS

### Test 2: Verify Standalone Mode (fallback)

#### Option A: Test on an actual mobile device
1. Open the app on Chrome Android or iOS Safari
2. Open browser console (use remote debugging)
3. Look for the log message:
   ```
   [DuckDB Service] SharedWorker support: NO (using fallback mode)
   [DuckDB Service] Initializing in standalone mode (no SharedWorker)...
   ```
4. Execute a query - it should work normally
5. Each tab will operate independently but still share OPFS cache

#### Option B: Mock the feature on desktop
1. Open browser DevTools Console BEFORE loading the app
2. Run this code to mock SharedWorker unavailability:
   ```javascript
   // Store original SharedWorker
   window.__OriginalSharedWorker = window.SharedWorker;
   // Delete SharedWorker to simulate unsupported browser
   delete window.SharedWorker;
   ```
3. Now reload the page
4. Check console logs - should show standalone mode
5. To restore:
   ```javascript
   window.SharedWorker = window.__OriginalSharedWorker;
   ```

### Test 3: Verify API Compatibility

The public API should work identically in both modes:

```javascript
import duckDbService from './services/duckDbService.js';

// Check current mode
console.log('Mode:', duckDbService.getMode()); // 'coordinator' or 'standalone'

// Execute query (works in both modes)
const result = await duckDbService.executeQuery(
    'SELECT 1 as test',
    'SELECT * FROM base',
    { ttl: 24 }
);

console.log('Query result:', result);

// Subscribe to events (works in both modes)
const unsubscribe = duckDbService.onEvent('EXTRACT_STARTED', (payload) => {
    console.log('Extract started:', payload);
});

// Invalidate cache (works in both modes)
await duckDbService.invalidateCache('some_cache_key');
```

## Expected Behavior

### Coordinator Mode (SharedWorker supported)
- ✅ Multi-tab coordination
- ✅ One "active" tab processes queries
- ✅ Events broadcast to all tabs
- ✅ OPFS cache shared across tabs
- ✅ Efficient resource usage (one DuckDB instance)

### Standalone Mode (fallback)
- ✅ Each tab processes queries independently
- ⚠️ No cross-tab event broadcasting
- ⚠️ No "active tab" election
- ✅ OPFS cache still shared across tabs
- ✅ Full DuckDB functionality per tab
- ⚠️ Higher memory usage (one DuckDB per tab)

## Debugging

### Check current mode:
```javascript
// In browser console
import('./services/duckDbService.js').then(m => {
    console.log('Current mode:', m.getMode());
});
```

### Monitor mode selection:
Watch console logs when the page loads - the first log message will indicate which mode was selected:
```
[DuckDB Service] SharedWorker support: YES/NO (using fallback mode)
```

## Performance Notes

- **Coordinator mode**: Lower memory (single DuckDB), higher coordination overhead
- **Standalone mode**: Higher memory (DuckDB per tab), no coordination overhead
- Both modes use OPFS for persistence and caching
- OPFS handles file locking automatically, even in standalone mode

## Rollback Plan

If issues are found with the fallback implementation, you can force coordinator-only mode by changing:

```javascript
// In duckDbService.js
const SUPPORTS_SHARED_WORKER = typeof SharedWorker !== 'undefined';
// Change to:
const SUPPORTS_SHARED_WORKER = typeof SharedWorker !== 'undefined' && true; // Force check
```

To disable fallback and only support SharedWorker browsers:
```javascript
if (!SUPPORTS_SHARED_WORKER) {
    throw new Error('SharedWorker not supported - please use a modern desktop browser');
}
```
