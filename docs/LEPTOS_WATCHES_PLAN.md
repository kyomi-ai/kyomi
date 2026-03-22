# Leptos Watches Page — Implementation Plan

**Status:** Awaiting approval
**Branch:** TBD (will branch from `main`)
**Date:** 2026-03-22

## Overview

Migrate the Watches page from React to Leptos. This is Phase 3 of the Leptos migration
(per `LEPTOS_MIGRATION_DESIGN.md`). Watches is a mid-complexity page featuring watch
management, AI-assisted creation via chat, cron schedule building, a Gmail-style alert
inbox with bulk actions, execution history viewing, and real-time WebSocket updates.

**Total React code:** ~3,754 lines across 8 components + utilities.

---

## Critical Rules for ALL Tasks

Every task in this plan MUST follow these rules. No exceptions.

### Rule 1: Read the React source FIRST
Before writing ANY Leptos code, open the exact React file specified in the task.
Copy CSS classes verbatim. Match HTML structure node for node. Include ALL sections.
Do NOT approximate from memory. Do NOT simplify. Do NOT skip "minor" sections.

### Rule 2: Follow the design system
Read `docs/DESIGN_SYSTEM.md` before writing any UI component.
Use semantic color tokens only (bg-primary, text-muted-foreground, etc.).
Use standard spacing scale (p-6 for cards, px-4 py-2 for buttons, etc.).
Use the Button, Card, Input, Label, Select components — do NOT create custom styles.

### Rule 3: Check for existing crates and components
Before building anything, check if a Rust crate exists (e.g., icondata_lu for icons).
Check `crates/kyomi-ui/src/components/` for existing shared components.
Reuse — do NOT duplicate.

### Rule 4: Register server functions
Every new `#[server(prefix = "/leptos-api")]` function needs a corresponding
`register_explicit::<TypeName>()` call in `crates/kyomi-ui/src/lib.rs`.

### Rule 5: Update the router
When the Watches page is complete, update `app.rs` so `/watches` and `/watches/:view`
route to the Leptos Watches page instead of `NotImplementedPage`.

### Rule 6: Build and verify
After every change:
1. `cargo check -p kyomi-ui --features ssr` — server compiles
2. `cd crates/kyomi-ui && trunk build --public-url /leptos/` — WASM compiles
3. `touch apps/server/src/leptos_frontend.rs && cargo build -p kyomi-server` — server embeds new WASM

### Rule 7: Quality over speed
There is no deadline. It is more important to get it right than to get it done quickly.
Read the React source. Match it exactly. Review your own work before calling it done.

---

## Architecture Mapping: React → Leptos

### Component Mapping

| React Component | Leptos Component | Lines | Notes |
|----------------|-----------------|-------|-------|
| `WatchesPage.jsx` | `pages/watches/mod.rs` | 666 | Main page — alerts/watches tab views, header, create button |
| `WatchModal.jsx` | `pages/watches/watch_modal.rs` | 516 | Create/edit watch form (quick edit mode) |
| `WatchAgentSidebar.jsx` | `pages/watches/agent_sidebar.rs` | 295 | AI chat sidebar for watch creation/editing |
| `WatchPreviewCard.jsx` | `pages/watches/preview_card.rs` | 221 | Watch config preview (in chat + standalone) |
| `AlertsHistory.jsx` | `pages/watches/alerts_history.rs` | 693 | Gmail-style alert inbox with bulk actions |
| `ExecutionLogViewer.jsx` | `pages/watches/execution_log.rs` | 200 | Execution detail viewer (chat-like conversation) |
| `ExecutionSelector.jsx` | `pages/watches/execution_selector.rs` | 95 | Dropdown to switch between executions |
| `ScheduleSelector.jsx` | `pages/watches/schedule_selector.rs` | 644 | Cron schedule builder with timezone handling |
| `cronUtils.js` | `pages/watches/cron_utils.rs` | 187 | Cron parsing, building, human-readable descriptions |

### API Mapping

| React API Call | Leptos Server Function |
|---------------|----------------------|
| `GET /api/v1/watches` | `list_watches()` |
| `POST /api/v1/watches` | `create_watch()` |
| `PATCH /api/v1/watches/{id}` | `update_watch()` |
| `DELETE /api/v1/watches/{id}` | `delete_watch()` |
| `POST /api/v1/watches/{id}/toggle` | `toggle_watch()` |
| `POST /api/v1/watches/{id}/run` | `run_watch_now()` |
| `GET /api/v1/watches/{id}/executions` | `list_watch_executions()` |
| `GET /api/v1/watches/{id}/executions/{eid}` | `get_watch_execution()` |
| `GET /api/v1/watches/{id}/executions/{eid}/thinking-events` | `get_thinking_events()` |
| `GET /api/v1/watches/alerts` | `get_alerts_history()` |
| `GET /api/v1/watches/alerts/count` | `get_unread_alerts_count()` |
| `POST /api/v1/watches/alerts/{id}/read` | `mark_alert_read()` |
| `POST /api/v1/watches/alerts/{id}/unread` | `mark_alert_unread()` |
| `POST /api/v1/watches/alerts/{id}/delete` | `delete_alert()` |
| `POST /api/v1/watches/alerts/{id}/restore` | `restore_alert()` |
| `POST /api/v1/watches/alerts/bulk-read` | `bulk_mark_alerts_read()` |
| `POST /api/v1/watches/alerts/bulk-unread` | `bulk_mark_alerts_unread()` |
| `POST /api/v1/watches/alerts/bulk-delete` | `bulk_delete_alerts()` |
| `POST /api/v1/watches/alerts/{id}/continue-chat` | `continue_alert_in_chat()` |

