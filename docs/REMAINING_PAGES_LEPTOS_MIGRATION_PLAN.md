# Remaining Pages — Leptos Migration Plan

> Comprehensive, feature-complete migration of all remaining React pages to Leptos:
> - `/try` — Trial chat (anonymous)
> - `/onboarding` — Datasource onboarding
> - `/setup` — Personal mode setup wizard
> - `/connect/setup` — Kyomi Connect CLI setup
> - `/` — Home / landing redirect
> - `/welcome` — Post-signup terms acceptance
> - `/unsubscribe` — Email unsubscribe
> - `/accept-ownership/:transfer_id` — Workspace ownership transfer

---

## Page A: Trial Chat (`/try`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/Try.jsx` | ~30 | Wrapper with analytics + TrialCapabilitiesProvider |
| `components/TrialChat.jsx` | ~650 | Full trial chat UI — session init, messaging, limits |
| `api/trialApi.js` | ~80 | Trial-specific API client (no auth headers) |
| `lib/chartml/createTrialChartML.js` | ~120 | ChartML factory for trial (fixed sample datasource) |
| `context/TrialCapabilitiesProvider.jsx` | ~50 | Static capabilities context (all features disabled) |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/
├── pages/
│   └── trial/
│       ├── mod.rs                    # Module exports
│       └── trial_chat.rs             # Full trial chat page
└── server_fns/
    └── trial.rs                      # Trial server functions
```

**Backend** (already exists — no changes needed):
- `apps/server/src/routes/trial_chat.rs` (900 lines) — Session creation, chat, query execution
- `apps/server/src/routes/websocket.rs` — Trial WebSocket handler (HMAC-token auth, Redis pub/sub)

**Key differences from authenticated chat**:
- No JWT auth — uses IP-based session tokens stored in localStorage
- Fixed sample datasource (`acme-analytics`) — no datasource resolver
- Rate limited: 5 queries/session, 10/IP/day
- WebSocket: `/ws/trial/{sessionId}?token={hmac_token}` (Redis pub/sub, not user-based)
- No pin, save-to-dashboard, share, or session persistence
- Conversation history kept locally (last 10 exchanges)
- HTTP request/response flow (not fire-and-forget like authenticated chat)

### Phase A1: Trial Server Functions

#### Task A1.1: Trial Session + Chat Server Functions
**File**: `crates/kyomi-ui/src/server_fns/trial.rs` (new)
**Estimated lines**: ~200

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialSessionResponse {
    pub session_token: String,
    pub trial_access_token: String,
    pub expires_at: String,
    pub queries_remaining: i32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrialChatResponse {
    pub response: String,
    pub message_id: String,
    pub query_count: i32,
    pub queries_remaining: i32,
    pub thinking_events: Vec<serde_json::Value>,
    pub session_token: Option<String>,        // Refreshed token
    pub trial_access_token: Option<String>,   // Refreshed token
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub role: String,      // "user" or "assistant"
    pub content: String,
}
```

**Server functions**:
```rust
#[server] pub async fn create_trial_session() -> Result<TrialSessionResponse, ServerFnError>
#[server] pub async fn send_trial_message(
    message: String,
    conversation_history: Vec<ConversationEntry>,
    session_token: String,
    trial_access_token: String,
    current_time_user_tz: Option<String>,
) -> Result<TrialChatResponse, ServerFnError>
```

**Implementation**: Call the trial chat service directly. The trial endpoints are IP-based
rate limited — extract the client IP from the request for session creation.

**Critical**: The trial chat is a synchronous request/response — the server function
must wait for the agent to complete (unlike authenticated chat which is fire-and-forget).
The backend `trial_chat.rs` already handles this differently.

**Reference**: `apps/server/src/routes/trial_chat.rs` for exact logic.

**Verification**: `cargo check --workspace`

### Phase A2: Trial Chat Page

#### Task A2.1: Trial Chat — Core UI + Welcome State
**File**: `crates/kyomi-ui/src/pages/trial/trial_chat.rs`
**Estimated lines**: ~350 (this task)

**Implement**:
1. **Page layout** — full screen, no sidebar:
   - CSS: `h-screen flex flex-col bg-muted`
   - Header: Kyomi logo (dual light/dark), "Sample Data Explorer" label, queries remaining counter, "Sign Up Free" button
   - Messages area: scrollable, max-w-4xl centered
   - Input footer: text input + send button + query counter

2. **Session initialization** — on mount:
   - Call `create_trial_session()` server function
   - Store tokens in `localStorage` (via `web_sys::Storage`)
   - Update `queries_remaining` signal
   - Set `is_initializing = false`

