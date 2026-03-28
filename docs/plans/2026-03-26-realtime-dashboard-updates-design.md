# Real-time Dashboard Updates via WebSocket

**Linear:** KYO-5
**Date:** 2026-03-26
**Status:** Design approved

## Goal

When a dashboard is created, updated, or deleted — by any user, any tab, or an MCP agent — all connected workspace members see the change without refreshing.

## Scope

- Dashboard CRUD events only (create, update, delete, version restore)
- Broadcast to workspace via existing WebSocket infrastructure
- Frontend refetches on event (no patching)
- Both Leptos and React frontends

## Design

### Event Format

Uses existing `MessageType::DashboardUpdate`. The `data` field carries:

```json
{
  "action": "created" | "updated" | "deleted",
  "dashboard_id": "dash_abc123",
  "changed_by": "usr_xyz789",
  "changed_by_name": "Jason Adams"
}
```

### Backend

**1. New helper** (`crates/kyomi-auth/src/websocket/helpers.rs`):

```rust
pub async fn send_dashboard_update(
    manager: &WebSocketManager,
    workspace_id: &str,
    dashboard_id: &str,
    action: &str,           // "created", "updated", "deleted"
    changed_by: &str,       // user_id
    changed_by_name: &str,  // display name
    exclude_user_id: Option<&str>,
) {
    let msg = WebSocketMessage::new(
        MessageType::DashboardUpdate,
        serde_json::json!({
            "action": action,
            "dashboard_id": dashboard_id,
            "changed_by": changed_by,
            "changed_by_name": changed_by_name,
        }),
    );
    manager.broadcast_to_workspace(workspace_id, msg, exclude_user_id).await;
}
```

**2. Dashboard routes** (`apps/server/src/routes/dashboards.rs`):

Add `send_dashboard_update()` call after each successful mutation:

| Route | Action |
|---|---|
| `POST /` (create) | `action: "created"` |
| `PATCH /{id}` (update) | `action: "updated"` |
| `DELETE /{id}` (delete) | `action: "deleted"` |
| `POST /{id}/versions/{num}/restore` | `action: "updated"` |

Each call excludes the current user (they already see their own change).

**3. MCP path** — verify whether MCP dashboard mutations call the same route handlers or separate DB logic. If separate, add the same `send_dashboard_update()` calls there.

### Frontend (Leptos)

**1. Dashboard viewer page** — subscribe to `dashboard_update`:
- If `dashboard_id` matches current view and action is `updated` → refetch dashboard resource
- If action is `deleted` → navigate away (dashboard no longer exists)

**2. Dashboard list page** — subscribe to `dashboard_update`:
- On any `created` or `deleted` → refetch dashboard list

**3. Sidebar** — subscribe to `dashboard_update`:
- On any `created` or `deleted` → refetch sidebar dashboard list

### Frontend (React)

Same subscriptions using existing `useWebSocket().subscribe()` pattern in the equivalent components.

## What's NOT in scope

- Chart-specific events (charts are part of dashboard payloads)
- Datasource change events (some already exist)
- Patching local state (refetch is simpler and always correct)
- Visual indicators of who changed what (future enhancement)
- Conflict resolution for concurrent edits (future enhancement)

## Acceptance Criteria

- [ ] Open same dashboard in two tabs — edit title in one, other tab updates without refresh
- [ ] MCP agent updates a dashboard — user viewing it sees the change within 1-2 seconds
- [ ] Delete a dashboard — other users' lists update, viewers are navigated away
- [ ] Create a dashboard — other users' sidebar/list updates
- [ ] Works in both React and Leptos frontends

## Implementation Order

1. Add `send_dashboard_update` helper
2. Wire up dashboard route handlers (4 routes)
3. Verify MCP code path
4. Leptos frontend subscriptions (dashboard viewer, list, sidebar)
5. React frontend subscriptions (same components)
6. Manual testing: two-tab test, MCP agent test