---

## Phase 1: Types, Cron Utilities & Server Functions

Build the data layer — types, utilities, and all server functions. No UI yet.

### Task 1.1: Watch types
**Creates:** `crates/kyomi-ui/src/pages/watches/types.rs`
**React reference:** Infer from `WatchesPage.jsx` state + API responses,
  `apps/server/src/routes/watches.rs` (request/response types),
  `crates/kyomi-core/src/models/watch.rs` (data model)

Define all shared types:

```rust
// Watch
pub struct WatchSummary {
    pub watch_id: String,
    pub name: String,
    pub prompt: String,
    pub schedule: String,
    pub mode: WatchMode,
    pub enabled: bool,
    pub last_run_at: Option<String>,
    pub last_run_status: Option<String>,
    pub next_run_at: Option<String>,
    pub alert_emails: Option<String>,
    pub alert_emails_enabled: bool,
    pub datasource_hints: Option<serde_json::Value>,
    pub queries: Option<Vec<ReferenceQuery>>,
    pub slack_channel_id: Option<String>,
    pub slack_channel_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

pub enum WatchMode { Alert, Report }

pub struct ReferenceQuery {
    pub comment: Option<String>,
    pub sql: String,
    pub datasource: String,
}

// Execution
pub struct WatchExecution {
    pub id: i64,
    pub watch_id: Option<String>,
    pub watch_name: String,
    pub mode: String,
    pub session_id: Option<String>,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: WatchExecutionStatus,
    pub agent_response: Option<String>,
    pub error_message: Option<String>,
    pub alert_triggered: bool,
    pub input_tokens: Option<i64>,
    pub output_tokens: Option<i64>,
    pub read_at: Option<String>,
    pub deleted_at: Option<String>,
}

pub enum WatchExecutionStatus { Running, Success, Error, NoAlert }

// Execution with full trace (for detail view)
pub struct WatchExecutionDetail {
    pub execution: WatchExecution,
    pub execution_trace: Option<serde_json::Value>,
}

// Thinking events
pub struct ThinkingEvent {
    pub event_type: String,
    pub content: String,
    pub timestamp: Option<String>,
}

// Alert filters
pub struct AlertFilters {
    pub watch_id: Option<String>,
    pub include_deleted: bool,
    pub limit: u32,
    pub offset: u32,
}

// Alert list response
pub struct AlertsPage {
    pub alerts: Vec<WatchExecution>,
    pub total: u64,
    pub has_more: bool,
}
```

All types must be `Clone + Serialize + Deserialize`.

**Acceptance criteria:**
- All types compile with both `ssr` and `hydrate` features
- Types match the REST API response shapes exactly

### Task 1.2: Cron utilities (pure Rust)
**Creates:** `crates/kyomi-ui/src/pages/watches/cron_utils.rs`
**React reference:** `apps/frontend/src/utils/cronUtils.js` (187 lines)
**Read the entire React file.**

Port the cron utility functions to pure Rust:

```rust
/// Build a 5-field cron expression from UI selections.
/// Handles timezone offset (local → UTC conversion for hour/day fields).
pub fn build_cron(schedule_type: &str, selections: &CronSelections) -> String

/// Parse a cron expression into UI-friendly selections.
/// Handles UTC → local timezone conversion.
pub fn parse_cron_to_selections(cron: &str, tz_offset_hours: i32) -> Option<CronSelections>

/// Generate a human-readable description of a cron expression.
/// e.g., "Daily at 9:00 AM" or "Every Monday and Wednesday at 2:30 PM"
pub fn describe_cron(cron: &str) -> String

pub struct CronSelections {
    pub schedule_type: String,  // "hourly", "daily", "weekly", "monthly"
    pub minute: u8,
    pub hour: u8,               // local time
    pub days_of_week: Vec<u8>,  // 0=Sun, 1=Mon, ..., 6=Sat
    pub day_of_month: u8,
}
```

**Critical timezone handling** (from React cronUtils.js):
- Cron is stored in UTC
- UI shows local time
- When building cron from UI: convert local hour to UTC, handle day-of-week
  offset when hour conversion crosses midnight (e.g., 11 PM local = next day UTC)
