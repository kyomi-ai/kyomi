# Leptos Settings Page — Implementation Plan

**Status:** Approved for implementation
**Branch:** `poc/leptos-settings-profile`
**Date:** 2026-03-20

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

### Rule 5: Update the settings shell
When a tab's UI is complete, update `settings_shell.rs` so the tab links to
the Leptos route instead of the React fallback.

### Rule 6: Build and verify
After every change:
1. `cargo check -p kyomi-ui --features ssr` — server compiles
2. `cd crates/kyomi-ui && trunk build --public-url /leptos/` — WASM compiles
3. `touch apps/server/src/leptos_frontend.rs && cargo build -p kyomi-server` — server embeds new WASM

### Rule 7: Quality over speed
There is no deadline. It is more important to get it right than to get it done quickly.
Read the React source. Match it exactly. Review your own work before calling it done.

---

## Phase 1: Shared Component Library + Infrastructure

Build all shared UI components that multiple tabs depend on. Each component
must match its React counterpart in `apps/frontend/src/components/ui/` exactly.

### Task 1.1: UserContext server function
**Creates:** `crates/kyomi-ui/src/server_fns/context.rs`
**React reference:** Data scattered across AuthContext, CapabilitiesContext, SystemConfigContext

Server function `get_user_context()` returning:
```rust
pub struct UserContext {
    pub user_id: String,
    pub email: String,
    pub name: Option<String>,
    pub workspace_id: Option<String>,
    pub workspace_name: Option<String>,
    pub workspace_roles: Vec<String>,  // ["workspace_admin", "user", etc.]
    pub is_owner: bool,
    pub subscription_tier: String,     // "free", "team", "enterprise"
    pub is_personal_mode: bool,
    pub is_self_hosted: bool,
    pub billing_enabled: bool,
    pub capabilities: HashMap<String, bool>,  // feature flags
}
```
This replaces 3 separate React contexts. Every settings tab uses it for role-based visibility.

**Acceptance criteria:**
- Server function compiles and is registered in lib.rs
- Unit test: construct test AppState, call function, verify fields populated
- Provide via Leptos context at settings shell level so all tabs can access it

### Task 1.2: Badge component
**Creates:** `crates/kyomi-ui/src/components/badge.rs`
**React reference:** `apps/frontend/src/components/ui/badge.jsx`
**Read the React file. Copy the variant classes exactly.**

Variants: default, secondary, destructive, outline.
Used by: Security (sessions, passkeys), Data Sources, Team, Billing.

### Task 1.3: Alert component
**Creates:** `crates/kyomi-ui/src/components/alert.rs`
**React reference:** `apps/frontend/src/components/ui/alert.jsx`
**Read the React file. Copy the variant classes exactly.**

Variants: default, warning, error, success, info.
Sub-components: Alert, AlertTitle, AlertDescription.
Used by: Security, Workspace, Data Sources, Billing, Team.

### Task 1.4: StatusBadge component
**Creates:** `crates/kyomi-ui/src/components/status_badge.rs`
**React reference:** `apps/frontend/src/components/ui/status-badge.jsx`
**Read the React file. Copy the variant classes exactly.**

Variants: default, warning, error, success, info.
Used by: TwoFactorAuth, SessionManagement, Billing.

### Task 1.5: Modal component
**Creates:** `crates/kyomi-ui/src/components/modal.rs`
**React reference:** `apps/frontend/src/components/Modal.jsx`
**Read the React file. Match the backdrop, sizes, close button, header/content/footer structure.**

Sizes: sm (384px), md (448px), lg (896px), xl (1152px), full (95vw).
Used by: Billing (plan change, checkout), Team (invite, transfer), Passkeys (rename).

### Task 1.6: Switch component
**Creates:** `crates/kyomi-ui/src/components/switch.rs`
**React reference:** `apps/frontend/src/components/ui/switch.jsx` (if exists, otherwise check Radix)
Used by: Data Sources (enable/disable toggle).

### Task 1.7: Skeleton loading component
**Creates:** `crates/kyomi-ui/src/components/skeleton.rs`
**React reference:** `apps/frontend/src/components/ui/skeleton.jsx`
**Read the React file. Copy the animation classes.**

Used by: Data Sources (loading state).

### Task 1.8: Tooltip component
**Creates:** `crates/kyomi-ui/src/components/tooltip.rs`
**React reference:** `apps/frontend/src/components/ui/tooltip.jsx`

Note: Radix Tooltip is complex (positioning, delays, portal). For the Leptos version,
implement a simple CSS-based tooltip using `group` + `group-hover:visible` pattern.
Match the visual appearance (bg-popover, border, shadow, text-sm).
Used by: Sidebar, ProfileSettings, Security, Data Sources.

---

## Phase 2: Complete Profile Tab

Finish the 4 remaining Profile sections. The Profile tab is already partially built.

### Task 2.1: MCP Connection card
**React reference:** `apps/frontend/src/components/settings/ProfileSettings.jsx` lines ~1000-1080
**Read the section.** It shows MCP server URLs and connection info for Claude Code/Desktop.