3. **Welcome state** (no messages):
   - "Welcome to Kyomi" heading
   - Description of sample dataset (18 months, Acme Analytics)
   - Data availability grid (2 cols mobile, 4 cols desktop):
     - Subscriptions (MRR, plans, churn)
     - Users (signups, roles, activity)
     - Events (50k+)
     - Website Sessions (funnel, conversions)
   - 4 suggested question buttons (fixed text, wrapping layout)
   - Clicking suggestion submits as message

4. **State signals**:
   ```rust
   let (messages, set_messages) = signal(Vec::new());
   let (conversation_history, set_conversation_history) = signal(Vec::new());
   let (input_value, set_input_value) = signal(String::new());
   let (is_loading, set_is_loading) = signal(false);
   let (is_initializing, set_is_initializing) = signal(true);
   let (query_count, set_query_count) = signal(0);
   let (queries_remaining, set_queries_remaining) = signal(5);
   let (error, set_error) = signal(Option::<String>::None);
   let (show_signup_modal, set_show_signup_modal) = signal(false);
   ```

**Reference**: React `TrialChat.jsx` lines 1-200 (state + welcome UI).

**Verification**: `cargo check --workspace`

#### Task A2.2: Trial Chat — Message Send + Response
**File**: `crates/kyomi-ui/src/pages/trial/trial_chat.rs` (extend)
**Estimated lines**: ~250 (added)

**Implement**:
1. **Send message handler**:
   - Generate unique message ID: `trial-msg-{timestamp}-{counter}`
   - Add user message to messages list (optimistic)
   - Create placeholder assistant message (for thinking events)
   - Call `send_trial_message()` server function
   - On success: replace placeholder with full response, update query count
   - On 429: show signup modal (query limit reached)
   - On 401: clear tokens, prompt refresh
   - Update conversation history (keep last 10 exchanges)
   - Refresh tokens if included in response

2. **Trial WebSocket connection** (for thinking events):
   - Connect to `/ws/trial/{sessionId}?token={accessToken}`
   - Use `web_sys::WebSocket` directly (not the auth WebSocket provider)
   - Protocol: `wss:` for HTTPS, `ws:` for HTTP
   - Listen for `agent_thinking` and `token_usage_update` events
   - Merge WebSocket events with HTTP response events (WS preferred)
   - Graceful fallback: if WS fails, use events from HTTP response

3. **Message rendering**:
   - Reuse `ChatMessage` component from chat module with `is_trial_mode = true`
   - Pin/save buttons disabled in trial mode
   - Agent thinking display via `AgentThinking` component (variant: "header-bar")

4. **Auto-submit from URL**: Support `?q=question` query parameter
   - On mount, if `q` param present and not already submitted, auto-send

**Reference**: React `TrialChat.jsx` lines 200-500 (message handling).

**Verification**: `cargo check --workspace`, browser test: send trial message, see response.

#### Task A2.3: Trial Chat — Signup Modal + Reset + Chart Info
**File**: `crates/kyomi-ui/src/pages/trial/trial_chat.rs` (extend)
**Estimated lines**: ~150 (added)

**Implement**:
1. **Signup prompt modal** — shown when query limit reached:
   - Fixed overlay with `bg-black/50` backdrop
   - Modal: `max-w-md p-6 bg-card rounded-xl shadow-xl border`
   - "You've used all your trial queries" heading
   - Feature bullets for full version
   - "Sign Up Free" button → `/login`
   - NOT dismissible (clicking backdrop does nothing)