- When parsing cron for UI: reverse the conversion

The timezone offset comes from the browser's `Date.getTimezoneOffset()` via `js_sys`.

**Acceptance criteria:**
- `build_cron` → `parse_cron_to_selections` round-trips correctly for all schedule types
- Timezone conversion handles midnight crossing (day offset)
- `describe_cron` produces readable descriptions matching React output
- Works in both WASM and SSR (SSR can skip timezone, use UTC)

### Task 1.3: Watch CRUD server functions
**Creates:** `crates/kyomi-ui/src/server_fns/watches.rs`
**React reference:** `apps/frontend/src/pages/WatchesPage.jsx` (API calls),
  `apps/server/src/routes/watches.rs` (route handlers — read the full file)
**Read the watches route file to understand the service layer calls.**

```rust
#[server(prefix = "/leptos-api")]
pub async fn list_watches() -> Result<Vec<WatchSummary>, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn create_watch(
    name: String,
    prompt: String,
    schedule: String,
    mode: String,
    datasource_hints: Option<serde_json::Value>,
    queries: Option<Vec<ReferenceQuery>>,
    slack_channel_id: Option<String>,
    slack_channel_name: Option<String>,
) -> Result<WatchSummary, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn update_watch(
    watch_id: String,
    name: Option<String>,
    prompt: Option<String>,
    schedule: Option<String>,
    mode: Option<String>,
    datasource_hints: Option<serde_json::Value>,
    queries: Option<Vec<ReferenceQuery>>,
    alert_emails: Option<String>,
    alert_emails_enabled: Option<bool>,
    slack_channel_id: Option<String>,
    slack_channel_name: Option<String>,
) -> Result<WatchSummary, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn delete_watch(watch_id: String) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn toggle_watch(watch_id: String, enabled: bool) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn run_watch_now(watch_id: String) -> Result<(), ServerFnError>
```

These call the same `watch_service` functions used by the REST API routes.
Include capability checking (Pro/Team tier required).

**Acceptance criteria:**
- All 6 server functions compile and are registered
- Create validates name length (3-255), prompt length (≥10), schedule (valid cron)
- Update uses PATCH semantics (only provided fields are changed)
- Toggle/run include rate limiting checks (run: max 5/hour)
- Capability gating works (free tier blocked)

### Task 1.4: Execution & thinking events server functions
**Creates:** Add to `crates/kyomi-ui/src/server_fns/watches.rs`
**React reference:** `apps/frontend/src/components/watches/ExecutionLogViewer.jsx` (API calls),
  `apps/server/src/routes/watches.rs` (execution endpoints)

```rust
#[server(prefix = "/leptos-api")]
pub async fn list_watch_executions(
    watch_id: String,
    limit: Option<u32>,
) -> Result<Vec<WatchExecution>, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn get_watch_execution(
    watch_id: String,
    execution_id: i64,
) -> Result<WatchExecutionDetail, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn get_thinking_events(
    watch_id: String,
    execution_id: i64,
) -> Result<Vec<ThinkingEvent>, ServerFnError>
```

The thinking events endpoint extracts events from the chat session messages first,
falling back to `execution_trace.events` for older executions.

**Acceptance criteria:**
- Execution list returns most recent first, capped at limit (default 100)
- Execution detail includes full trace
- Thinking events extracted correctly from session or trace fallback
- All server functions registered

### Task 1.5: Alert management server functions
**Creates:** Add to `crates/kyomi-ui/src/server_fns/watches.rs`
**React reference:** `apps/frontend/src/components/watches/AlertsHistory.jsx` (API calls),
  `apps/server/src/routes/watches.rs` (alert endpoints)

```rust
#[server(prefix = "/leptos-api")]
pub async fn get_alerts_history(
    watch_id: Option<String>,
    include_deleted: bool,
    limit: u32,
    offset: u32,
) -> Result<AlertsPage, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn get_unread_alerts_count() -> Result<u64, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn mark_alert_read(execution_id: i64) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn mark_alert_unread(execution_id: i64) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn delete_alert(execution_id: i64) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn restore_alert(execution_id: i64) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_alerts_read(execution_ids: Vec<i64>) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn bulk_mark_alerts_unread(execution_ids: Vec<i64>) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn bulk_delete_alerts(execution_ids: Vec<i64>) -> Result<(), ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn continue_alert_in_chat(execution_id: i64) -> Result<String, ServerFnError>
// Returns: session_id for the new chat session
```

Bulk operations enforce max 100 items per request.

**Acceptance criteria:**
- Alert history supports filtering by watch_id, deleted inclusion, pagination
- Unread count returns correct number for the workspace
- All individual and bulk operations work correctly
- Soft-delete sets deleted_at/deleted_by, restore clears them
- Continue-in-chat creates a new session with alert context and returns session_id

