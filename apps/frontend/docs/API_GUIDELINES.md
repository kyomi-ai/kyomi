# API Client Usage Guidelines

## ⚠️ CRITICAL RULE: ALWAYS Use apiClient for Backend API Calls

**NEVER use raw `fetch()` or `axios.create()` for backend API calls in this application.**

### Why This Matters

The application uses a dual-token authentication system with short-lived access tokens (15 minutes) and long-lived refresh tokens (7 days). When an access token expires, the backend returns a 401 Unauthorized error.

**The apiClient handles this automatically:**
1. Detects 401 responses
2. Calls the refresh token endpoint
3. Updates the HTTPOnly cookies with new tokens
4. Retries the original request automatically
5. Queues any concurrent requests during token refresh

**If you use raw fetch():**
- 401 errors fail immediately
- Users get logged out unexpectedly
- No automatic retry logic
- Inconsistent authentication state

### Correct Usage

```javascript
// ✅ CORRECT - Use apiClient
import apiClient from '../api/apiClient.js';

async function fetchData() {
  const response = await apiClient.get('/api/v1/endpoint');
  return response.data;
}

async function postData(payload) {
  const response = await apiClient.post('/api/v1/endpoint', payload);
  return response.data;
}
```

### Incorrect Usage

```javascript
// ❌ WRONG - Raw fetch bypasses token refresh
async function fetchData() {
  const response = await fetch('/api/v1/endpoint', {
    credentials: 'include'
  });
  return await response.json();
}

// ❌ WRONG - Direct axios instance bypasses interceptors
import axios from 'axios';

async function fetchData() {
  const response = await axios.get('/api/v1/endpoint');
  return response.data;
}
```

## Available Methods

The apiClient provides standard HTTP methods:

```javascript
// GET request
const response = await apiClient.get('/api/v1/endpoint', { params: { id: 123 } });

// POST request
const response = await apiClient.post('/api/v1/endpoint', { data: 'value' });

// PUT request
const response = await apiClient.put('/api/v1/endpoint', { data: 'value' });

// PATCH request
const response = await apiClient.patch('/api/v1/endpoint', { data: 'value' });

// DELETE request
const response = await apiClient.delete('/api/v1/endpoint');
```

All methods return an axios response object with a `data` property.

## Special Cases

### Streaming Endpoints

For Server-Sent Events (SSE) or streaming endpoints, use `apiClient.sendMessageStream()` which has built-in 401 handling:

```javascript
// ✅ CORRECT - Has token refresh logic
const result = await apiClient.sendMessageStream(
  message,
  sessionId,
  onChunk,
  onComplete,
  onError,
  onStart,
  provider,
  model_name
);
```

If you need a custom streaming implementation, study `sendMessageStream()` in [apiClient.js](src/api/apiClient.js#L356) to see how it handles 401 errors.

### External APIs

If you're calling an external API (not our backend), raw `fetch()` is fine:

```javascript
// ✅ OK - External API
const response = await fetch('https://api.external-service.com/data');
```

## Error Handling

The apiClient automatically handles:
- 401 Unauthorized → Token refresh + retry
- Network errors → Proper error propagation
- Response errors → Accessible via `error.response.data`

```javascript
try {
  const response = await apiClient.get('/api/v1/endpoint');
  return response.data;
} catch (error) {
  // Axios error structure
  if (error.response) {
    // Server responded with error status
    console.error('Status:', error.response.status);
    console.error('Data:', error.response.data);
  } else if (error.request) {
    // Request made but no response
    console.error('No response received');
  } else {
    // Error setting up request
    console.error('Error:', error.message);
  }
}
```

## Checklist for Code Reviews

When reviewing new code, check:

- [ ] All backend API calls use `apiClient`
- [ ] No raw `fetch()` calls to `/api/v1/*` endpoints
- [ ] No new axios instances created with `axios.create()`
- [ ] Streaming endpoints handle 401 errors properly
- [ ] Error handling uses axios error structure

## Common Mistakes to Avoid

1. **Using fetch() for convenience** - Always import apiClient instead
2. **Creating custom axios instances** - Use the singleton apiClient
3. **Forgetting credentials** - apiClient handles this automatically
4. **Manual token management** - apiClient's interceptors handle this
5. **Ignoring 401 errors** - apiClient retries automatically

## How Token Refresh Works

The flow is invisible to your code:

```
Your Code                    apiClient                     Backend
   |                            |                             |
   |-- apiClient.get() -------->|                             |
   |                            |-- GET /api/endpoint ------->|
   |                            |<-- 401 Unauthorized --------|
   |                            |                             |
   |                            |-- POST /auth/refresh ------>|
   |                            |<-- 200 OK (new tokens) -----|
   |                            |                             |
   |                            |-- GET /api/endpoint ------->|
   |                            |    (with refreshed token)   |
   |                            |<-- 200 OK (data) -----------|
   |<-- response.data ----------|                             |
```

Your code just sees a successful response, even though a token refresh happened behind the scenes.

## Reference Implementation

See these files for correct usage:

- ✅ [ChartEditorModal.jsx](src/components/ChartEditorModal.jsx#L473) - Uses apiClient.post()
- ✅ [apiClient.js](src/api/apiClient.js) - Core implementation with interceptors
- ✅ [queryExecutor.js](src/lib/queryExecutor.js) - Proper apiClient integration

## Questions?

If you're unsure whether to use apiClient for a specific case:

1. Is this calling our backend at `/api/v1/*`? → **Use apiClient**
2. Is this an external API? → Raw fetch is OK
3. Is this a special case (WebSockets, etc.)? → Check with the team

**When in doubt, use apiClient. It's always safe for backend calls.**
