# WebSocket Implementation Guide

**CRITICAL: Read this entire document before implementing ANY WebSocket functionality in Kyomi.**

## Overview

Kyomi uses a **unified WebSocket architecture** with Redis pub/sub for multi-worker communication. All WebSocket messages MUST flow through this centralized system.

**Golden Rule**: Never create direct WebSocket connections or send messages outside the unified system.

---

## Architecture

```
Backend Code (any module)
    ↓
unified_websocket_manager.send_to_user()
    ↓
Redis Pub/Sub (channel: ws:user:{user_id})
    ↓
All workers subscribe to channel
    ↓
Each worker delivers to local WebSocket connections
```

### Why Redis Pub/Sub?

- **Multi-worker support**: Users might connect to Worker A, but events happen on Worker B
- **Horizontal scaling**: Add more workers without changing code
- **Single source of truth**: All messages flow through one channel per user
- **Guaranteed delivery**: Redis ensures messages reach all workers

---

## Core Components

### 1. UnifiedWebSocketManager (Singleton)

**Location**: `apps/backend/src/api/communication/unified_websocket.py`

**Usage**:
```python
from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

# Send a message to a user
message = WebSocketMessage(
    type=MessageType.YOUR_MESSAGE_TYPE,
    session_id="optional-session-id",  # Only if message relates to a chat session
    message_id="optional-message-id",  # Only if message relates to a specific message
    data={
        "key": "value",
        # Your message payload
    }
)
await unified_websocket_manager.send_to_user(user_id, message)
```