### Task 1.6: Supporting server functions (Slack, datasources)
**Creates:** Add to `crates/kyomi-ui/src/server_fns/watches.rs` or reuse existing
**React reference:** `apps/frontend/src/components/watches/WatchModal.jsx` (Slack + datasource calls)

Check what already exists:
- `list_datasources()` — already in `server_fns/datasources.rs` ✓
- Slack status/channels — may already be in `server_fns/slack.rs`

If not already available, add:
```rust
#[server(prefix = "/leptos-api")]
pub async fn get_slack_channels_for_watch() -> Result<Vec<SlackChannel>, ServerFnError>

#[server(prefix = "/leptos-api")]
pub async fn get_workspace_capabilities() -> Result<HashMap<String, bool>, ServerFnError>
```

These are needed by the WatchModal for:
- Showing Slack channel selector (if Slack is connected)
- Checking capabilities: `kyomi_watch`, `watch_slack_alerts`, `watch_email_alerts`

**Acceptance criteria:**
- Slack channels list works when Slack is connected
- Returns empty list gracefully when Slack is not connected
- Capability check returns correct flags for the workspace tier

---

## Phase 2: Schedule Selector & Cron UI

The schedule selector is a self-contained, complex component used by the
WatchModal. Build it first as an independent unit.

### Task 2.1: Schedule selector component
**Creates:** `crates/kyomi-ui/src/pages/watches/schedule_selector.rs`
**React reference:** `apps/frontend/src/components/watches/ScheduleSelector.jsx` (644 lines)
**Read the ENTIRE React file — this is the most complex component.**

Two modes:
1. **UI mode** (default) — friendly dropdowns and checkboxes
2. **Raw cron mode** — direct cron expression input

UI mode schedule types:
- **Hourly**: minute selector (0-59 in 5-min increments)
- **Daily**: hour selector (12h format with AM/PM) + minute selector
- **Weekly**: day-of-week checkboxes (Mon-Sun) + hour + minute
- **Monthly**: day-of-month selector (1-31) + hour + minute

Component props:
```rust
#[component]
pub fn ScheduleSelector(
    value: Signal<String>,                    // Current cron expression
    on_change: Callback<String>,              // Called with new cron expression
    #[prop(optional)] is_raw: Signal<bool>,   // Raw cron mode toggle
) -> impl IntoView
```

Key behaviors:
- On mount: parse existing cron into UI selections via `parse_cron_to_selections()`
- On UI change: build cron via `build_cron()`, call `on_change`
- Timezone display: show "(your local time)" label, convert UTC↔local
- Raw mode: direct text input with validation feedback
- Invalid cron in raw mode: show red border + error message
- Schedule type change: reset to sensible defaults for new type

**Acceptance criteria:**
- All 4 schedule types render correct controls
- Day-of-week checkboxes work for weekly schedule
- 12-hour time format with AM/PM selector
- Timezone conversion: displayed time is local, stored cron is UTC
- Raw cron mode allows direct input with validation
- Mode switching preserves the cron expression where possible
- Matches React styling exactly (read the JSX, copy classes verbatim)

---

## Phase 3: Watch Management UI

### Task 3.1: Watch card grid
**Creates:** `crates/kyomi-ui/src/pages/watches/watch_card.rs`
**React reference:** `apps/frontend/src/pages/WatchesPage.jsx` (watch cards section, ~lines 300-500)
**Read the watch card rendering in WatchesPage.jsx.**

Individual watch card showing:
- Watch name (title)
- Mode badge: "Alert" (amber) or "Report" (blue)
- Enabled/disabled toggle switch
- Schedule description (human-readable, from `describe_cron()`)
- Next run time (relative: "in 2 hours")
- Last run status badge: success (green), error (red), no_alert (gray), running (blue spinner)
- Prompt text (truncated, expandable)
- Action buttons: Edit, Run Now, Delete
- Reference queries count (if any)
- Notification indicators: Slack channel name, email count

Card layout: grid of cards, responsive (1 col mobile, 2 col tablet, 3 col desktop).

**Acceptance criteria:**
- All card fields render correctly
- Toggle switch enables/disables watch (optimistic update)
- Run Now button triggers immediate execution
- Delete button shows confirmation dialog
- Status badges use correct colors
- Schedule shows local timezone
- Matches React styling exactly

### Task 3.2: Watch modal (quick edit)
**Creates:** `crates/kyomi-ui/src/pages/watches/watch_modal.rs`
**React reference:** `apps/frontend/src/components/watches/WatchModal.jsx` (516 lines)
**Read the entire React file.**

Modal for editing existing watches (not AI-assisted creation):

Fields:
- **Name** — text input, 3-255 chars
- **Mode** — select: Alert / Report
- **Prompt** — textarea, min 10 chars, multiline
- **Schedule** — embedded `ScheduleSelector` component (from Task 2.1)
- **Reference Queries** — expandable section:
  - List of { comment, SQL, datasource } items
  - Add/remove query buttons
  - Datasource selector per query (from `list_datasources()`)
  - SQL textarea per query
