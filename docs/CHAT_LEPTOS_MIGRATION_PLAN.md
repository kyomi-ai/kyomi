# Chat Pages — Leptos Migration Plan

> Comprehensive, feature-complete migration of `/chat`, `/chat/:session_id`, and `/chats`
> from React to Leptos. Every feature in the React implementation must be present in
> the Leptos version — no shortcuts, no deferred features, no placeholders.

## Architecture Overview

### Source Files (React)
| File | Lines | Purpose |
|------|-------|---------|
| `pages/Chat.jsx` | 1826 | Main chat page — session management, message send, streaming, shared conversations |
| `pages/ChatsList.jsx` | 629 | Chat history — search, filters, bulk delete, real-time unread updates |
| `components/ChatInterface.jsx` | 635 | Reusable chat UI (used by Chat page + Copilot sidebar) |
| `components/AgentThinking.jsx` | 240 | Expandable thinking events display |
| `hooks/useChatState.js` | 230 | State machine: IDLE → SENDING → STREAMING → IDLE |
| `hooks/useAgentThinking.js` | 166 | Thinking event processing + WebSocket subscription |
| `components/MarkdownRenderer.jsx` | 610 | Markdown + ChartML chart rendering (partially exists in Leptos) |
| `components/InlineEditableTitle.jsx` | ~80 | Click-to-edit session title |

### Target Files (Leptos)
```
crates/kyomi-ui/src/
├── pages/
│   └── chat/
│       ├── mod.rs                    # Module exports
│       ├── chat_list.rs              # /chats — session list page
│       ├── chat_page.rs              # /chat and /chat/:session_id — main chat
│       └── chat_message.rs           # Memoized message bubble component
├── components/
│   ├── chat/
│   │   ├── mod.rs                    # Module exports
│   │   ├── agent_thinking.rs         # Expandable thinking events UI
│   │   ├── chat_input.rs             # Textarea + send/stop buttons
│   │   ├── inline_editable_title.rs  # Click-to-edit title
│   │   └── websocket_client.rs       # WebSocket connection + event dispatch
│   └── dashboard/
│       └── markdown_renderer.rs      # Already exists — extend for chat features
├── server_fns/
│   ├── chat.rs                       # Chat server functions (new)
│   └── copilot.rs                    # Already exists — shares WebSocket patterns
└── hooks/
    └── chat_state.rs                 # State machine (new module under components/chat/)
```

### Backend Services (Already Exist — No Changes Needed)
- `kyomi-auth/src/chat_service.rs` (14,222 lines) — All CRUD, encryption, shared conversations
- `kyomi-core/src/websocket.rs` — Message types
- `kyomi-auth/src/websocket/manager.rs` — Connection management
- `kyomi-auth/src/websocket/helpers.rs` — Stream/complete/thinking event senders
- `apps/server/src/routes/chat.rs` — REST endpoints
- `apps/server/src/routes/websocket.rs` — WebSocket upgrade handler
- `kyomi-agent/src/execution.rs` — Agent execution pipeline
- `kyomi-agent/src/thinking.rs` — Thinking event tracking