2. **Reset conversation button** — appears after first query:
   - Clears messages and conversation history
   - Does NOT reset query count (that's server-side)
   - Re-shows welcome state

3. **Chart info modal** — reuse existing `ChartInfoModal`:
   - Shows datasource, SQL query, full ChartML YAML
   - Copy buttons with clipboard support

4. **Analytics events** (if analytics system available in Leptos):
   - `trial_page_viewed`, `trial_message_sent`, `trial_signup_click`,
     `trial_limit_reached`, `trial_reset`

**Reference**: React `TrialChat.jsx` lines 500-650 (modal + reset + chart info).

**Verification**: `cargo check --workspace`, browser test: exhaust queries, see modal.

#### Task A2.4: Trial ChartML Configuration
**File**: `crates/kyomi-ui/src/pages/trial/trial_chat.rs` (extend) or separate utility
**Estimated lines**: ~50 (added)

**Implement**: Configure the Leptos MarkdownRenderer for trial mode:
1. All datasource slugs resolve to `acme-analytics`
2. SQL queries execute via trial query endpoint (server function)
3. `is_trial_mode = true` passed to MarkdownRenderer (disables actions)

**Note**: The Leptos MarkdownRenderer already uses `chartml-rs` for rendering. The
trial query execution path needs a server function:
```rust
#[server] pub async fn execute_trial_query(
    sql: String,
    trial_access_token: String,
    limit: Option<i64>,
) -> Result<QueryResult, ServerFnError>
```

**Reference**: React `createTrialChartML.js` for datasource resolution logic.

**Verification**: `cargo check --workspace`

---

## Page B: Datasource Onboarding (`/onboarding`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/DatasourceOnboarding.jsx` | 574 | Role-based routing, OAuth flows, sample data, credential setup |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/
├── pages/
│   └── onboarding/
│       ├── mod.rs
│       └── datasource_onboarding.rs   # Main onboarding page
└── server_fns/
    └── onboarding.rs                  # Onboarding server functions
```

**Backend** (already exists): Datasource endpoints in `apps/server/src/routes/datasources.rs`

### Phase B1: Onboarding Server Functions

#### Task B1.1: Onboarding Server Functions
**File**: `crates/kyomi-ui/src/server_fns/onboarding.rs` (new)
**Estimated lines**: ~200

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DatasourceInfo {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub datasource_type: String,
    pub connection_type: Option<String>,   // "direct" | "connect"
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CredentialStatusItem {
    pub datasource_id: String,
    pub datasource_name: String,
    pub datasource_type: String,
    pub slug: String,
    pub status: String,              // "valid" | "expired" | "missing" | "shared"
    pub auth_mode: Option<String>,   // "password" | "oauth" | "enterprise_oauth" | etc.
    pub needs_action: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OnboardingState {
    pub has_datasources: bool,
    pub is_admin: bool,
    pub sample_available: bool,
    pub needs_credentials: bool,
    pub credential_status: Vec<CredentialStatusItem>,
}
```

**Server functions**:
```rust
#[server] pub async fn get_onboarding_state() -> Result<OnboardingState, ServerFnError>
// Combines: GET /datasources, GET /datasources/credential-status, GET /datasources/sample/available

#[server] pub async fn create_sample_datasource() -> Result<(), ServerFnError>
// Calls: POST /datasources/sample

#[server] pub async fn get_oauth_connect_url(
    datasource_type: String,
    auth_mode: String,
    datasource_slug: Option<String>,
) -> Result<String, ServerFnError>
// Returns the correct OAuth redirect URL for the datasource type + auth mode
```

**Reference**: `apps/server/src/routes/datasources.rs` for credential-status and sample endpoints.

**Verification**: `cargo check --workspace`

### Phase B2: Onboarding Page

#### Task B2.1: Onboarding — Role-Based Routing + Choice Card
**File**: `crates/kyomi-ui/src/pages/onboarding/datasource_onboarding.rs`
**Estimated lines**: ~300 (this task)

**Implement the decision tree**:

1. **Loading state** — spinner while checking onboarding state

2. **Admin with no datasources** — "Binary choice card":
   - Option 1 (if sample available): "Explore with Sample Data"
     - Icon: Database + sparkle
     - Description: "Try Kyomi with a pre-loaded SaaS analytics dataset"
     - Action: call `create_sample_datasource()`, redirect to `/chat`
   - Option 2: "Connect Your Own Database"
     - Icon: Database + plug
     - Description: "Connect to BigQuery, Postgres, Snowflake, and more"
     - Action: open DatasourceModal in create mode (or navigate to `/settings/datasources`)
   - Card layout: `max-w-2xl w-full p-8`, centered, with divider between options

3. **Invited user with existing datasources needing credentials** — "Credential setup":
   - List each datasource needing credentials
   - Per-datasource row: icon + name + type badge + action button
   - OAuth datasources: "Connect [Provider]" button → opens OAuth popup
   - Password datasources: "Set up credentials" → navigates to `/settings/datasources`
   - All done: auto-redirect to `/chat`

4. **Non-admin with no datasources** — "Waiting for setup":
   - "Your workspace admin needs to connect a datasource"
   - "Contact your admin" message
   - Polling every 10 seconds (check if datasources appear)

5. **User with all datasources ready** — immediate redirect to `/chat`

**Reference**: React `DatasourceOnboarding.jsx` lines 1-574 (full page).

**Verification**: `cargo check --workspace`

#### Task B2.2: Onboarding — OAuth Popup Handling
**File**: `crates/kyomi-ui/src/pages/onboarding/datasource_onboarding.rs` (extend)
**Estimated lines**: ~200 (added)

**Implement OAuth popup flow**:

1. **Open OAuth popup**:
   - Compute URL based on datasource type + auth mode:
     - BigQuery kyomi_oauth: `/api/v1/auth/google-oauth/connect`
     - BigQuery enterprise_oauth: `/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=X`
     - Snowflake: `/api/v1/auth/oauth/snowflake/connect?datasource_slug=X`
     - Synapse: `/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug=X`
     - Databricks: `/api/v1/auth/oauth/databricks/connect?datasource_slug=X`
   - Open centered popup window (500x600)
   - Use `web_sys::Window::open()`

2. **Listen for popup messages**:
   - `web_sys::Window::add_event_listener("message")`
   - Handle message types:
     - `GOOGLE_OAUTH_SUCCESS` / `GOOGLE_OAUTH_ERROR`
     - `BIGQUERY_ENTERPRISE_OAUTH_SUCCESS` / `BIGQUERY_ENTERPRISE_OAUTH_ERROR`
     - `SNOWFLAKE_OAUTH_SUCCESS` / `SNOWFLAKE_OAUTH_ERROR`
     - `MICROSOFT_ENTERPRISE_OAUTH_SUCCESS` / `MICROSOFT_ENTERPRISE_OAUTH_ERROR`
     - `DATABRICKS_OAUTH_SUCCESS` / `DATABRICKS_OAUTH_ERROR`
   - On success: re-check credential status, show toast
   - On error: show error toast

3. **Monitor popup closure** — poll every 500ms via `gloo-timers::callback::Interval`,
   clear `oauth_connecting` state when popup closes

4. **Cleanup**: Remove event listeners on unmount

**Reference**: React `DatasourceOnboarding.jsx` OAuth handler logic.

**Verification**: `cargo check --workspace`, browser test: click OAuth connect, complete flow.

---

## Page C: Personal Setup Wizard (`/setup`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/PersonalSetupWizard.jsx` | 281 | Two-step wizard: Connect Data → Connect AI Tool |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/pages/
└── setup/
    ├── mod.rs
    └── personal_setup.rs             # Setup wizard page
```

### Phase C1: Personal Setup Wizard

#### Task C1.1: Setup Wizard — Full Page
**File**: `crates/kyomi-ui/src/pages/setup/personal_setup.rs`
**Estimated lines**: ~350

**Implement the two-step wizard**:

**Step 1: Connect Data** (shown when no datasources):
- Card with two options:
  1. "Connect a Database" → navigates to `/onboarding`
  2. "Explore with Sample Data" → navigates to `/`
- Each option: icon + title + description + button

**Step 2: Connect Your AI Tool** (shown when has datasources):
- Tabbed interface with 3 tabs: Claude Code | Claude Desktop | Cursor

- **Claude Code tab**:
  - CLI command: `claude mcp add --transport http kyomi {mcpUrl}`
  - Config JSON block with copy button
  - MCP URL computed from `window.location.port` or `3000`

- **Claude Desktop tab**:
  - Config file path: `claude_desktop_config.json`
  - JSON config block with copy button

- **Cursor tab**:
  - "One-click install" deep link button: `cursor://anysphere.cursor-deeplink/mcp/install?name=kyomi&config={base64}`
  - Manual config: `.cursor/mcp.json` with copy button

- **Final actions**:
  - "I've Connected" button → navigates to `/dashboards`
  - "Or use Kyomi's built-in chat instead" link → navigates to `/settings`

**Dynamic URL generation**:
```rust
let port = window.location().port().unwrap_or("3000".into());
let mcp_url = format!("http://localhost:{port}/mcp");
```

**Copy button pattern**: Use `web_sys::Clipboard::write_text()`, show "Copied!" feedback.

**Server function needed**:
```rust
#[server] pub async fn check_has_datasources() -> Result<bool, ServerFnError>
// Simple check: any datasources in workspace?
```

**Reference**: React `PersonalSetupWizard.jsx` (281 lines) for exact layout and content.

**Verification**: `cargo check --workspace`, browser test: see correct step based on datasource state.

---

## Page D: Connect Setup (`/connect/setup`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/ConnectSetupPage.jsx` | 443 | CLI integration — select/create datasource, generate token, deliver to CLI |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/pages/
└── connect_setup/
    ├── mod.rs
    └── connect_setup_page.rs         # Connect CLI setup page
```

### Phase D1: Connect Setup Server Functions

#### Task D1.1: Connect Setup Server Functions
**File**: `crates/kyomi-ui/src/server_fns/onboarding.rs` (extend — shared with onboarding)
**Estimated lines**: ~150 (added)

**Server functions**:
```rust
#[server] pub async fn list_connect_datasources() -> Result<Vec<DatasourceInfo>, ServerFnError>
// Lists datasources filtered by connection_type == "connect"

#[server] pub async fn create_connect_datasource(
    name: String,
    slug: Option<String>,
    datasource_type: String,    // postgres, mysql, clickhouse, sqlserver, redshift
) -> Result<CreateConnectResult, ServerFnError>

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateConnectResult {
    pub datasource_id: String,
    pub connect_token: String,
}