- **Slack Notifications** — channel selector (feature-gated: `watch_slack_alerts`)
- **Email Notifications** — email input + enable toggle (feature-gated: `watch_email_alerts`)

Modal actions:
- Save button → calls `update_watch()` server function
- Cancel button → closes modal
- Validation errors shown inline

**Acceptance criteria:**
- All fields render and bind correctly
- Validation: name length, prompt length, valid cron
- Reference queries: add/remove works, datasource selector populates
- Slack channel selector shows when Slack is connected + capability enabled
- Email field shows when email capability enabled
- Save calls update_watch() with only changed fields
- Loading state on save button
- Error display for server-side validation failures
- Matches React modal layout exactly

### Task 3.3: Watch preview card
**Creates:** `crates/kyomi-ui/src/pages/watches/preview_card.rs`
**React reference:** `apps/frontend/src/components/watches/WatchPreviewCard.jsx` (221 lines)
**Read the entire React file.**

Rendered inline in chat messages when the AI agent proposes a watch configuration.
Shows a card with:
- Watch name, mode, schedule (human-readable)
- Prompt preview (truncated)
- Reference queries (if any)
- Slack channel (if configured)
- **Approve / Edit / Reject** buttons (when in chat context)
- **Standalone mode** (no action buttons, just display)

Two rendering modes:
1. **Chat mode** — shown in WatchAgentSidebar chat, has approval buttons
2. **Standalone mode** — shown in execution log, display only

The chat mode buttons trigger callbacks that:
- Approve → calls `create_watch()` or `update_watch()`
- Edit → opens WatchModal pre-filled with proposed config
- Reject → dismisses the card

**Acceptance criteria:**
- Card renders all watch fields correctly
- Approve/Edit/Reject buttons work in chat mode
- Standalone mode shows no buttons
- Markdown in prompt rendered via MarkdownRenderer (already exists)
- Matches React card styling exactly

---

## Phase 4: Watch Agent Sidebar (AI Chat)

### Task 4.1: Watch agent sidebar
**Creates:** `crates/kyomi-ui/src/pages/watches/agent_sidebar.rs`
**React reference:** `apps/frontend/src/components/watches/WatchAgentSidebar.jsx` (295 lines)
  AND `apps/frontend/src/components/ChatInterface.jsx` (635 lines — shared chat component)
**Read BOTH React files.**

The sidebar is used for AI-assisted watch creation and editing. It reuses the
shared ChatInterface component (which must already exist for the Chat page migration).

**Important dependency:** This task requires the shared ChatInterface/chat components
to be available. If the Chat page migration hasn't built them yet, this task must
either build a minimal version or wait for Chat Phase 1.

Sidebar features:
- Resizable width on desktop (drag handle, same pattern as dashboard copilot sidebar)
- Slide-in panel on mobile (<768px)
- Chat interface for conversation with watch agent
- Detects `json:watch-response` code blocks in agent messages
- Renders WatchPreviewCard inline for watch proposals
- Tracks approved/rejected cards via `acceptedCardIds` set
- On approve: creates the watch, shows success toast
- On edit: opens WatchModal with proposed config
- Close button dismisses sidebar

Chat context:
- Session type: the agent knows it's in "watch creation" mode
- Agent has access to watch tools (create, search, update, etc.)
- Messages stream via WebSocket (same as regular chat)

**Acceptance criteria:**
- Sidebar opens/closes with animation
- Resizable on desktop (320-600px, same as dashboard copilot)
- Chat messages render correctly
- Watch proposals detected and rendered as WatchPreviewCard
- Approve creates the watch and refreshes the list
- Edit opens modal pre-filled
- Reject dismisses the card
- Multiple proposals in one conversation handled correctly
- Close button works, keyboard Escape works

---

## Phase 5: Alerts History (Gmail-Style Inbox)

### Task 5.1: Alerts history — list and filters
**Creates:** `crates/kyomi-ui/src/pages/watches/alerts_history.rs`
**React reference:** `apps/frontend/src/components/watches/AlertsHistory.jsx` (693 lines)
**Read the ENTIRE React file — this is the largest component.**

Gmail-style alert inbox with:

**Filter bar:**
- Watch filter dropdown (filter by specific watch, or "All watches")
- Status filter: All / Unread / Read
- Include deleted toggle
- Pagination: page size selector + page navigation

**Alert list:**
- Each alert row shows:
  - Checkbox (for bulk selection)
  - Unread indicator (blue left border + bold text)
  - Watch name
  - Alert title (or "No alert" for no_alert status)
  - Timestamp (relative: "2 hours ago")
  - Status badge (success/error/no_alert)
  - Expand/collapse arrow
- Expanded alert shows:
  - Full agent response (rendered via MarkdownRenderer, including ChartML charts)
  - Action buttons: Mark read/unread, Delete, Restore (if deleted), Continue in Chat