No server function needed — the MCP URL is derived from the frontend URL config.
Uses: Card, copy button pattern, code snippet display.

### Task 2.2: AI Provider Settings card
**React reference:** `apps/frontend/src/components/settings/AIProviderSettings.jsx` (197 lines)
**Read the entire file.**

Uses localStorage only — no server functions. Needs password input with show/hide toggle.
Provider select (Anthropic/OpenAI/Gemini), model override, base URL override, API key.
All `web_sys::window().local_storage()` access gated behind `#[cfg(target_arch = "wasm32")]`.

### Task 2.3: Slack Connection section (server functions)
**React reference:** `apps/frontend/src/components/settings/ProfileSettings.jsx` lines 160-300
**Read the Slack-related state, effects, and handlers.**

Server functions needed (gated behind `#[cfg(feature = "slack")]` on kyomi-ui):
- `get_slack_status()` → SlackStatus
- `slack_connect()` → String (OAuth URL)
- `slack_disconnect()`
- `get_slack_channels()` → Vec<SlackChannel>
- `get_default_watch_channel()` → Option<WatchChannel>
- `set_default_watch_channel(channel_id, channel_name)`

These call into `enterprise/kyomi-slack` service layer.

### Task 2.4: Slack Connection section (UI) + Push Notifications
**Depends on:** Task 2.3
**React reference:** ProfileSettings.jsx Slack section (~lines 600-900) and Push section (~lines 900-1000)

Slack: connect/disconnect buttons, channel selector, default watch channel.
Push: browser Push API via `web_sys`, subscription management.
Both hidden in personal mode (`is_personal_mode`).

---

## Phase 3: Security Tab

4 sub-components. Each is a separate task with its own server functions.

### Task 3.1: Password Manager
**React reference:** `apps/frontend/src/components/PasswordManager.jsx` (250 lines)
**Read the entire file.**

Server functions:
- `change_password(current_password, new_password)`
- `set_password(new_password)` (for users without a password)
- `has_password()` → bool

UI: Card with either "Set Password" or "Change Password" form.
Needs: password input with show/hide toggle (build as shared component if not done in Phase 1).

### Task 3.2: Two-Factor Authentication
**React reference:** `apps/frontend/src/components/TwoFactorAuth.jsx` (307 lines)
**Read the entire file.**

Server functions:
- `get_totp_status()` → TotpStatus { enabled: bool }
- `setup_totp()` → TotpSetup { secret, qr_uri }
- `enable_totp(code: String)`
- `disable_totp(code: String)`

UI: Card showing enabled/disabled status, setup flow with QR code, verification code input.
QR code: use the `qr_uri` returned by setup — can render as an `<img>` with a data URI.

### Task 3.3: Session Management
**React reference:** `apps/frontend/src/components/SessionManagement.jsx` (352 lines)
**Read the entire file.**

Server functions:
- `get_sessions()` → Vec<SessionInfo> (device, IP, last active, current flag)
- `revoke_session(session_id: String)`
- `logout_all_sessions()`

UI: Card with list of active sessions, device icons, "this device" badge, revoke buttons.

### Task 3.4: Passkey Manager
**React reference:** `apps/frontend/src/components/PasskeyManager.jsx` (441 lines)
AND `apps/frontend/src/utils/passkeys.js` (369 lines)
**Read BOTH files.**

This is the most complex Security component — uses WebAuthn browser API.

Server functions:
- `list_passkeys()` → Vec<PasskeyInfo>
- `start_passkey_registration()` → RegistrationOptions (JSON)
- `complete_passkey_registration(credential: String)`
- `delete_passkey(credential_id: String)`
- `rename_passkey(credential_id: String, name: String)`

WebAuthn browser calls via `web_sys`:
- `navigator.credentials.create()` for registration
- Response serialization to send back to server

UI: Card with passkey list, add/delete/rename, device detection icons.

---

## Phase 4: Simple Admin Tabs

### Task 4.1: Usage Panel
**React reference:** `apps/frontend/src/components/UsagePanel.jsx` (266 lines)
**Read the entire file.**

Server function:
- `get_ai_usage_status()` → UsageData (monthly usage, limit, breakdown by user and feature)

UI: Card with usage bars (CSS-only horizontal stacked bars), user breakdown table.
Simplest standalone tab — good warmup for admin pages.

### Task 4.2: Workspace Settings (server functions)
**React reference:** `apps/frontend/src/components/settings/WorkspaceSettings.jsx` (394 lines)
**Read the entire file — note the API calls and their parameters.**

Server functions:
- `get_workspace_settings()` → WorkspaceSettingsData
- `update_workspace_settings(name, default_model, chartml_config)`
- `populate_knowledge_graph()` (triggers background rebuild)

Slack-related server functions (if not already built in Task 2.3):
- `get_slack_workspace_status()`
- `slack_install()` → String (install URL)
- `slack_uninstall(team_id: String)`

### Task 4.3: Workspace Settings (UI)
**Depends on:** Task 4.2
**React reference:** Same file (WorkspaceSettings.jsx)

UI: Workspace name edit, default model select, chart palette selector (workspace level),
Slack workspace integration section, knowledge graph rebuild button.