### Key Dependency: WebSocket Client
The React app uses a `WebSocketContext` provider that maintains a persistent WebSocket
connection and dispatches events by type. Leptos has no equivalent yet. The copilot
sidebar currently makes server function calls and gets empty responses (AI responses
arrive via WebSocket but aren't received on the client). **This is the critical new
infrastructure needed.**

---

## Phase 1: WebSocket Client Infrastructure

> **Why first**: Every chat feature depends on WebSocket events. Without this, nothing
> streams. This also unblocks the copilot sidebar (currently broken for AI responses).

### Task 1.1: WebSocket Context Provider
**File**: `crates/kyomi-ui/src/components/chat/websocket_client.rs`
**Estimated lines**: ~350

Create a Leptos context provider that:
1. Obtains a WebSocket token via server function (`GET /api/v1/auth/websocket-token`)
2. Establishes WebSocket connection to `ws(s)://host/ws/{workspace_id}_{user_id}?token={jwt}`
3. Maintains connection state signal: `disconnected | connecting | connected | reconnecting`
4. Auto-reconnects with exponential backoff (max 30s, 10 attempts) — match React exactly
5. Sends ping every 45s (Cloudflare 100s timeout)
6. Provides `subscribe(message_type, callback)` → returns cleanup function
7. Provides `send(message)` for outbound messages (cancel_request, etc.)
8. Deduplicates messages using `type_sessionId_messageId_data` key — match React
9. Parses incoming JSON into `WebSocketMessage` struct (from `kyomi-core/src/websocket.rs`)
10. Uses `web-sys::WebSocket` (already in Cargo.toml dependencies)

**Server function needed**:
```rust
#[server(prefix = "/leptos-api")]
pub async fn get_websocket_token() -> Result<WebSocketTokenResponse, ServerFnError>
// Returns: { token: String, user_id: String, workspace_id: String }
```

**Integration**: Wrap in `<Layout>` so all app pages have WebSocket access.

**Reference**: React `WebSocketContext.jsx` for exact reconnection logic, deduplication,
and event dispatch patterns.

**Verification**:
- `cargo check --workspace`
- Unit test: construct provider, verify connection state signal transitions
- Integration: open browser, verify WebSocket connects (check browser DevTools network tab)

### Task 1.2: WebSocket Token Server Function
**File**: `crates/kyomi-ui/src/server_fns/chat.rs` (start this file)
**Estimated lines**: ~50

Create the server function that obtains a WebSocket token from the existing REST endpoint.
The backend already has `GET /api/v1/auth/websocket-token` — call the underlying service
directly (don't HTTP-call ourselves).

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WebSocketConfig {
    pub token: String,
    pub user_id: String,
    pub workspace_id: String,
}
```

**Implementation**: Look at how `apps/server/src/routes/websocket.rs` generates tokens
and replicate the same logic in the server function. The token is a JWT signed with
the app's JWT secret.

**Verification**: `cargo check --workspace`

### Task 1.3: Integrate WebSocket Provider into Layout
**File**: `crates/kyomi-ui/src/components/layout.rs` (modify)
**File**: `crates/kyomi-ui/src/app.rs` (modify)

Wrap the `<Layout>` component's children with the WebSocket provider so all authenticated
pages have access. The provider should:
- Only connect when user is authenticated (check auth context)
- Disconnect on logout
- Be available via `use_context::<WebSocketContext>()`

**Verification**:
- `cargo check --workspace`
- Browser test: navigate to any page, verify WebSocket connects in network tab

---

## Phase 2: Chat State Machine + Thinking Events

> **Why second**: These are pure-logic modules with no UI. They're used by every chat
> component and can be tested independently.

### Task 2.1: Chat State Machine
**File**: `crates/kyomi-ui/src/components/chat/chat_state.rs`
**Estimated lines**: ~180

Port `useChatState.js` to a Leptos reactive struct:

**States**: `Idle | Sending | Streaming | Cancelling | Cancelled | Error`

**Valid transitions** (enforce these, log invalid ones):
```
Idle → Sending
Sending → Streaming | Error | Idle
Streaming → Idle | Cancelling | Error
Cancelling → Cancelled | Idle | Error
Cancelled → Idle
Error → Idle
```

**Signals exposed**:
- `state: ReadSignal<ChatState>`
- `active_message_id: ReadSignal<Option<String>>`
- `active_session_id: ReadSignal<Option<String>>`
- `error: ReadSignal<Option<String>>`

**Computed signals**:
- `can_send: Signal<bool>` — `state == Idle`
- `is_sending: Signal<bool>`
- `is_streaming: Signal<bool>`
- `show_stop_button: Signal<bool>` — `Sending | Streaming | Cancelling`
- `can_cancel: Signal<bool>` — `Streaming && active_message_id.is_some()`

**Methods**: `start_sending()`, `start_streaming(message_id)`, `request_cancel() -> bool`,
`confirm_cancelled()`, `complete()`, `set_error(msg)`, `reset()`

**Auto-reset**: `Cancelled` and `Error` auto-transition to `Idle` after 100ms
(use `gloo-timers::callback::Timeout`).

**Reference**: React `useChatState.js` lines 27-195 for exact logic.

**Verification**: `cargo check --workspace`

### Task 2.2: Thinking Event Processing
**File**: `crates/kyomi-ui/src/components/chat/thinking.rs`
**Estimated lines**: ~150

Port `useAgentThinking.js` to Leptos:

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThinkingEvent {
    pub event_id: String,
    pub event_type: String,  // agent_start, agent_thought, tool_execution_start, etc.
    pub timestamp: String,
    pub title: String,
    pub description: Option<String>,
    pub data: Option<serde_json::Value>,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct ThinkingState {
    pub events: Vec<ThinkingEvent>,
    pub is_active: bool,
    pub cancelled: bool,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}
```

**Functions**:
- `process_thinking_event(existing: &[ThinkingEvent], new: ThinkingEvent) -> Vec<ThinkingEvent>`
  - Deduplicates by `event_id`
  - Updates in-place if exists
  - Appends and sorts lexicographically by `event_id` if new

**Reactive state**: `RwSignal<HashMap<String, ThinkingState>>` keyed by message_id

**Methods**:
- `handle_thinking_event(message_id, event, token_usage)`
- `complete_thinking(message_id)` — sets `is_active = false`
- `cancel_thinking(message_id)` — sets `is_active = false, cancelled = true`
- `clear_all()` — resets everything
- `get_for_message(message_id) -> ThinkingState`

**Reference**: React `useAgentThinking.js` for exact deduplication logic.

**Verification**: `cargo check --workspace`

---

## Phase 3: Chat Server Functions

> **Why third**: Server functions provide the data layer. Pages need these to fetch/create
> sessions and send messages.

### Task 3.1: Session CRUD Server Functions
**File**: `crates/kyomi-ui/src/server_fns/chat.rs` (extend from Task 1.2)
**Estimated lines**: ~300 (cumulative with Task 1.2)

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatSessionItem {
    pub session_id: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub session_type: Option<String>,
    pub shared: bool,
    pub shared_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: i64,
    pub pinned_count: i64,
    pub unread_count: i64,
    pub created_by: Option<SessionUser>,
    pub slack_channel_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionUser {
    pub user_id: String,
    pub display_name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessageItem {
    pub message_id: String,
    pub message_type: String,       // "user" or "assistant"
    pub content: String,
    pub timestamp: String,
    pub pinned: bool,
    pub sent_by: Option<SessionUser>,
    pub thinking_events: Vec<serde_json::Value>,
    pub token_usage: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionDetail {
    pub title: Option<String>,
    pub shared: bool,
    pub created_by: Option<SessionUser>,
    pub slack_channel_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionMessagesResponse {
    pub messages: Vec<ChatMessageItem>,
    pub session: SessionDetail,
}
```

**Server functions**:
```rust
#[server] pub async fn list_chat_sessions(pinned_only: bool) -> Result<Vec<ChatSessionItem>, ServerFnError>
#[server] pub async fn get_session_messages(session_id: String) -> Result<SessionMessagesResponse, ServerFnError>
#[server] pub async fn update_session_title(session_id: String, title: String) -> Result<(), ServerFnError>
#[server] pub async fn delete_chat_session(session_id: String) -> Result<(), ServerFnError>
#[server] pub async fn bulk_delete_sessions(session_ids: Vec<String>) -> Result<(), ServerFnError>
#[server] pub async fn search_chat_messages(query: String) -> Result<Vec<ChatSessionItem>, ServerFnError>
```

**Implementation**: Call `kyomi_auth::chat_service::*` directly (same pattern as
`server_fns/dashboards.rs`). Use `extract_auth()` and `extract_context()`.

**Reference**: `apps/server/src/routes/chat.rs` handlers for exact service calls and
parameter mapping.

**Verification**: `cargo check --workspace`

### Task 3.2: Message Send Server Function
**File**: `crates/kyomi-ui/src/server_fns/chat.rs` (extend)
**Estimated lines**: ~150 (added to file)

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageResponse {
    pub session_id: String,
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub status: String,
    pub thinking_events: Vec<serde_json::Value>,
    pub token_usage: Option<serde_json::Value>,
    pub skip_ai: bool,
}
```

**Server function**:
```rust
#[server]
pub async fn send_chat_message(
    message: String,
    session_id: Option<String>,
    current_time_user_tz: Option<String>,
    skip_ai: bool,
) -> Result<SendMessageResponse, ServerFnError>
```

**This is the most complex server function.** It must replicate the logic from
`apps/server/src/routes/chat.rs::send_message`:

1. Validate message (non-empty, <100KB)
2. Check AI capability (credits not exhausted, LLM configured)
3. Find or create session
4. Generate `user_message_id` and `assistant_message_id`
5. Create `AgentExecutionConfig` with cancel token
6. Register cancel token in cancel registry
7. Spawn async agent execution task (fire-and-forget)
8. Fire-and-forget title generation for new sessions
9. Return immediately with IDs and status

**Critical**: The server function needs access to:
- `CancelRegistry` — add to `ServerContext` if not already there
- `kyomi_agent::execute_agent_chat` — for spawning the agent
- `kyomi_auth::websocket::helpers::*` — for streaming delivery

**Reference**: `apps/server/src/routes/chat.rs` lines for `send_message` handler. Match
it exactly — do not deviate from the Python→Rust migration principle.

**Verification**: `cargo check --workspace`, then browser test: send a message, verify
response streams back via WebSocket.

### Task 3.3: Collaboration Server Functions
**File**: `crates/kyomi-ui/src/server_fns/chat.rs` (extend)
**Estimated lines**: ~100 (added to file)

```rust
#[server] pub async fn share_session(session_id: String) -> Result<(), ServerFnError>
#[server] pub async fn unshare_session(session_id: String) -> Result<(), ServerFnError>
#[server] pub async fn mark_session_read(session_id: String, last_message_id: Option<String>) -> Result<(), ServerFnError>
#[server] pub async fn toggle_message_pin(session_id: String, message_id: String) -> Result<bool, ServerFnError>
#[server] pub async fn update_message_content(session_id: String, message_id: String, content: String) -> Result<(), ServerFnError>
```

**Implementation**: Direct calls to `chat_service::*` functions. Match the REST endpoint
handlers exactly.

**Verification**: `cargo check --workspace`

---

## Phase 4: Agent Thinking UI Component

> **Why fourth**: The thinking component is self-contained and shared between chat and
> copilot. Build it before the chat page so it's ready to plug in.

### Task 4.1: Agent Thinking Component
**File**: `crates/kyomi-ui/src/components/chat/agent_thinking.rs`
**Estimated lines**: ~300

Port `AgentThinking.jsx` (240 lines React → ~300 lines Rust):

**Props**:
```rust
#[component]
pub fn AgentThinking(
    #[prop(default = vec![])] thinking_events: Vec<ThinkingEvent>,
    #[prop(default = false)] is_active: bool,
    #[prop(default = "inset")] variant: &'static str,  // "inset" | "header-bar" | "tab" | "default"
    #[prop(optional)] token_usage: Option<TokenUsage>,
) -> impl IntoView
```

**Features to implement**:
1. **Expandable/collapsible** — click header to toggle
2. **Live timer** — updates every 100ms while `is_active` (use `gloo-timers::callback::Interval`)
3. **Auto-scroll** — scroll to bottom of events list when new events arrive
4. **Event icons** — emoji per event_type (agent_start→🤖, agent_thought→💭, tool_execution_start→🔧, tool_execution_end→✅, agent_decision→🎯, agent_complete→🎉, error→⚠️)
5. **Duration formatting** — `<1000ms` → "Xms", `>=1000ms` → "X.Xs"
6. **Timestamp formatting** — HH:MM:SS.f (24-hour)
7. **Strip emojis** from header title text
8. **Token usage display** — show prompt + completion token counts
9. **Tool count** — count events with type `tool_execution_start`
10. **4 rendering variants** — different container styles per variant:
    - `inset`: `mb-4 -mx-6 -mt-2 bg-accent border-l-4 border-primary shadow-inner`
    - `header-bar`: `mb-3 -mx-6 -mt-4 bg-muted border-b border-border`
    - `tab`: sticky tab, card-based
    - `default`: plain card

**CSS classes**: Copy verbatim from React `AgentThinking.jsx`.

**Reference**: React `AgentThinking.jsx` lines 1-240 for exact layout and styling.

**Verification**: `cargo check --workspace`

---

## Phase 5: Chat Input Component

> **Why fifth**: The input area is complex enough to be its own component — auto-resize
> textarea, send/stop button toggling, keyboard handling, connection status.

### Task 5.1: Chat Input Component
**File**: `crates/kyomi-ui/src/components/chat/chat_input.rs`
**Estimated lines**: ~250

**Props**:
```rust
#[component]
pub fn ChatInput(
    #[prop(into)] on_send: Callback<String>,
    #[prop(into)] on_cancel: Callback<()>,
    #[prop(into)] can_send: Signal<bool>,
    #[prop(into)] show_stop_button: Signal<bool>,
    #[prop(into)] connection_state: Signal<String>,
    #[prop(default = "Ask me anything...")] placeholder: &'static str,
    #[prop(default = false)] show_skip_ai: bool,       // For shared conversations
    #[prop(into, optional)] skip_ai: Option<RwSignal<bool>>,
    #[prop(default = false)] credits_exhausted: bool,
    #[prop(default = 200)] max_height: u32,             // 200 for full, 120 for sidebar
) -> impl IntoView
```

**Features**:
1. **Auto-expanding textarea** — grows from min 52px to `max_height`px based on content
   - On input: set `style:height = "auto"` then `style:height = scrollHeight + "px"`
   - Use `web_sys::HtmlTextAreaElement` for DOM access
2. **Send on Enter** (not Shift+Enter) — `on:keydown` handler
3. **Send button** — enabled when `can_send && input_not_empty && connected`
   - Icon: paper airplane SVG
   - Style: `bg-primary text-primary-foreground`
4. **Stop button** — shown when `show_stop_button` is true
   - Icon: square SVG
   - Style: `bg-destructive text-white`
5. **"Skip AI response" checkbox** — only shown when `show_skip_ai` is true
   - For shared conversations: post comment without triggering AI
6. **Credits exhausted message** — shown instead of input when exhausted
7. **Connection status indicator** — pulsing dot + text when not connected
8. **Auto-focus** — focus textarea on mount (100ms delay)

**CSS**: Match React `ChatInterface.jsx` input area (lines 564-628).

**Verification**: `cargo check --workspace`

### Task 5.2: Inline Editable Title Component
**File**: `crates/kyomi-ui/src/components/chat/inline_editable_title.rs`
**Estimated lines**: ~100

**Props**:
```rust
#[component]
pub fn InlineEditableTitle(
    #[prop(into)] value: Signal<String>,
    #[prop(into)] on_save: Callback<String>,
    #[prop(default = "Untitled")] placeholder: &'static str,
) -> impl IntoView
```

**Features**:
1. **Display mode** — shows title as text, clickable to edit
2. **Edit mode** — inline textarea, saves on blur or Enter
3. **Escape to cancel** — reverts to original value
4. Auto-focus when entering edit mode

**Reference**: React `InlineEditableTitle.jsx`.

**Verification**: `cargo check --workspace`

---

## Phase 6: Chats List Page

> **Why sixth**: The list page is simpler than the chat page and establishes patterns
> for session loading, search, and WebSocket subscriptions.

### Task 6.1: Chats List Page — Core Layout + Session Loading
**File**: `crates/kyomi-ui/src/pages/chat/chat_list.rs`
**Estimated lines**: ~250 (this task)

**Implement**:
1. Page header: "Chats" title + "New Chat" button (navigates to `/chat`)
2. Session list loading via `Resource::new` calling `list_chat_sessions(false)`
3. `Suspense` wrapper with `Spinner` fallback
4. Session item rendering:
   - Title (or "New conversation" if null)
   - Relative time formatting (`formatDate` equivalent — "Just now", "Xm ago", etc.)
   - Status badges: Private/Shared/Slack (based on `getChatStatus` logic)
   - Unread count badge (red, "X new")
   - Click to navigate to `/chat/:session_id`
5. Empty state: "No chats yet" with "Start New Chat" button
6. Loading state: spinner

**Reference**: React `ChatsList.jsx` lines 1-200, 290-400 (session items), 590-620 (list rendering).

**Verification**: `cargo check --workspace`, browser test: navigate to `/chats`, see session list.

### Task 6.2: Chats List — Search + Filters
**File**: `crates/kyomi-ui/src/pages/chat/chat_list.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement**:
1. **Search input** with 300ms debounce (same pattern as dashboards_list.rs)
   - Searching spinner indicator
   - Clear button (X icon)
   - Calls `search_chat_messages(query)` server function
2. **Pinned filter toggle** — star icon button, toggles `show_pinned_only` signal
   - Reloads sessions with `list_chat_sessions(pinned_only)`
3. **Chat filter buttons**: All / My Conversations / Shared with Me / Slack
   - Client-side filtering of loaded sessions
   - Filter logic from `getFilteredSessions()`:
     - `mine`: `created_by.user_id == current_user_id`
     - `shared_with_me`: NOT owned
     - `slack`: has `slack_channel_id` AND `shared`
4. **Results count** when searching
5. **Clear selection** when filter/search/pinned changes

**Reference**: React `ChatsList.jsx` lines 63-147 (effects), 207-231 (filter logic), 438-587 (toolbar UI).

**Verification**: `cargo check --workspace`, browser test: search sessions, toggle filters.

### Task 6.3: Chats List — Delete + Bulk Operations
**File**: `crates/kyomi-ui/src/pages/chat/chat_list.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement**:
1. **Individual delete** — hover delete button on owned sessions
   - Confirm dialog before deletion
   - Calls `delete_chat_session(session_id)` server function
   - Removes from list reactively
2. **Bulk selection** — checkboxes on owned sessions
   - "Select all" checkbox in header (only selectable/owned sessions)
   - Per-session checkbox
   - Uses `RwSignal<HashSet<String>>` for selected IDs
3. **Bulk delete bar** — appears when sessions selected
   - Shows count: "X selected"
   - Delete button with confirmation dialog
   - Cancel button to clear selection
   - Calls `bulk_delete_sessions(ids)` server function
4. **Custom event dispatch** — fire `sessions-deleted` event (for sidebar update)
   - Use `web_sys::CustomEvent` with detail containing deleted session IDs

**Reference**: React `ChatsList.jsx` lines 234-290 (selection/delete handlers), 440-590 (bulk UI).

**Verification**: `cargo check --workspace`, browser test: delete single session, bulk delete.

### Task 6.4: Chats List — Real-time WebSocket Updates
**File**: `crates/kyomi-ui/src/pages/chat/chat_list.rs` (extend)
**Estimated lines**: ~80 (added)

**Implement**:
1. **Subscribe to `shared_conversation_activity`** WebSocket event
   - On event: update matching session's `updated_at` and increment `unread_count`
   - Re-sort sessions list to move updated session to top
2. **Subscribe to `sessions-deleted`** custom DOM event
   - Remove deleted sessions from the displayed list
   - Clear any deleted sessions from selected set
3. **Cleanup**: Unsubscribe on component unmount via `on_cleanup()`

**Reference**: React `ChatsList.jsx` lines 35-113.

**Verification**: `cargo check --workspace`, browser test: open shared conversation in another tab, verify unread count updates.

---

## Phase 7: Chat Page — Core Message Display

> **Why seventh**: Build the read-only message display first, then add send/stream on top.

### Task 7.1: Chat Message Component
**File**: `crates/kyomi-ui/src/pages/chat/chat_message.rs`
**Estimated lines**: ~300

**Props**:
```rust
#[component]
pub fn ChatMessage(
    message: ChatMessageItem,
    thinking_state: Signal<ThinkingState>,
    is_streaming: Signal<bool>,
    active_message_id: Signal<Option<String>>,
    current_session_id: Signal<Option<String>>,
    session_metadata: Signal<SessionDetail>,
    current_user_id: String,
    on_toggle_pin: Callback<String>,            // message_id
    on_open_dashboard_modal: Callback<String>,  // message content
    on_message_update: Callback<(String, String)>, // (message_id, content)
) -> impl IntoView
```

**Features**:
1. **User message bubble** — right-aligned, primary background
   - CSS: `max-w-sm md:max-w-md lg:max-w-lg xl:max-w-xl px-4 py-3 bg-primary text-primary-foreground rounded-2xl shadow-sm`
   - Show sender name (display_name or "You") + relative timestamp
   - Determine "is mine" from `sent_by.user_id == current_user_id` OR no `sent_by` (private)
2. **Assistant message bubble** — full width, card background
   - CSS: `w-full px-6 py-4 bg-card border border-border rounded-2xl shadow-sm overflow-hidden`
   - Agent thinking header (if active or has events)
   - MarkdownRenderer for content
   - Footer: "Kyomi" + timestamp
   - Pin button (star icon, filled if pinned)
   - Save to dashboard button (grid icon)
3. **Streaming indicator** — show thinking animation when `is_active && message.id == active_message_id`
4. **Sender name logic** (for shared conversations):
   - Assistant: "Kyomi"
   - User: `sent_by.display_name` or current user name or email or "You"

**Reference**: React `Chat.jsx` ChatMessage component (lines 35-203) for exact styling.

**Verification**: `cargo check --workspace`

### Task 7.2: Chat Page — Session Loading + Message Display
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs`
**Estimated lines**: ~400 (this task)

**Implement**:
1. **URL parameter parsing** — extract `session_id` from `/chat/:session_id` route
2. **Session loading effect** — when `session_id` changes:
   - Set `is_loading = true`
   - Call `get_session_messages(session_id)` server function
   - Populate messages signal, session metadata, title
   - Set `is_loading = false`
3. **New chat mode** — when no `session_id`:
   - Empty messages list
   - Generate random greeting (from pool of 20 templated greetings)
   - Show empty state with centered greeting + input area
4. **Messages container** — scrollable, `space-y-6` gap
   - Render `ChatMessage` for each message
   - Pinned filter: if `show_pinned_only`, only show messages where `pinned == true`
   - Auto-scroll ref at bottom
5. **Loading state** — spinner + "Loading session..."
6. **Smart scroll** — only auto-scroll if user is near bottom (within 100px)
   - Debounce scroll-to-bottom by 50ms to avoid stuttering

**State signals**:
```rust
let (messages, set_messages) = signal(Vec::<ChatMessageItem>::new());
let (current_session_id, set_current_session_id) = signal(Option::<String>::None);
let (session_title, set_session_title) = signal(String::new());
let (session_metadata, set_session_metadata) = signal(SessionDetail::default());
let (is_loading, set_is_loading) = signal(false);
let (show_pinned_only, set_show_pinned_only) = signal(false);
let (current_greeting, set_current_greeting) = signal(String::new());
```

**Reference**: React `Chat.jsx` lines 206-450 (state + loading).

**Verification**: `cargo check --workspace`, browser test: navigate to `/chat/:id`, see messages.

---

## Phase 8: Chat Page — Message Sending + Streaming

> **Why eighth**: This is the core interactive feature. Depends on WebSocket (Phase 1),
> state machine (Phase 2), server functions (Phase 3).

### Task 8.1: Message Send Flow
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement the send_message handler**:
1. Validate: input not empty, `chat_state.can_send()`, WebSocket connected
2. Create optimistic user message with generated ID, add to messages
3. Compute time context: `YYYY-MM-DDTHH:MM:SS±HH:MM` format
   - Use `js_sys::Date` for timezone offset on WASM
4. Call `chat_state.start_sending(session_id)`
5. Call `send_chat_message()` server function with:
   - `message`, `session_id` (None for new chat), `current_time_user_tz`, `skip_ai`
6. On success: update `current_session_id` from response (for new chats)
   - Navigate to `/chat/:session_id` with `replace: true` if new
7. On error: display error, `chat_state.set_error(msg)`
8. Integrate with `ChatInput` component

**Time context**: The agent needs the user's local time + timezone to handle queries
like "growth this week". Use `js_sys::Date` to get offset, format as ISO 8601.

**Reference**: React `Chat.jsx` send_message handler, `ChatInterface.jsx` lines 384-460.

**Verification**: `cargo check --workspace`, browser test: send a message, verify server
function is called and session ID returned.

### Task 8.2: WebSocket Streaming — Response Chunks
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~200 (added)

**Subscribe to WebSocket events** (using provider from Phase 1):

1. **`session_created`** event:
   - Only process if in SENDING state (user just sent a message)
   - Update `session_title` from event data
   - Update `session_metadata` (shared, created_by, slack_channel_id)

2. **`title_update`** event:
   - Update `session_title` if session_id matches

3. **`chat_stream`** event:
   - Filter by `session_id` matching `current_session_id`
   - Append `content` chunk to matching assistant message text
   - Mark message as `is_streaming = true`
   - Trigger smart scroll

4. **`chat_complete`** event:
   - Ignore if state is Cancelling/Cancelled
   - Update message with full `content`
   - Set `is_streaming = false`
   - Call `chat_state.complete()`
   - Stop thinking animation for this message
   - Process any `thinking_events` snapshot from the event data

5. **`request_cancelled`** event:
   - Call `chat_state.confirm_cancelled()`
   - Update message text to "_Request cancelled by user._"
   - Stop thinking animation

6. **`error`** event:
   - Call `chat_state.set_error(message)`

**Cleanup**: Unsubscribe all listeners on component unmount.

**Reference**: React `Chat.jsx` WebSocket subscription block (lines 345-681).

**Verification**: `cargo check --workspace`, browser test: send message, see streaming
response appear character-by-character.

### Task 8.3: WebSocket Streaming — Thinking Events
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~150 (added)

**Subscribe to `agent_thinking` WebSocket event**:

1. Filter by session_id (allow null for new-chat race condition)
2. **First thinking event** for a message:
   - Create placeholder assistant message (empty text, `is_streaming = true`)
   - Transition `chat_state` from SENDING to STREAMING
   - If session_id was null, update `current_session_id`
3. **Buffering**: If `current_session_id` is still None and state is SENDING:
   - Buffer the event (store in signal)
   - Still add to thinking state immediately for UI
4. Process event via `thinking.handle_thinking_event(message_id, event, token_usage)`

**Subscribe to `token_usage_update` WebSocket event**:
- Update `token_usage` in thinking state for the message

**Flush buffer**: When `current_session_id` becomes set, replay buffered events.

**Reference**: React `Chat.jsx` lines 386-528 (agent_thinking + token_usage handlers).

**Verification**: `cargo check --workspace`, browser test: send message, see thinking
events appear in real-time before response starts streaming.

### Task 8.4: Cancellation Flow
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~50 (added)

**Implement cancel handler**:
1. Call `chat_state.request_cancel()` — returns false if invalid state
2. Send WebSocket message: `{ "type": "cancel_request", "session_id": "...", "message_id": "..." }`
   - Use the `send()` method from WebSocket provider
3. The `request_cancelled` WebSocket event handler (Task 8.2) completes the flow

**Wire to `ChatInput` component's `on_cancel` callback.**

**Reference**: React `Chat.jsx` handleCancel + ChatInterface.jsx lines 471-480.

**Verification**: `cargo check --workspace`, browser test: send message, click stop, verify cancellation.

---

## Phase 9: Chat Page — Session Management + Header

> **Why ninth**: These are polish features that make the chat page fully functional.

### Task 9.1: Chat Header — Title + Badges + Actions
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement the header bar** (only shown when messages exist):

1. **Inline editable title** — `InlineEditableTitle` component
   - On save: call `update_session_title()` server function
2. **Slack sync badge** — show if `session_metadata.slack_channel_id` is Some and shared
   - Text: "Synced with Slack"
   - Style: Badge component with muted variant
3. **Share/Private status badge** — show in multi-user workspaces
   - Private: lock icon + "Private"
   - Shared: globe icon + "Shared"
4. **Share dropdown** (owner only, team plan):
   - "Share with Workspace" button → calls `share_session()` server function
   - "Make Private" button → calls `unshare_session()` server function
   - Use existing dropdown/popover pattern
5. **Pinned messages filter button** — star icon, toggles `show_pinned_only`

**Reference**: React `Chat.jsx` header section.

**Verification**: `cargo check --workspace`, browser test: edit title, toggle share.

### Task 9.2: Message Actions — Pin + Save to Dashboard
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~150 (added)

**Implement**:
1. **Toggle pin** handler:
   - Call `toggle_message_pin()` server function
   - Update message's `pinned` state locally
2. **Save to dashboard modal**:
   - Re-use existing `SaveDashboardModal` component from dashboard module
   - On open: pass message content (markdown)
   - On save: create new dashboard or append to existing
3. **Chart info modal**:
   - Re-use existing `ChartInfoModal` component
   - Shows chart YAML spec when user clicks chart info button

**Reference**: React `Chat.jsx` handleTogglePin, onOpenDashboardModal, onShowChartInfo.

**Verification**: `cargo check --workspace`, browser test: pin a message, open save modal.

### Task 9.3: Shared Conversation Features
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~150 (added)

**Implement**:
1. **"Skip AI response" checkbox** — visible when conversation is shared
   - Wired to `skip_ai` signal passed to ChatInput
   - When checked, `send_chat_message()` called with `skip_ai: true`
   - Backend stores message but doesn't trigger AI agent
2. **Shared message display** — subscribe to `shared_chat_message` WebSocket event:
   - Filter by session_id
   - Deduplicate by `client_msg_id` (don't show own messages twice)
   - Add incoming message to messages list
   - Trigger smart scroll
3. **Mark session read** — call `mark_session_read()` on load and on new messages
   - Fire-and-forget via `spawn_local`
4. **Sender attribution** — show sender name for all messages in shared conversations

**Reference**: React `Chat.jsx` shared conversation logic, ChatsList.jsx unread tracking.

**Verification**: `cargo check --workspace`, browser test: share a conversation, send from
another user, verify message appears.

---

## Phase 10: Markdown Renderer — Chat Extensions

> **Why tenth**: The Leptos MarkdownRenderer exists for dashboards but needs chat-specific
> features: watch preview cards, chart actions, streaming cleanup.

### Task 10.1: Streaming Markdown Cleanup
**File**: `crates/kyomi-ui/src/components/dashboard/markdown_renderer.rs` (extend)
**Estimated lines**: ~50 (added)

**Implement**: When rendering streaming content (incomplete markdown), clean up
incomplete ChartML code blocks:
- If the last `\`\`\`chartml` fence is not closed (no matching `\`\`\``), remove
  everything from that fence to end of content
- This prevents partial YAML from being parsed and causing errors during streaming

**Reference**: React `MarkdownRenderer.jsx` lines 578-593 (markdown cleaning).

**Verification**: `cargo check --workspace`

### Task 10.2: Watch Preview Cards
**File**: `crates/kyomi-ui/src/components/dashboard/markdown_renderer.rs` (extend)
**Estimated lines**: ~150 (added)

**Implement**: Detect `watch-response` code blocks in markdown:
1. Parse JSON content: look for objects with `message` and `watch` keys
2. Render message as markdown
3. Render `WatchPreviewCard` component:
   - Shows watch name, schedule, query summary
   - "Approve" button → calls `on_watch_approved` callback
   - "Approved" state (dimmed) when card ID is in `accepted_card_ids`
   - Card ID: `{message_id}-{watch.name}-{watch.schedule}`

**Props to add to MarkdownRenderer**:
```rust
#[prop(optional)] on_watch_approved: Option<Callback<(serde_json::Value, String)>>,
#[prop(optional)] accepted_card_ids: Option<Signal<HashSet<String>>>,
```

**Reference**: React `MarkdownRenderer.jsx` customCodeRenderer lines 442-495.

**Verification**: `cargo check --workspace`

### Task 10.3: Chart Action Buttons
**File**: `crates/kyomi-ui/src/components/dashboard/markdown_renderer.rs` (extend)
**Estimated lines**: ~100 (added)

**Add callbacks to MarkdownRenderer** for chart actions in chat context:
1. **Save to dashboard** — button on chart header, calls callback with chart YAML
2. **Ask about chart** — button on chart header, calls callback with chart context
3. **Chart info** — button to show chart spec YAML

These callbacks are already partially supported for dashboard context. Ensure they
work for chat context too by making the callback props available and wiring them
through `ChartWithChrome` (or equivalent Leptos wrapper).

**Reference**: React `MarkdownRenderer.jsx` ChartWithChrome handler props.

**Verification**: `cargo check --workspace`

---

## Phase 11: Special Navigation Contexts

> **Why eleventh**: These are entry points into chat from other parts of the app.

### Task 11.1: Chart Exploration ("Ask About This Chart")
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~100 (added)

**Implement**: Support navigating to `/chat` with chart context:
1. **From dashboard**: When user clicks "Ask about this chart" on a dashboard chart,
   navigate to `/chat` with chart markdown in router state or query parameter
2. **From URL**: Support `/chat?chart={contextId}` — fetch chart context from server
   (need server function to call the existing `GET /api/v1/chart-context/{chartId}` equivalent)
3. **On first message**: Prepend chart markdown to the user's message content
4. **Clear context** after first message is sent

**State**: `chart_context: RwSignal<Option<ChartContext>>` — stores chart markdown + title

**Reference**: React `Chat.jsx` chart exploration logic.

**Verification**: `cargo check --workspace`, browser test: click "Ask about chart" on dashboard, land on chat with context.

### Task 11.2: Watch Creation Context
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~40 (added)

**Implement**: Support navigating to `/chat` with watch creation intent:
1. Detect `?createWatch=true` query param or router state
2. Create initial system message guiding watch setup
3. Clear context after use

**Reference**: React `Chat.jsx` createWatch handling.

**Verification**: `cargo check --workspace`

---

## Phase 12: Empty States + Personal Mode

> **Why twelfth**: Edge cases and special modes that need to work correctly.

### Task 12.1: No Datasources Empty State
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~80 (added)

**Implement**: When user has no datasources configured:
1. Check via capabilities context or server function
2. Show helpful empty state: "Connect a data source to start chatting"
3. Link to datasource setup page

**Reference**: React `Chat.jsx` NoDatasourcesEmptyState usage.

**Verification**: `cargo check --workspace`

### Task 12.2: Personal Mode Without LLM
**File**: `crates/kyomi-ui/src/pages/chat/chat_page.rs` (extend)
**Estimated lines**: ~60 (added)

**Implement**: When in personal/standalone mode without LLM configured:
1. Show settings button linking to AI provider settings
2. Show "Learn about MCP" external link button
3. Disable chat input

**Reference**: React `ChatInterface.jsx` lines 488-526.

**Verification**: `cargo check --workspace`

---

## Phase 13: Route Wiring + Integration

> **Why last**: Wire everything together and verify end-to-end.

### Task 13.1: Route Registration
**File**: `crates/kyomi-ui/src/app.rs` (modify)
**File**: `crates/kyomi-ui/src/pages/chat/mod.rs` (create)
**File**: `crates/kyomi-ui/src/pages/mod.rs` (modify)
**File**: `crates/kyomi-ui/src/components/chat/mod.rs` (create)
**File**: `crates/kyomi-ui/src/components/mod.rs` (modify)
**File**: `crates/kyomi-ui/src/lib.rs` (modify — register new server functions)

**Implement**:
1. Create `mod.rs` files that export all chat page and component modules
2. Register all new server functions in `lib.rs`
3. Update routes in `app.rs`:
   ```rust
   <Route path=path!("/chat") view=|| view! { <Layout><ChatPage/></Layout> }/>
   <Route path=path!("/chat/:session_id") view=|| view! { <Layout><ChatPage/></Layout> }/>
   <Route path=path!("/chats") view=|| view! { <Layout><ChatListPage/></Layout> }/>
   ```
4. Remove `NotImplementedPage` references for chat routes

**Verification**: `cargo check --workspace`

### Task 13.2: Sidebar Integration
**File**: `crates/kyomi-ui/src/components/layout.rs` (modify)

**Implement**:
1. Update sidebar "New Chat" link to navigate to `/chat`
2. Update sidebar "Chats" link to navigate to `/chats`
3. Verify recent sessions list in sidebar updates when sessions are created/deleted
   (should already work via `get_recent_sessions()` server function)
4. Listen for `sessions-deleted` custom event to refresh recent sessions

**Verification**: `cargo check --workspace`, browser test: sidebar links work.

### Task 13.3: Copilot Sidebar — Wire WebSocket Streaming
**File**: `crates/kyomi-ui/src/components/dashboard/copilot_sidebar.rs` (modify)

**Now that WebSocket infrastructure exists** (Phase 1), fix the copilot sidebar:
1. Subscribe to `chat_stream`, `chat_complete`, `agent_thinking` events
2. Filter by copilot `context_type`
3. Update messages reactively as response streams in
4. Show thinking events
5. Remove the current "empty response" behavior

**Reference**: React `ChatInterface.jsx` WebSocket subscription logic (lines 207-381).

**Verification**: `cargo check --workspace`, browser test: open copilot sidebar on dashboard,
send message, see AI response stream in.

### Task 13.4: End-to-End Testing
**No file changes — verification only.**

**Test matrix** (must pass before declaring complete):

| Test Case | Steps | Expected |
|-----------|-------|----------|
| New chat | Navigate to `/chat`, type message, send | Message appears, AI response streams back |
| Load existing | Click session in sidebar | Messages load, title shown |
| Session list | Navigate to `/chats` | All sessions listed with correct badges |
| Search | Type in search box on `/chats` | Results filter after 300ms |
| Pinned filter | Click star on `/chats` | Only pinned sessions shown |
| Chat filters | Click "My Conversations" | Only owned sessions shown |
| Delete session | Hover session, click delete | Confirm dialog, session removed |
| Bulk delete | Select multiple, click delete | Confirm with count, all removed |
| Pin message | Click star on assistant message | Star fills, message marked pinned |
| Edit title | Click title in chat header | Inline edit, saves on blur |
| Share session | Click share button | Session badge changes to "Shared" |
| Cancel response | Click stop during streaming | Response stops, cancellation message shown |
| Thinking events | Send message, watch thinking | Events appear in expandable panel |
| Chart in response | AI returns ChartML | Chart renders with refresh/info/save buttons |
| Save to dashboard | Click save on chart/message | Modal opens, saves to dashboard |
| Unread count | Receive shared message while away | Badge shows "X new" on `/chats` |
| Smart scroll | Scroll up, then receive message | Does NOT auto-scroll (user is reading) |
| Smart scroll 2 | Stay at bottom, receive message | Auto-scrolls to show new message |
| Personal mode no LLM | Open chat in standalone, no AI key | Shows settings + MCP link |
| No datasources | Open chat with no datasources | Shows helpful empty state |
| Ask about chart | Click "Ask" on dashboard chart | Opens chat with chart context pre-loaded |
| Skip AI checkbox | Shared conversation, check "Skip AI" | Message posted, no AI response |

---

## Estimated Totals

| Phase | Tasks | Est. New Lines | Files Created | Files Modified |
|-------|-------|---------------|---------------|----------------|
| 1. WebSocket Client | 3 | ~400 | 1 | 2 |
| 2. State Machine + Thinking | 2 | ~330 | 2 | 0 |
| 3. Server Functions | 3 | ~550 | 1 | 0 |
| 4. Agent Thinking UI | 1 | ~300 | 1 | 0 |
| 5. Chat Input | 2 | ~350 | 2 | 0 |
| 6. Chats List Page | 4 | ~730 | 1 | 0 |
| 7. Chat Page Core | 2 | ~700 | 2 | 0 |
| 8. Sending + Streaming | 4 | ~600 | 0 | 1 |
| 9. Session Mgmt + Header | 3 | ~500 | 0 | 1 |
| 10. Markdown Extensions | 3 | ~300 | 0 | 1 |
| 11. Navigation Contexts | 2 | ~140 | 0 | 1 |
| 12. Empty States | 2 | ~140 | 0 | 1 |
| 13. Integration + Testing | 4 | ~200 | 2 | 4 |
| **Total** | **35** | **~5,240** | **12** | **11** |

## Task Execution Order

Tasks within each phase are sequential (each builds on the previous). Phases can be
parallelized where dependencies allow:

```
Phase 1 (WebSocket) ──────────────────────────────────────────────────────►
         │
         ├── Phase 2 (State Machine + Thinking) ─────────────────────────►
         │            │
         │            ├── Phase 4 (Thinking UI) ─────────────────────────►
         │            │
         │            ├── Phase 5 (Chat Input) ──────────────────────────►
         │            │
         ├── Phase 3 (Server Functions) ─────────────────────────────────►
         │            │
         │            ├── Phase 6 (Chats List) ──────────────────────────►
         │            │
         │            ├── Phase 7 (Chat Page Core) ──────────────────────►
         │            │         │
         │            │         ├── Phase 8 (Sending + Streaming) ───────►
         │            │         │         │
         │            │         │         ├── Phase 9 (Session Mgmt) ────►
         │            │         │         │
         │            │         │         ├── Phase 11 (Nav Contexts) ───►
         │            │         │         │
         │            │         │         ├── Phase 12 (Empty States) ───►
         │            │         │         │
         │            │         │         └── Phase 13 (Integration) ────►
         │            │         │
         │            │         └── Phase 10 (Markdown Extensions) ──────►
```

## Critical Rules for Implementing Agents

1. **Match React source exactly** — copy CSS classes verbatim, match HTML structure
2. **No hacks, shortcuts, or mocks** — if streaming doesn't work, fix it properly
3. **Read the React source AND the Rust backend** before writing any code
4. **Use existing Leptos patterns** — look at `dashboards_list.rs`, `copilot_sidebar.rs` for examples
5. **Server functions call service layer directly** — don't HTTP-call REST endpoints
6. **Every task must end with `cargo check --workspace`** passing
7. **Don't skip features** — every checkbox, every badge, every empty state
8. **Use DESIGN_SYSTEM.md colors/classes** — check existing React CSS before inventing styles