**Toolbar (appears when alerts are selected):**
- Selection count
- Mark Read / Mark Unread buttons
- Delete button
- Select All / Deselect All

**Acceptance criteria:**
- Filter by watch, status, deleted works correctly
- Pagination loads correct page
- Checkbox selection + bulk operations work
- Expand/collapse shows full alert content
- MarkdownRenderer renders charts in alert responses
- Read/unread visual distinction (blue border, bold)
- Toolbar appears/disappears based on selection
- Optimistic updates for read/unread/delete actions
- Empty state: "No alerts yet" message
- Matches React styling exactly

### Task 5.2: Alert expand/collapse and actions
**Creates:** Inline in `alerts_history.rs` or separate `alert_row.rs`
**React reference:** Same AlertsHistory.jsx (expanded alert section)

When an alert row is expanded:
- Full `agent_response` markdown rendered (with charts via MarkdownRenderer)
- **Mark Read** / **Mark Unread** button (toggles)
- **Delete** button (soft-deletes, shows toast with undo)
- **Restore** button (only visible for deleted alerts)
- **Continue in Chat** button → calls `continue_alert_in_chat()`,
  navigates to `/chat/{session_id}` with the alert context

Auto-mark-as-read: when an alert is expanded, it's automatically marked read
after a short delay (500ms), matching the React behavior.

**Acceptance criteria:**
- Expanded content renders full markdown including embedded charts
- All action buttons work correctly
- Auto-mark-read fires on expand
- Continue in Chat navigates to new chat session
- Delete shows undo toast
- Restore works for soft-deleted alerts

---

## Phase 6: Execution History

### Task 6.1: Execution selector dropdown
**Creates:** `crates/kyomi-ui/src/pages/watches/execution_selector.rs`
**React reference:** `apps/frontend/src/components/watches/ExecutionSelector.jsx` (95 lines)
**Read the entire React file.**

Dropdown that shows recent executions for a watch:
- Lists executions by date (newest first)
- Each item shows: timestamp, status badge, duration
- Selected execution highlighted
- On select: loads the execution detail

**Acceptance criteria:**
- Dropdown lists executions in reverse chronological order
- Status badges render correctly (success/error/no_alert/running)
- Selection triggers detail load
- Matches React styling

### Task 6.2: Execution log viewer
**Creates:** `crates/kyomi-ui/src/pages/watches/execution_log.rs`
**React reference:** `apps/frontend/src/components/watches/ExecutionLogViewer.jsx` (200 lines)
**Read the entire React file.**

Shows a single execution as a chat-like conversation:

1. **User message bubble**: The watch prompt (what was asked)
2. **Thinking section** (expandable): Agent's thinking events from the execution
   - Lazy-loaded via `get_thinking_events()` server function
   - Collapsible "View agent thinking" toggle
   - Each event shown with type label + content
3. **Assistant message bubble**: The agent's response
   - Rendered via MarkdownRenderer (includes charts if present)
   - If `alert_triggered`: shows the alert card with title
   - If `no_alert`: shows "No alert triggered" with reason

Header shows:
- Execution timestamp
- Duration (completed_at - started_at)
- Status badge
- Token usage (input + output tokens)
- Execution selector dropdown (to switch between runs)

The entire component is displayed in a modal, opened from:
- Watch card "History" button
- Alert row "View execution" action

**Acceptance criteria:**
- Chat-style layout with user/assistant bubbles
- Thinking events lazy-load and expand/collapse
- MarkdownRenderer renders charts in agent response
- Header shows all metadata
- Execution selector switches between runs
- Modal opens/closes correctly
- Loading skeleton while execution detail fetches
- Error state if execution not found

---

## Phase 7: Page Assembly & Integration

### Task 7.1: Watches page shell
**Creates:** `crates/kyomi-ui/src/pages/watches/mod.rs`
**React reference:** `apps/frontend/src/pages/WatchesPage.jsx` (666 lines)
**Read the entire React file.**

Assemble the full page:

```
┌──────────────────────────────────────────────────────────┐
│  Header: "Watches" title                                  │
│  Tab bar: [Alerts] [Watches]          [+ Create Watch]    │
├──────────────────────────────────────────────────────────┤
│                                                           │
│  Alerts view (default):                                   │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ AlertsHistory component (Gmail-style inbox)         │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                           │
│  OR                                                       │
│                                                           │
│  Watches view:                                            │
│  ┌─────────────────────────────────────────────────────┐  │
│  │ Watch card grid                                     │  │
│  │ ┌──────────┐ ┌──────────┐ ┌──────────┐             │  │
│  │ │ Watch 1  │ │ Watch 2  │ │ Watch 3  │             │  │
│  │ └──────────┘ └──────────┘ └──────────┘             │  │
│  └─────────────────────────────────────────────────────┘  │
│                                                           │
│  WatchAgentSidebar (slides in from right when creating)   │
│  WatchModal (opens for editing)                           │
│  ExecutionLogModal (opens for history viewing)            │
│  ConfirmDialog (for deletions)                            │
└──────────────────────────────────────────────────────────┘
```