#[server] pub async fn rotate_connect_token(
    datasource_id: String,
) -> Result<String, ServerFnError>    // Returns new token
```

**Reference**: `apps/server/src/routes/datasources.rs` for connect token endpoints.

**Verification**: `cargo check --workspace`

### Phase D2: Connect Setup Page

#### Task D2.1: Connect Setup — Select + Create Steps
**File**: `crates/kyomi-ui/src/pages/connect_setup/connect_setup_page.rs`
**Estimated lines**: ~400 (this task)

**Implement the multi-step flow**:

1. **Admin check** — non-admins see "Admin Access Required" message with explanation

2. **Query parameters**:
   - `callback_port` — CLI callback port for token delivery
   - `state` — CLI state token for callback verification
   - Both extracted via `leptos_router::hooks::use_query_map()`

3. **Step 1: Select Datasource**:
   - List existing Connect datasources with icon + name + type
   - Click generates token via `rotate_connect_token()`
   - "or" divider if datasources exist
   - "Create new datasource" button

4. **Step 2: Create Datasource**:
   - Name input (required, autofocus)
   - Slug input (shown only after 409 conflict, with auto-generation helper)
   - Database type grid (5 buttons): postgres, mysql, clickhouse, sqlserver, redshift
   - "Create & Generate Token" submit button
   - Back button to Step 1
   - Slug auto-generation: lowercase, replace spaces with hyphens, strip non-alphanumeric

5. **Step 3: Success — Token Display**:
   - Token in copyable code block
   - **If has callback**: Background fetch to `http://127.0.0.1:{port}/callback?token={token}&state={state}`
     - Delivery states: pending → delivered (green) or failed (show manual instructions)
     - Use `web_sys::Request` or `gloo_net::http::Request` for the callback
   - **If no callback**: Show manual installation command:
     ```
     curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token "<token>"
     ```
   - Alternative: `kyomi-connect setup --token <TOKEN>`