**CRITICAL**: Always use the singleton `unified_websocket_manager`. NEVER:
- Create your own WebSocketManager instance
- Access `app.state.websocket_manager` (doesn't exist)
- Import and call `WebSocketManager()` directly

### 2. MessageType Enum

**Location**: `apps/backend/src/api/communication/unified_websocket.py:24-47`

**All available message types**:
```python
class MessageType(Enum):
    CHAT_STREAM = "chat_stream"                          # Real-time chat chunks
    CHAT_COMPLETE = "chat_complete"                      # Chat completion
    TITLE_UPDATE = "title_update"                        # Session title updates
    SESSION_CREATED = "session_created"                  # New session created
    AGENT_THINKING = "agent_thinking"                    # Agent reasoning events
    TOKEN_USAGE_UPDATE = "token_usage_update"            # Token usage updates
    OAUTH_RECONNECT_REQUIRED = "oauth_reconnect_required" # OAuth reconnection needed
    OAUTH_CANCEL = "oauth_cancel"                        # User cancelled OAuth
    OWNERSHIP_TRANSFER_OFFERED = "ownership_transfer_offered" # Ownership transfer
    WORKSPACE_INVITATION = "workspace_invitation"        # Workspace invitation
    WORKSPACE_REMOVED = "workspace_removed"              # User removed from workspace
    DASHBOARD_UPDATE = "dashboard_update"                # Dashboard updates
    CHART_UPDATE = "chart_update"                        # Chart updates
    WATCH_ALERT = "watch_alert"                          # Watch alerts
    WATCH_STATE_UPDATE = "watch_state_update"            # Watch state changes
    SHARED_CONVERSATION_ACTIVITY = "shared_conversation_activity" # Shared conversation activity
    SHARED_CHAT_MESSAGE = "shared_chat_message"          # Shared chat message broadcast
    REQUEST_CANCELLED = "request_cancelled"              # Request cancellation confirmation
    ERROR = "error"                                      # Error notifications
    HEARTBEAT = "heartbeat"                              # Connection health check
```

**Adding a new message type**:
1. Add to the `MessageType` enum
2. Document what it's for in a comment
3. Update frontend `WebSocketContext.jsx` to handle it (if needed)

### 3. WebSocketMessage Dataclass

**Location**: `apps/backend/src/api/communication/unified_websocket.py:49-67`

**Structure**:
```python
@dataclass
class WebSocketMessage:
    type: MessageType                    # REQUIRED - Message type from enum
    session_id: Optional[str] = None     # Optional - if message relates to a chat session
    message_id: Optional[str] = None     # Optional - if message relates to a specific message
    timestamp: Optional[str] = None      # Auto-generated if not provided
    data: Optional[Dict[str, Any]] = None # Optional - message payload
```

**When to use each field**:
- `type`: ALWAYS required
- `session_id`: When message relates to a chat session (chat streams, titles, thinking)
- `message_id`: When message relates to a specific message (cancellation, errors for a message)
- `timestamp`: Auto-generated - leave as None
- `data`: Your actual message content

---

## How to Send Messages (Step-by-Step)

### Basic Pattern

```python
# 1. Import the necessary components
from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

# 2. Create a WebSocketMessage object
message = WebSocketMessage(
    type=MessageType.YOUR_TYPE,
    data={"your": "data"}
)

# 3. Send to user via the unified manager
await unified_websocket_manager.send_to_user(user_id, message)
```

### Example 1: Workspace Invitation Notification

```python
from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

# Send notification when user is invited to workspace
ws_message = WebSocketMessage(
    type=MessageType.WORKSPACE_INVITATION,
    data={
        "invitation_id": invitation.invitation_id,
        "workspace_id": workspace_id,
        "workspace_name": workspace_name,
        "invited_by_name": inviter_name,
        "role": invitation.role,
        "message": f"You have been invited by {inviter_name} to join \"{workspace_name}\""
    }
)
await unified_websocket_manager.send_to_user(invited_user.user_id, ws_message)
```

### Example 2: Error Notification

```python
from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

# Send error message to user
error_msg = WebSocketMessage(
    type=MessageType.ERROR,
    message_id=message_id,  # If error relates to a specific message
    data={
        "error": "Something went wrong",
        "details": "Additional error details here"
    }
)
await unified_websocket_manager.send_to_user(user_id, error_msg)
```

### Example 3: Watch Alert Notification

```python
from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

# Send watch alert to user
alert_message = WebSocketMessage(
    type=MessageType.WATCH_ALERT,
    data={
        "watch_id": watch.watch_id,
        "watch_name": watch.name,
        "alert_title": "Revenue dropped 20%",
        "alert_details": "...",
        "timestamp": execution.started_at.isoformat()
    }
)
await unified_websocket_manager.send_to_user(user_id, alert_message)
```

### Example 4: Chat Stream (with session and message context)

```python
from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

# Send chat chunk during streaming
chunk_message = WebSocketMessage(
    type=MessageType.CHAT_STREAM,
    session_id=session_id,      # Include session context
    message_id=message_id,       # Include message context
    data={
        "content": "Hello, ",    # The text chunk
        "role": "assistant"
    }
)
await unified_websocket_manager.send_to_user(user_id, chunk_message)
```

---

## What NOT to Do (Common Mistakes)

### ❌ WRONG: Direct WebSocket sends

```python
# NEVER do this - bypasses Redis pub/sub
await websocket.send_json({
    "type": "some_message",
    "data": {...}
})

await websocket.send_text(json.dumps({...}))
```

**Why it's wrong**: Message only reaches users connected to THIS worker. Users on other workers won't receive it.

### ❌ WRONG: Accessing app.state.websocket_manager

```python
# NEVER do this - websocket_manager is NOT in app.state
websocket_manager = app_request.app.state.websocket_manager
await websocket_manager.send_to_user(user_id, message)
```

**Why it's wrong**: `app.state.websocket_manager` doesn't exist. Use the singleton instead.

### ❌ WRONG: Passing raw dicts instead of WebSocketMessage

```python
# NEVER do this - wrong type
await unified_websocket_manager.send_to_user(
    user_id,
    {"type": "workspace_removed", "data": {...}}  # ❌ Raw dict
)
```

**Why it's wrong**: `send_to_user()` expects a `WebSocketMessage` object, not a dict. This will fail type checking.

### ❌ WRONG: Creating your own WebSocket connections

```python
# NEVER do this - creates parallel connection
@router.websocket("/my-custom-ws")
async def my_websocket(websocket: WebSocket):
    await websocket.accept()
    await websocket.send_json({...})
```

**Why it's wrong**: Creates a separate WebSocket endpoint outside the unified system. Doesn't work with Redis pub/sub.

---

## The ONE Exception: Direct Sends Inside Redis Subscriber

**ONLY ONE PLACE** in the entire codebase should have direct `websocket.send_text()` calls:

**Location**: `apps/backend/src/api/communication/unified_websocket.py:314`

```python
async def _deliver_to_local_connections(self, user_id: str, message_json: str):
    """
    Deliver a message from Redis to local WebSocket connections.
    This is the ONLY place where direct websocket.send_text() is acceptable.
    """
    if user_id in self._connections:
        for websocket in self._connections[user_id]:
            await websocket.send_text(message_json)  # ✅ OK here - receiving from Redis
```

**Why this is OK**: This method receives messages FROM the Redis subscriber and delivers them to local WebSocket connections on this worker only. This is the final delivery step.

**Rule**: If you're NOT inside `_deliver_to_local_connections()`, you MUST use `send_to_user()`.

---

## Initial Heartbeat: The Second Exception

**Location**: `apps/backend/src/api/communication/unified_websocket.py:152`

```python
# Initial heartbeat - sent directly without Redis (intentional)
# This is ONLY for connection establishment confirmation
heartbeat = WebSocketMessage(
    type=MessageType.HEARTBEAT,
    data={"status": "connected", "user_id": user_id},
)
await websocket.send_text(json.dumps(heartbeat.to_dict()))  # ✅ OK here - connection handshake
```

**Why this is OK**: When a user first connects, we send an immediate heartbeat to confirm the connection is established. This is a connection-level handshake, not a cross-worker message.

**Rule**: After this initial heartbeat, ALL subsequent messages MUST use `send_to_user()`.

---

## Adding a New Message Type Checklist

When you need to send a new type of WebSocket message, follow these steps:

### Backend

1. **Add to MessageType enum** (`unified_websocket.py:24-47`)
   ```python
   class MessageType(Enum):
       ...
       MY_NEW_TYPE = "my_new_type"  # Add your type with a descriptive comment
   ```

2. **Send the message from your code**
   ```python
   from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType

   message = WebSocketMessage(
       type=MessageType.MY_NEW_TYPE,
       data={"your": "payload"}
   )
   await unified_websocket_manager.send_to_user(user_id, message)
   ```

### Frontend

3. **Subscribe to the message type** (`apps/frontend/src/context/WebSocketContext.jsx`)
   ```javascript
   // In your component
   const { subscribe } = useWebSocket();

   useEffect(() => {
       const unsubscribe = subscribe('my_new_type', (message) => {
           console.log('Received:', message);
           // Handle the message
       });

       return () => unsubscribe();
   }, [subscribe]);
   ```

4. **Add to WebSocketContext handlers** (if global handling needed)
   - Located in `apps/frontend/src/context/WebSocketContext.jsx`
   - Add handler if the message needs global state updates or side effects

---

## Debugging WebSocket Issues

### Check Redis Pub/Sub is Working

```bash
# Connect to Redis and monitor channels
redis-cli
> PSUBSCRIBE ws:user:*

# In another terminal, trigger a WebSocket message
# You should see the message published to the channel
```

### Check Backend Logs

```bash
# Look for WebSocket-related logs
tail -f apps/backend/src/logs/backend.log | grep -i websocket

# Look for Redis publish logs
tail -f apps/backend/src/logs/backend.log | grep "Published message to Redis"
```

### Common Issues

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Message not received on other workers | Direct `websocket.send_json()` bypassing Redis | Use `unified_websocket_manager.send_to_user()` |
| `AttributeError: 'State' object has no attribute 'websocket_manager'` | Accessing `app.state.websocket_manager` | Import and use `unified_websocket_manager` singleton |
| Type error on send_to_user | Passing dict instead of WebSocketMessage | Wrap dict in `WebSocketMessage(type=..., data={...})` |
| Message not received at all | Wrong user_id, or user not connected | Check user_id matches, check user has active WebSocket connection |

---

## Testing WebSocket Messages

### Manual Testing

1. **Open browser DevTools → Network → WS tab**
2. **Trigger the action** that should send a WebSocket message
3. **Check the message appears** in the WebSocket frame

### Backend Testing

```python
# In your test file
import pytest
from unittest.mock import AsyncMock, patch

@pytest.mark.asyncio
async def test_websocket_notification():
    with patch('your_module.unified_websocket_manager') as mock_manager:
        mock_manager.send_to_user = AsyncMock()

        # Trigger your code that sends a WebSocket message
        await your_function()

        # Verify send_to_user was called correctly
        mock_manager.send_to_user.assert_called_once()
        call_args = mock_manager.send_to_user.call_args

        user_id = call_args[0][0]
        message = call_args[0][1]

        assert user_id == "expected-user-id"
        assert message.type == MessageType.YOUR_TYPE
        assert message.data["key"] == "expected_value"
```

---

## Summary: The Rules

1. ✅ **ALWAYS** use `unified_websocket_manager.send_to_user()`
2. ✅ **ALWAYS** import the singleton: `from ..communication.unified_websocket import unified_websocket_manager`
3. ✅ **ALWAYS** wrap messages in `WebSocketMessage` objects
4. ✅ **ALWAYS** use `MessageType` enum values
5. ❌ **NEVER** use `websocket.send_json()` or `websocket.send_text()` directly (except in `_deliver_to_local_connections()` and initial heartbeat)
6. ❌ **NEVER** access `app.state.websocket_manager` (doesn't exist)
7. ❌ **NEVER** create your own WebSocket endpoints
8. ❌ **NEVER** pass raw dicts to `send_to_user()` - use `WebSocketMessage`

---

## Reference Implementation

**Location**: `apps/backend/src/api/routers/workspaces.py:1585-1597`

This is a perfect example of correct WebSocket usage:

```python
# Send WebSocket notification if user exists and is logged in
try:
    invited_user = session.query(User).filter(User.email == request.email).first()
    if invited_user:
        from ..communication.unified_websocket import unified_websocket_manager, WebSocketMessage, MessageType
        ws_message = WebSocketMessage(
            type=MessageType.WORKSPACE_INVITATION,
            data={
                "invitation_id": invitation.invitation_id,
                "workspace_id": workspace_id,
                "workspace_name": workspace_name,
                "invited_by_name": inviter_name,
                "role": invitation.role,
                "message": f"You have been invited by {inviter_name} to join \"{workspace_name}\""
            }
        )
        await unified_websocket_manager.send_to_user(invited_user.user_id, ws_message)
except Exception as ws_error:
    logger.warning(f"Failed to send WebSocket notification for invitation: {ws_error}")
```

**Copy this pattern for all WebSocket implementations.**

---

## Questions?

If you're unsure about WebSocket implementation:
1. Read this document again
2. Look at reference implementations in `workspaces.py`
3. Check `unified_websocket.py` for the full implementation
4. Ask before implementing something that doesn't follow this pattern

**Remember: The unified WebSocket system exists to prevent bugs, ensure multi-worker support, and maintain a single source of truth. Follow the patterns, and WebSocket communication will just work.**