Routing:
- `/watches` → defaults to alerts view
- `/watches/alerts` → alerts view
- `/watches/config` → watches list view

The view is controlled by the URL path param. Match the React routing exactly.

State management:
- Watch list fetched via `list_watches()` Resource
- Alerts fetched via `get_alerts_history()` Resource
- WebSocket `watch_state_update` event invalidates both resources
- Modal/sidebar visibility as local signals
- Editing watch tracked as `Option<WatchSummary>` signal

**"Create Watch" button:**
- Opens the WatchAgentSidebar
- After successful creation, refreshes watch list, shows success toast

**Acceptance criteria:**
- Tab switching between Alerts and Watches views works
- URL updates on tab change (`/watches/alerts`, `/watches/config`)
- Create button opens agent sidebar
- Watch cards render in responsive grid
- AlertsHistory renders with full functionality
- WebSocket events trigger data refresh
- Modals open/close correctly for edit, history, delete confirmation
- Empty states for no watches and no alerts
- Feature gating: watches disabled on free tier shows upgrade prompt

### Task 7.2: Route registration
**Modifies:** `crates/kyomi-ui/src/app.rs`

Update the router:
```rust
// Replace NotImplementedPage with WatchesPage
<Route path=path!("/watches") view=|| view! { <Layout><WatchesPage/></Layout> }/>
<Route path=path!("/watches/:view") view=|| view! { <Layout><WatchesPage/></Layout> }/>
```

**Acceptance criteria:**
- `/watches` renders the Leptos Watches page
- `/watches/alerts` and `/watches/config` both work
- Navigation from sidebar works

### Task 7.3: WebSocket integration for real-time updates
**Modifies:** `crates/kyomi-ui/src/utils/websocket.rs` (extend existing)
**React reference:** `apps/frontend/src/context/WebSocketContext.jsx` (watch_state_update event)

Extend the existing WebSocket hook to handle `watch_state_update` events:
- When received: invalidate/refetch watch list and alerts resources
- Also handle `WatchAlert` message type for toast notifications
  (new alert received → show toast with watch name and alert title)

**Acceptance criteria:**
- Watch state changes trigger list refresh
- New alert toast shows watch name + title
- Toast has "View" action that navigates to the alert

### Task 7.4: Unread badge in sidebar
**Modifies:** `crates/kyomi-ui/src/components/layout.rs` (sidebar)
**React reference:** Check how React sidebar shows unread alerts count

The sidebar "Watches" navigation item should show an unread badge:
- Fetch unread count via `get_unread_alerts_count()` on mount
- Show red/blue badge with count next to "Watches" in sidebar
- Update count when WebSocket `watch_state_update` event arrives
- Hide badge when count is 0

**Acceptance criteria:**
- Badge shows correct unread count
- Badge updates in real-time via WebSocket
- Badge hidden when count is 0
- Matches React sidebar badge styling

---

## Phase 8: Polish & Parity

### Task 8.1: Mobile responsive layout
**Modifies:** Various files
**React reference:** `apps/frontend/src/pages/WatchesPage.jsx` (mobile rendering)

Mobile (<768px) adaptations:
- Watch cards: single column
- Agent sidebar: full-width slide-in overlay
- Alert rows: condensed layout (stack metadata vertically)
- Tab bar: scrollable if needed
- Execution log modal: full-screen on mobile

**Acceptance criteria:**
- All features accessible on mobile
- No horizontal overflow
- Sidebar overlays content properly
- Touch targets adequate size (44px minimum)

### Task 8.2: Capability gating and empty states
**Modifies:** Various files
**React reference:** `apps/frontend/src/pages/WatchesPage.jsx` (capability checks, empty states)

Handle all capability and empty states:
- **Free tier**: Show upgrade prompt instead of watch functionality
  - "Watches require a Pro or Team plan" message
  - Upgrade button linking to billing settings
- **No watches yet**: Show empty state with illustration + "Create your first watch" CTA
- **No alerts yet**: Show empty state message
- **Watch limit reached**: Show message in create flow ("You've reached your 10-watch limit")
- **Slack not connected**: Hide Slack option in modal, or show "Connect Slack" link
- **Email not configured**: Hide email option (self-hosted without SMTP)

**Acceptance criteria:**
- Each capability gate shows appropriate message
- Empty states render with helpful CTAs
- Upgrade prompts link to correct settings page
- Feature-gated UI elements hidden when capability is absent

### Task 8.3: Toasts and confirmation dialogs
**Modifies:** Various files