**State**:
```rust
enum SetupStep { Select, Create, Success }
let (step, set_step) = signal(SetupStep::Select);
let (datasources, set_datasources) = signal(Vec::new());
let (token, set_token) = signal(Option::<String>::None);
let (delivery_status, set_delivery_status) = signal(Option::<DeliveryStatus>::None);
let (new_name, set_new_name) = signal(String::new());
let (new_type, set_new_type) = signal("postgres".to_string());
let (show_slug, set_show_slug) = signal(false);
let (new_slug, set_new_slug) = signal(String::new());
```

**Reference**: React `ConnectSetupPage.jsx` (443 lines) for exact step logic.

**Verification**: `cargo check --workspace`, browser test: create datasource, see token.

---

## Page E: Home / Landing Redirect (`/`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `components/LandingRedirect.jsx` | ~80 | Dynamic redirect based on user's landing_page preference |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/pages/
└── home.rs                           # Landing redirect component
```

### Phase E1: Landing Redirect

#### Task E1.1: Home Page — Dynamic Redirect
**File**: `crates/kyomi-ui/src/pages/home.rs`
**Estimated lines**: ~120

**Implement the redirect router**:

1. **Fetch user preference** — via existing user context or server function:
   - Read `user.extra_metadata.landing_page` preference
   - Values: `"chat"` (default), `"watches"`, `"sql_editor"`, `"dashboards"`

2. **Routing logic**:
   - `"chat"` → render `ChatPage` component directly (not a redirect — matches React behavior)
   - `"watches"` → redirect to `/watches`
   - `"sql_editor"` → redirect to `/sql-editor`
   - `"dashboards"` → resolve default dashboard:
     1. Check user's `default_dashboard_id` → redirect to `/dashboard/{id}`
     2. If none, check workspace default via server function → redirect to `/dashboard/{id}`
     3. If none, redirect to `/dashboards` (list page)

3. **Personal mode special case**:
   - If personal mode + no LLM configured → default to dashboards instead of chat

**Server function needed**:
```rust
#[server] pub async fn get_landing_config() -> Result<LandingConfig, ServerFnError>

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LandingConfig {
    pub landing_page: String,                    // "chat", "watches", "sql_editor", "dashboards"
    pub user_default_dashboard_id: Option<String>,
    pub workspace_default_dashboard_id: Option<String>,
    pub is_personal_mode: bool,
    pub llm_configured: bool,
}
```

**Use `navigate()` with `replace: true`** for all redirects (don't add to browser history).

**Reference**: React `LandingRedirect.jsx` for exact routing logic.

**Verification**: `cargo check --workspace`, browser test: navigate to `/`, verify correct redirect.

---

## Page F: Welcome (`/welcome`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/Welcome.jsx` | ~150 | Post-signup terms acceptance with consent checkboxes |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/pages/
└── welcome.rs                        # Terms acceptance page
```

### Phase F1: Welcome Page

#### Task F1.1: Welcome — Terms Acceptance
**File**: `crates/kyomi-ui/src/pages/welcome.rs`
**Estimated lines**: ~200

**Implement**:

1. **Layout** — full-screen centered card, no sidebar:
   - Container: `min-h-screen flex items-center justify-center bg-background p-4`
   - Card: `max-w-2xl w-full p-8`

2. **Query parameter extraction**:
   - `temp_token` (required) — temporary token from signup flow
   - `existing_user` (optional) — shows "Welcome Back!" vs "Welcome to Kyomi!"
   - If no `temp_token`: redirect to `/login`

3. **Heading** — conditional:
   - Existing user: "Welcome Back!"
   - New user: "Welcome to Kyomi!"
   - Subtitle explaining terms requirement

4. **Terms links**:
   - "Terms of Service" — external link (opens new tab)
   - "Privacy Policy" — external link (opens new tab)

5. **Checkboxes**:
   - "I agree to the Terms of Service and Privacy Policy" (required)
   - "I'd like to receive product updates and news" (optional, marketing consent)
   - CSS: `flex items-start space-x-3 cursor-pointer`

6. **Submit button**: "Continue to Kyomi"
   - Disabled until terms checkbox checked
   - Shows loading spinner during submit
   - Full width

7. **Server function**:
   ```rust
   #[server] pub async fn accept_terms(
       temp_token: String,
       marketing_consent: bool,
   ) -> Result<(), ServerFnError>
   // Calls: POST /api/v1/auth/accept-terms
   // Sets auth cookies via response headers
   ```

8. **On success**: Hard redirect to `/onboarding` via `window.location.set_href()`
   (full page reload needed to pick up new auth cookies)

9. **On error**: Show error alert, allow retry

**Reference**: React `Welcome.jsx` for exact layout and flow.

**Verification**: `cargo check --workspace`

---

## Page G: Unsubscribe (`/unsubscribe`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/Unsubscribe.jsx` | ~120 | Public email unsubscribe form |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/pages/
└── unsubscribe.rs                    # Email unsubscribe page
```

### Phase G1: Unsubscribe Page

#### Task G1.1: Unsubscribe — Form + Success State
**File**: `crates/kyomi-ui/src/pages/unsubscribe.rs`
**Estimated lines**: ~150

**Implement**:

1. **Layout** — full-screen centered, no sidebar, no auth:
   - Container: `min-h-screen bg-background flex items-center justify-center p-8`
   - Content: `max-w-md` centered

2. **Kyomi logo** — theme-aware:
   - Light mode: light logo, `dark:hidden`
   - Dark mode: dark logo, `hidden dark:block`
   - Size: `h-12 mx-auto mb-6`

3. **Heading**: "Unsubscribe" + subtitle "We're sorry to see you go"

4. **Email input**:
   - Pre-filled from `?email=` query parameter
   - CSS: `w-full px-4 py-3.5 bg-muted border border-border rounded-xl`
   - Focus: `focus:ring-2 focus:ring-primary focus:border-transparent`

5. **Unsubscribe button**: disabled if email empty, shows spinner during submit

6. **"Never mind" link**: navigates to `/`

7. **Server function** (public, no auth):
   ```rust
   #[server] pub async fn unsubscribe_email(email: String) -> Result<(), ServerFnError>
   // Calls: POST /api/v1/unsubscribe with { email }
   // Public endpoint — no auth extraction needed
   ```

8. **Success state**:
   - Green success alert: "You've been unsubscribed from marketing emails"
   - "Return to homepage" link

9. **Error state**: Red error alert with error message, can retry

**Reference**: React `Unsubscribe.jsx` for exact styling.

**Verification**: `cargo check --workspace`

---

## Page H: Accept Ownership (`/accept-ownership/:transfer_id`)

### Architecture Overview

**Source Files (React)**
| File | Lines | Purpose |
|------|-------|---------|
| `pages/AcceptOwnershipPage.jsx` | ~300 | Multi-state ownership transfer acceptance |

**Target Files (Leptos)**
```
crates/kyomi-ui/src/pages/
└── accept_ownership.rs               # Ownership transfer page
```

### Phase H1: Accept Ownership Server Functions

#### Task H1.1: Ownership Transfer Server Functions
**File**: `crates/kyomi-ui/src/server_fns/onboarding.rs` (extend)
**Estimated lines**: ~100 (added)

**Types**:
```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OwnershipTransfer {
    pub transfer_id: String,
    pub workspace_name: String,
    pub from_user_email: String,
    pub expires_at: String,
    pub status: String,         // "pending" | "accepted" | "declined"
}
```

**Server functions**:
```rust
#[server] pub async fn get_ownership_transfer(
    transfer_id: String,
) -> Result<Option<OwnershipTransfer>, ServerFnError>
// Fetches all pending transfers, filters by transfer_id, checks status == "pending"