### Task 4.4: Analytics Settings
**React reference:** `apps/frontend/src/components/settings/AnalyticsSettings.jsx` (438 lines)
**Read the entire file.**

Server functions:
- `get_analytics_usage()` → AnalyticsUsage
- `list_analytics_sites()` → Vec<AnalyticsSite>
- `create_analytics_site(name, domain)`
- `update_analytics_site(site_id, name, domain)`
- `delete_analytics_site(site_id)`

UI: Sites list with inline create/edit form, tracking snippet with copy button,
usage stats, catalog datasource link.

---

## Phase 5: Complex Admin Tabs

### Task 5.1: Team Management (server functions)
**React reference:** `apps/frontend/src/components/settings/TeamManagement.jsx` (569 lines)
**Read the entire file.**

Server functions:
- `list_workspace_members()` → Vec<Member>
- `list_workspace_invitations()` → Vec<Invitation>
- `invite_member(email, role)`
- `cancel_invitation(invitation_id)`
- `update_member_role(user_id, role)`
- `remove_member(user_id)`
- `list_ownership_transfers()` → Vec<Transfer>
- `cancel_ownership_transfer(transfer_id)`

### Task 5.2: Team Management (UI)
**Depends on:** Task 5.1
**React reference:** Same file (TeamManagement.jsx)

UI: Members table with role select, invite form, pending invitations list,
ownership transfer section. Needs Modal (for invite), ConfirmDialog (for remove).

Note: The React version subscribes to WebSocket `ownership_transfer_offered` events.
For the Leptos POC, skip WebSocket subscription — the ownership transfer list
can be loaded on page mount and refreshed manually.

### Task 5.3: Data Sources (server functions)
**React reference:** `apps/frontend/src/components/settings/DatasourceSettings.jsx` (790 lines)
**Read the entire file — note all API calls including OAuth.**

Server functions:
- `list_datasources()` → Vec<DatasourceInfo>
- `get_datasource_types()` → Vec<DatasourceType>
- `get_credential_status()` → Vec<CredentialStatus>
- `get_catalog_status(datasource_id)` → CatalogStatus
- `toggle_datasource(datasource_id, enabled: bool)`
- `delete_datasource(datasource_id)`

OAuth functions (return URL to open in popup):
- `start_oauth_connect(datasource_type)` → String

### Task 5.4: Data Sources (DatasourceModal)
**React reference:** `apps/frontend/src/components/settings/datasources/` directory
**This is the biggest single component — multiple files for the modal.**

Audit the datasources directory, identify all files, understand the schema-driven
form rendering approach. This may need to be broken into sub-tasks.

### Task 5.5: Data Sources (main UI)
**Depends on:** Tasks 5.3, 5.4
**React reference:** DatasourceSettings.jsx

UI: Datasource list with status badges, enable/disable toggles, credential status,
catalog status, add/edit/delete. OAuth popup flow for BigQuery, Snowflake, etc.

---

## Phase 6: Billing

### Task 6.1: Billing (server functions)
**React reference:** `apps/frontend/src/components/BillingPanel.jsx` (887 lines)
**Read the entire file.**

Server functions:
- `get_subscription_info()` → SubscriptionInfo
- `get_invoices()` → Vec<Invoice>
- `create_checkout(plan, team_size)` → String (Stripe checkout URL)
- `cancel_subscription()`
- `reactivate_subscription()`
- `update_team_size(team_size: i32)`
- `create_portal_session()` → String (Stripe portal URL)

### Task 6.2: Billing (plan cards + pricing)
**Depends on:** Task 6.1
**React reference:** BillingPanel.jsx PlanCard sub-component (lines 804-887)

UI: Plan comparison cards (Free, Pro, Team), feature lists, pricing display,
current plan indicator, upgrade/downgrade buttons.

### Task 6.3: Billing (subscription management UI)
**Depends on:** Tasks 6.1, 6.2
**React reference:** BillingPanel.jsx main component

UI: Current subscription card, cancel/reactivate, team size controls,
invoice history, Stripe portal link, checkout redirect handling.

---

## Verification After Each Phase

After completing each phase:
1. All `cargo check -p kyomi-ui --features ssr` passes with zero errors
2. `trunk build --public-url /leptos/` succeeds
3. Server starts and serves the Leptos page
4. Visual comparison: Leptos page matches React page in browser
5. All server functions return correct data
6. Tab navigation works (Leptos tabs serve Leptos, React tabs serve React)

---

## Summary

| Phase | Tasks | New Server Fns | New Components | Est. Complexity |
|-------|-------|---------------|----------------|-----------------|
| 1 | 8 | 1 (UserContext) | 7 UI components | Medium |
| 2 | 4 | 6 (Slack) | 2 (MCP, AIProvider) | Medium |
| 3 | 4 | 12 | 0 (uses Phase 1) | Medium-Hard |
| 4 | 4 | 11 | 0 (uses Phase 1) | Medium |
| 5 | 5 | 14 + OAuth | DatasourceModal | Hard |
| 6 | 3 | 7 | PlanCard | Hard |
| **Total** | **28** | **~51** | **~10** | |