Ensure all user actions have appropriate feedback:
- **Create watch**: success toast with watch name
- **Update watch**: success toast
- **Delete watch**: confirmation dialog → success toast
- **Toggle watch**: optimistic update with error rollback toast
- **Run now**: toast "Watch triggered" (or error if rate limited)
- **Mark read/unread**: optimistic (no toast)
- **Delete alert**: toast with "Undo" action (soft delete)
- **Restore alert**: success toast
- **Bulk operations**: toast with count ("3 alerts marked as read")
- **Continue in chat**: navigates to chat page (no toast needed)

Use the existing toast system from the Leptos component library.

**Acceptance criteria:**
- All actions have appropriate user feedback
- Confirmation required before destructive actions
- Optimistic updates roll back on error
- Toast messages match React exactly

### Task 8.4: Accessibility
**Modifies:** Various files

- Checkboxes in alert list have proper labels
- Bulk action toolbar announced by screen reader
- Tab switching has ARIA roles (tablist, tab, tabpanel)
- Modal focus trapping (watch modal, execution log modal)
- Alert expand/collapse uses aria-expanded
- Status badges have aria-label (not just color)
- Keyboard navigation for watch card actions

**Acceptance criteria:**
- Tab key navigates all interactive elements
- Screen readers announce state changes
- Focus trapped in modals
- Color-only information has text alternative

---

## Verification After Each Phase

After completing each phase:
1. All `cargo check -p kyomi-ui --features ssr` passes with zero errors
2. `cd crates/kyomi-ui && trunk build --public-url /leptos/` succeeds
3. Server starts and serves the Leptos page
4. Visual comparison: Leptos page matches React page in browser
5. All server functions return correct data
6. Watches route serves Leptos (after Phase 7)

---

## File Structure

```
crates/kyomi-ui/src/pages/watches/
├── mod.rs                    # WatchesPage — page shell + tab routing
├── types.rs                  # All shared types (WatchSummary, WatchExecution, etc.)
├── cron_utils.rs             # Cron parsing, building, human-readable descriptions
├── watch_card.rs             # Individual watch card for grid view
├── watch_modal.rs            # Quick edit modal for existing watches
├── preview_card.rs           # Watch config preview card (chat + standalone)
├── agent_sidebar.rs          # AI chat sidebar for watch creation
├── alerts_history.rs         # Gmail-style alert inbox with bulk actions
├── execution_log.rs          # Execution detail viewer (chat-style)
├── execution_selector.rs     # Dropdown for switching between executions
└── schedule_selector.rs      # Cron schedule builder UI

crates/kyomi-ui/src/server_fns/
└── watches.rs                # All watch server functions (~20 functions)
```

---

## Dependencies

### External dependencies (check what's already available)
- **ChatInterface** — shared chat component needed for agent sidebar.
  If not yet built by the Chat migration, Task 4.1 must build a minimal version
  or be deferred until Chat Phase 1 is done.
- **MarkdownRenderer** — already exists in `components/dashboard/markdown_renderer.rs` ✓
- **ConfirmDialog** — already exists in `components/confirm_dialog.rs` ✓
- **Modal** — already exists in `components/modal.rs` ✓
- **Toast** — check if toast system exists, build if not
- **Slider/Toggle** — Switch component in `components/switch.rs` ✓

### Phase dependency graph

```
Phase 1 (Types + Server Fns + Cron Utils)
  ├── Phase 2 (Schedule Selector) — needs cron_utils
  ├── Phase 3 (Watch Cards + Modal + Preview Card) — needs types, server fns, Phase 2
  ├── Phase 5 (Alerts History) — needs types, alert server fns
  └── Phase 6 (Execution History) — needs types, execution server fns
Phase 4 (Agent Sidebar) — needs Phase 3 (preview card, modal) + ChatInterface
Phase 7 (Page Assembly) — needs all of 2-6
Phase 8 (Polish) — needs assembled page
```

Phases 2, 5, and 6 can run in parallel after Phase 1. Phase 3 needs Phase 2.
Phase 4 needs Phase 3 + Chat dependency. Phase 7 and 8 are sequential at the end.

---

## Summary

| Phase | Tasks | New Server Fns | New Components | Complexity |
|-------|-------|---------------|----------------|------------|
| 1 | 6 | ~20 (CRUD, executions, alerts, bulk ops) | 0 (types + utils only) | Medium |
| 2 | 1 | 0 | ScheduleSelector | Hard (timezone logic) |
| 3 | 3 | 0 | WatchCard, WatchModal, PreviewCard | Medium |
| 4 | 1 | 0 | AgentSidebar (+ ChatInterface dep) | Medium-Hard |
| 5 | 2 | 0 | AlertsHistory, AlertRow | Hard (bulk actions, Gmail UX) |
| 6 | 2 | 0 | ExecutionSelector, ExecutionLog | Medium |
| 7 | 4 | 0 | Page shell, Route, WebSocket, Badge | Medium |
| 8 | 4 | 0 | Mobile, Capability gates, Toasts, A11y | Medium |
| **Total** | **23** | **~20** | **~11** | |