#[server] pub async fn accept_ownership_transfer(
    transfer_id: String,
) -> Result<(), ServerFnError>

#[server] pub async fn decline_ownership_transfer(
    transfer_id: String,
) -> Result<(), ServerFnError>
```

**Reference**: `apps/server/src/routes/workspaces.rs` for ownership transfer endpoints.

**Verification**: `cargo check --workspace`

### Phase H2: Accept Ownership Page

#### Task H2.1: Accept Ownership — Multi-State Page
**File**: `crates/kyomi-ui/src/pages/accept_ownership.rs`
**Estimated lines**: ~350

**Implement 5-state page**:

1. **Loading state**:
   - Spinner + "Loading transfer details..."
   - Fetch transfer via `get_ownership_transfer(transfer_id)` server function

2. **Error state** (transfer not found, expired, or already processed):
   - Alert icon in red circle
   - Error message
   - "Go to Dashboard" button → navigates to `/`

3. **Ready state** (transfer details loaded):
   - **Header**: Large icon (`ArrowRightLeft`) in primary-tinted circle
   - **Title**: "Ownership Transfer Request"
   - **Transfer info card** (`bg-muted/50 rounded-xl p-6 border`):
     - Workspace name (with Building2 icon)
     - Current owner email (with User icon)
     - Expiration date
   - **Warning alert** (variant: warning):
     - "You will become the workspace owner. The current owner will become an admin."
   - **Capabilities list** (5 items with CheckCircle icons):
     - Manage billing and subscription
     - Transfer ownership to others
     - Delete the workspace
     - Manage all team members
     - Full administrative control
   - **Action buttons**:
     - "Accept Ownership" — primary, full width, shows spinner during processing
     - "Decline" — outline variant, shows spinner during processing

4. **Processing state**: Buttons show spinners, disabled

5. **Success state**:
   - Green checkmark icon
   - "Ownership Transfer Complete"
   - Description: "You are now the owner of [workspace]"
   - Auto-redirect to `/settings/team` after 3 seconds

**Toast messages**: Show success/error toasts (use existing toast component)

**Layout**:
- Container: `min-h-screen bg-gradient-to-br from-background via-muted/30 to-muted/50`
- Main card: `bg-card/80 backdrop-blur-sm rounded-2xl shadow-xl border border-border`
- Centered: `flex items-center justify-center p-4`
- Max width: `max-w-lg w-full`
- Footer: small text with support email

**Reference**: React `AcceptOwnershipPage.jsx` for exact layout, states, and styling.

**Verification**: `cargo check --workspace`, browser test: load valid/invalid transfer ID.

---

## Phase Z: Route Wiring + Integration

> This phase wires all remaining pages into the router and verifies end-to-end.

### Task Z.1: Module Setup + Route Registration
**Files to create**: `mod.rs` for each new page module (trial, onboarding, setup, connect_setup)
**Files to modify**: `pages/mod.rs`, `app.rs`, `lib.rs`

**Route updates in `app.rs`**:
```rust
// Public pages — NO layout, NO auth
<Route path=path!("/try") view=TrialChatPage/>
<Route path=path!("/welcome") view=WelcomePage/>
<Route path=path!("/unsubscribe") view=UnsubscribePage/>

// Flow pages — NO sidebar, requires auth
<Route path=path!("/onboarding") view=DatasourceOnboardingPage/>
<Route path=path!("/onboarding/catalog") view=|| view! { <Redirect path="/onboarding"/> }/>
<Route path=path!("/setup") view=PersonalSetupPage/>
<Route path=path!("/connect/setup") view=ConnectSetupPage/>
<Route path=path!("/accept-ownership/:transfer_id") view=AcceptOwnershipPage/>

// Home — wrapped in Layout
<Route path=path!("/") view=|| view! { <Layout><HomePage/></Layout> }/>
```

Remove all `NotImplementedPage` references for these routes.

**Register all new server functions** in `lib.rs`.

**Verification**: `cargo check --workspace`

### Task Z.2: End-to-End Test Matrix
**No file changes — verification only.**

| Test Case | Route | Steps | Expected |
|-----------|-------|-------|----------|
| Trial — welcome | `/try` | Load page | Welcome state with sample data grid + 4 suggestions |
| Trial — send message | `/try` | Click suggestion | Message sent, AI response returned |
| Trial — query limit | `/try` | Send 5 messages | Signup modal appears, not dismissible |
| Trial — reset | `/try` | Click "Reset conversation" | Messages cleared, welcome shown |
| Trial — chart render | `/try` | Ask for chart | ChartML chart renders from sample data |
| Trial — URL submit | `/try?q=What is MRR` | Load page | Auto-submits question |
| Onboarding — admin no DS | `/onboarding` | Admin, no datasources | Choice card: sample vs connect |
| Onboarding — sample data | `/onboarding` | Click "Explore with Sample" | Sample created, redirect to `/chat` |
| Onboarding — invited user | `/onboarding` | Non-admin, DS needs creds | Credential setup list shown |
| Onboarding — OAuth connect | `/onboarding` | Click "Connect Google" | OAuth popup opens, returns success |
| Onboarding — all ready | `/onboarding` | All credentials valid | Redirect to `/chat` |
| Setup — step 1 | `/setup` | No datasources | Shows Connect Data options |
| Setup — step 2 | `/setup` | Has datasources | Shows AI tool tabs (Claude Code/Desktop/Cursor) |
| Setup — copy config | `/setup` | Click copy button | Config copied to clipboard |
| Connect — admin | `/connect/setup` | Admin user | Datasource list or create form |
| Connect — non-admin | `/connect/setup` | Non-admin user | "Admin Access Required" |
| Connect — create DS | `/connect/setup` | Create new datasource | Token generated and displayed |
| Connect — CLI callback | `/connect/setup?callback_port=X` | With callback | Token delivered to CLI |
| Connect — 409 slug | `/connect/setup` | Name conflict | Slug field appears |
| Home — chat default | `/` | Landing = chat | Chat page renders |
| Home — dashboards | `/` | Landing = dashboards | Redirect to dashboard |
| Home — watches | `/` | Landing = watches | Redirect to `/watches` |
| Welcome — new user | `/welcome?temp_token=X` | New signup | Terms checkboxes, "Welcome to Kyomi!" |
| Welcome — existing | `/welcome?temp_token=X&existing_user=1` | Existing user | "Welcome Back!" heading |
| Welcome — no token | `/welcome` | No temp_token | Redirect to `/login` |
| Welcome — submit | `/welcome` | Check terms, click Continue | Redirect to `/onboarding` |
| Unsubscribe — form | `/unsubscribe` | Load page | Email input form |
| Unsubscribe — prefill | `/unsubscribe?email=a@b.com` | With email param | Email pre-filled |
| Unsubscribe — submit | `/unsubscribe` | Enter email, click Unsubscribe | Success message shown |
| Ownership — valid | `/accept-ownership/X` | Valid pending transfer | Transfer details + Accept/Decline |
| Ownership — accept | `/accept-ownership/X` | Click Accept | Success, redirect to `/settings/team` |
| Ownership — decline | `/accept-ownership/X` | Click Decline | Redirect to `/` |
| Ownership — invalid | `/accept-ownership/bad` | Invalid transfer ID | Error state with "Go to Dashboard" |

---

## Estimated Totals

| Page | Tasks | Est. New Lines | Files Created | Files Modified |
|------|-------|---------------|---------------|----------------|
| A. Trial Chat | 5 | ~1,000 | 3 | 0 |
| B. Onboarding | 3 | ~700 | 3 | 0 |
| C. Personal Setup | 1 | ~350 | 2 | 0 |
| D. Connect Setup | 2 | ~550 | 2 | 1 |
| E. Home Redirect | 1 | ~120 | 1 | 0 |
| F. Welcome | 1 | ~200 | 1 | 0 |
| G. Unsubscribe | 1 | ~150 | 1 | 0 |
| H. Accept Ownership | 2 | ~450 | 1 | 1 |
| Z. Integration | 2 | ~100 | 4 | 3 |
| **Total** | **18** | **~3,620** | **18** | **5** |

## Task Execution Order

Pages are independent — all can run in parallel. Within each page, tasks are sequential.

```
Phase A (Trial Chat) ───────────────────────────────────────────────────►
Phase B (Onboarding) ───────────────────────────────────────────────────►
Phase C (Personal Setup) ──────────────────────────────────────────────►
Phase D (Connect Setup) ────────────────────────────────────────────────►
Phase E (Home Redirect) ────────────────────────────────────────────────►
Phase F (Welcome) ──────────────────────────────────────────────────────►
Phase G (Unsubscribe) ──────────────────────────────────────────────────►
Phase H (Accept Ownership) ─────────────────────────────────────────────►
                              │
                              └── Phase Z (Integration + Testing) ──────►
```

**Dependencies on other migration plans**:
- Trial Chat (A) depends on Chat migration (Phase 1 WebSocket, Phase 4 AgentThinking component)
- Home Redirect (E) depends on Chat migration (renders ChatPage for default landing)
- Onboarding (B) may reference DatasourceModal from Settings (already migrated)

## Critical Rules for Implementing Agents

1. **Match React source exactly** — copy CSS classes verbatim, match HTML structure
2. **No hacks, shortcuts, or mocks** — if OAuth popup doesn't work, fix it properly
3. **Read the React source AND the Rust backend** before writing any code
4. **Use existing Leptos patterns** — look at auth pages for public routes, settings for forms
5. **Server functions call service layer directly** — don't HTTP-call REST endpoints
6. **Every task must end with `cargo check --workspace`** passing
7. **Don't skip features** — every state, every error case, every redirect
8. **Public pages (trial, unsubscribe) must NOT require auth** — no `extract_auth()` call
9. **OAuth popup flow is critical** — test with real OAuth providers, not mocks
10. **Token delivery callback (Connect Setup) uses localhost fetch** — handle CORS/failure gracefully
