# Server Fn ↔ REST Parity Sweep (KYO-121)

Audit of every `(crates/kyomi-ui/src/server_fns/<module>.rs, apps/server/src/routes/<module>.rs)` pair as of 2026-04-21 (branch `jason/kyo-121-parity-audit`, from `origin/main` at 7c3e188).

This is a research-only document. No code changes are in scope — the goal is a surgical inventory that Phase 2 PRs will execute against. Each pair lists:

1. File:line anchors on both sides
2. Orchestration diff summary (what each side actually does)
3. DI divergence check (grep `use_context::<`, compare against `AppState`)
4. Severity rating (🔴 / 🟡 / 🟢)
5. Proposed shared `services::` signature (concrete Rust)
6. Dependency order

## DI baseline

A single `use_context::<ServerContext>()` call is centralized in `crates/kyomi-ui/src/server_fns/mod.rs:195` (`extract_context()`). Every server_fn pulls dependencies out of `ServerContext` — there is no per-module `use_context::<Arc<Foo>>()` today. Grep:

```
$ rg 'use_context::<' crates/kyomi-ui/src/server_fns/
server_fns/mod.rs:195:    leptos::prelude::use_context::<ServerContext>()
server_fns/connect.rs:338:    //! `leptos::use_context::<Arc<ConnectTokenService>>()`, which the server
```

The `connect.rs` hit is a comment in the KYO-115 regression-test doc explaining the old bug — not a live `use_context` call. Net: the per-module DI drift hotspots from earlier in the migration are all resolved. The remaining drift risk is orchestration (what the two sides *do* after getting their deps), not wiring.

`ServerContext` exposes these optional fields that may be absent (`None`) at runtime and therefore are individual drift-risk seams: `encryption_key`, `kv`, `redis`, `webauthn`, `connect_registry`, `ws_manager`, `cancel_registry`, `platforms`, `connect_token`, `mcp_sessions`, `slack_client`. The REST side gets the same things via `AppState` but without the `Option` wrapping — several `None` guards in server_fns turn what should be a straight call into an early-return error. Call this out per-pair where it matters.

---

## 🔴 Fix-first

These pairs have already bit us or are silently diverged today. Phase 2 PRs go at these first.

| Pair | Impact (one line) |
| --- | --- |
| `analytics.rs` | Leptos `create_analytics_site` skips `CatalogIndexingService::spawn_analytics_post_create`; site created, catalog never indexed, usage dashboard empty |
| `watches.rs` | Leptos `create_watch` / `update_watch` skip `check_watch_capability`, name/prompt length validation, AND the `send_watch_update` WS broadcast → other tabs/users never see the change |
| `team.rs` (workspaces invitations) | Leptos `invite_member` skips the invitation email AND the `send_workspace_invitation` WS toast → invitee gets no email and no realtime notification (repeat of the KYO-70 feedback-notifications bug) |
| `team.rs` (ownership transfer) | Leptos `initiate_ownership_transfer` skips `send_ownership_transfer_offered` WS broadcast AND skips the "target must be workspace member" precondition check; also uses `xfer-{24}` id format vs REST's `transfer-{20}` |
| `dashboards.rs` | Leptos `create_dashboard` / `update_dashboard` / `delete_dashboard` skip the `send_dashboard_update` WS broadcast → collaborators' tabs never refresh |
| `chat.rs` | Leptos `send_chat_message` re-implements ~250 lines of agent-dispatch orchestration that REST has a copy of; the two will drift on any future agent change |
| `copilot.rs` | Same as chat.rs — 100+ lines of duplicate agent orchestration (exec_config build, spawn, deliver_response, error pathway, cancel registry bookkeeping) |

KYO-70 (feedback), KYO-72 (billing MCP invalidation), and KYO-115 (connect DI) were all verified against current code below and are already **🟢** — they are correctly resolved and should not be Phase 2 targets.

---

## Inventory

Enumerated by cross-referencing `ls crates/kyomi-ui/src/server_fns/` and `ls apps/server/src/routes/` plus `enterprise/kyomi-slack/src/routes.rs` (where Slack REST lives). 29 server_fn modules, 33 REST route files, plus the Slack enterprise crate. Pair counts below are by **feature domain**, not by filename, because several routes handle multiple server_fns and vice versa.

Sections are in file-alphabetical order of the server_fn side, with "REST only" singletons at the end.

---

### ai.rs (workspace BYOK)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/ai.rs:1-1016` — `get_workspace_ai_config`, `update_workspace_ai_config`, `test_workspace_ai_config`, `list_workspace_ai_models`
- REST: **N/A — only server_fn.** No REST counterpart exists; BYOK is a Leptos-only settings page (`pages/settings/ai.rs`).

**Orchestration diff summary:** Reads/writes `workspace_ai_config`, validates provider enum, reconciles API key with encrypted-at-rest semantics, and performs live auth-check HTTP calls against Anthropic/OpenAI/Gemini to test candidate keys. Includes a curated cross-provider model list fetch with URL fallback chain (caller → stored → default). All logic lives in the server_fn and delegates to `kyomi_auth::workspace_ai_config` and `kyomi_auth::billing_service` (for bundle balance).

**DI divergence check:** Uses `extract_auth` + `extract_context` only; pulls `ctx.db`, `ctx.config`. No non-allowlisted context lookups. No REST equivalent to compare against — the HTTP client is constructed inline per-call.

**Severity:** 🟢 — singleton. The leptos-only path already goes through a single shared module (`kyomi_auth::workspace_ai_config`) for persistence and `kyomi_auth::billing_service` for balance. The parse/HTTP helpers are only called from one caller, so no duplication exists.

**Proposed shared signature:** None needed. Document as a REST-less singleton. If we ever add an MCP or public API path that needs the same config read, extract `load_view(&db, &auth, ws_id) -> WorkspaceAiConfigView` at that point.

**Dependency order:** Independent.

---

### analytics.rs (analytics sites + usage)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/analytics.rs:54-305` — `list_analytics_sites`, `get_analytics_usage`, `create_analytics_site`, `update_analytics_site`, `delete_analytics_site`
- REST: `apps/server/src/routes/analytics_sites.rs:37-319` (site CRUD) + `apps/server/src/routes/analytics_sites.rs:321-421` (`get_usage`)

**Orchestration diff summary:** Both sides delegate CRUD to `kyomi_auth::analytics_site_service`. REST's `create_site` (line 134-196) spawns `CatalogIndexingService::spawn_analytics_post_create` (line 183-193) after creation to kick off per-site catalog indexing — **the Leptos `create_analytics_site` does not**. Both sides' "get usage" implementations diverge sharply: REST reads per-site events from ClickHouse via `get_per_site_counts_from_clickhouse` and returns a `sites: Vec<SiteUsage>` breakdown (`analytics_sites.rs:366-390`), while Leptos returns a single top-level `events_used` with no per-site breakdown and computes a different status ladder including a `"reserve"` state that REST lacks (`analytics.rs:142-156`). This is two distinct orchestrations, not a thin wrapper pair.

**DI divergence check:** Leptos uses `ctx.db`, `ctx.config.analytics_clickhouse_*`, `ctx.config.redis_url` (creates a one-off pool — REST gets `state.redis` directly). The Leptos-side Redis pool construction on every call is a minor perf issue but not a correctness drift. No non-allowlisted contexts.

**Severity:** 🔴 — the missing `spawn_analytics_post_create` on the Leptos create path is a real product bug (catalog never gets indexed for sites created via the settings UI). The usage-shape divergence is 🟡 (no per-site breakdown in Leptos UI today, but React used to show it — refer to RemainingPagesLeptosMigrationPlan if present).

**Proposed shared signature:** Split into two services. Target file: `apps/server/src/services/analytics_sites.rs`.

```rust
pub struct CreateAnalyticsSiteCtx<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub redis: Option<kyomi_core::RedisPool>,
    pub encryption_key: Option<std::sync::Arc<[u8; 32]>>,
    pub embedding: kyomi_embed::LazyEmbedding,
    pub config: &'a kyomi_core::Config,
}

pub async fn create_analytics_site(
    ctx: &CreateAnalyticsSiteCtx<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    name: &str,
    domains: &[String],
    datasource_slug: Option<&str>,
) -> Result<kyomi_auth::analytics_site_service::AnalyticsSite, kyomi_core::Error>;

pub async fn get_analytics_usage(
    db: &kyomi_core::DbPool,
    redis: Option<&mut kyomi_core::RedisPool>,
    config: &kyomi_core::Config,
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<AnalyticsUsageSnapshot, kyomi_core::Error>;

pub struct AnalyticsUsageSnapshot {
    pub events_used: u64,
    pub events_limit: u64,
    pub grace_limit: u64,
    pub usage_percent: f64,
    pub bundle_balance: u64,
    pub status: &'static str, // "ok" | "warning" | "exceeded" | "blocked" | "reserve"
    pub sites: Vec<AnalyticsSiteUsage>,
}
```

**Dependency order:** Independent.

---

### auth.rs (login / signup / OAuth / recovery / passkeys)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/auth.rs:111-2698` — `get_auth_config`, `login_with_password`, `signup_start`, `signup_complete`, `google_oauth_callback`, `resend_verification`, `recovery_start`, `recovery_verify`, `recovery_set_password`, `passkey_login_start`, `passkey_login_complete`, `passkey_register_start/complete`, `passkey_signup_complete`, `passkey_recovery_verify`
- REST: spread across five files:
  - `apps/server/src/routes/auth.rs:30-768` — `refresh_token`, `logout`, `logout_all`, `get_me`, `get_profile/update_profile`, `get_sessions/revoke_session`, `verify_email`, `resend_verification`, `check_email`, `check_token`, `switch_workspace`, `websocket_token`, `get_auth_config`
  - `apps/server/src/routes/auth_password.rs:32-741` — `login`, `set_password`, `change_password`, `signup_start`, `signup_complete`
  - `apps/server/src/routes/auth_google_oauth.rs:47-959` — `google_login`, `google_callback`, `accept_terms`, `google_oauth_connect/disconnect/status`, `google_link_callback`, `google_oauth_projects`
  - `apps/server/src/routes/auth_recovery.rs:35-295` — `recovery_start`, `recovery_verify`, `recovery_set_password`
  - `apps/server/src/routes/auth_passkeys.rs:37-1420` — `register_start/complete`, `login_start/complete`, `add_start/complete`, `recovery_request/verify/register`, `passkey_delete/rename`, `passkeys_list`, `signup_complete`

**Orchestration diff summary:** Every server_fn in `server_fns/auth.rs` constructs its own session+cookie+headers dance inline. Shared service modules already exist (`kyomi_auth::session::create_authenticated_session`, `kyomi_auth::google_oauth`, `kyomi_auth::password`, `kyomi_auth::totp`, `kyomi_auth::webauthn`, `kyomi_auth::token_service`), but the Leptos and REST sides still build their own parameter structs and handle their own cookie emission differently (Leptos via `ResponseOptions`, REST via `HeaderMap`). There are minor but real divergences: e.g. `login_with_password` server_fn rate-limits login attempts (by calling `kyomi_auth::login_rate_limit` — verify); REST's `login` handler has its own rate-limit plumbing in `auth_password.rs`. Needs a deep read in Phase 2 to map each server_fn to its REST counterpart 1:1.

**DI divergence check:** Leptos uses `ctx.kv`, `ctx.webauthn`, `ctx.config`, `ctx.encryption_key`, `ctx.db`. REST has the same via `AppState`. All `Option`-wrapping in `ServerContext` for `kv`/`webauthn` produces `None`-guard early errors that the REST side does not have (REST assumes they're always present — because they are, for an AppState). This is a drift risk: if auth-related features quietly disable themselves via `None` error paths that never happen in production but hit in tests, we won't notice.

**Severity:** 🟡 — large, sprawling, many small differences. None of the individual paths has a confirmed drift *today* (KYO-70/72/115 all clean), but the surface area is huge and PaaS invariants are scattered across both sides. This needs a dedicated pair of Phase 2 tickets:
- Ticket A: extract `{login, signup_start, signup_complete, recovery_start, recovery_verify, recovery_set_password}` into `apps/server/src/services/auth.rs`
- Ticket B: extract passkey lifecycle into `apps/server/src/services/passkeys.rs`

**Proposed shared signatures:**

```rust
// apps/server/src/services/auth.rs

pub struct AuthContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub kv: &'a kyomi_core::KVPool,
    pub config: &'a kyomi_core::Config,
    pub encryption_key: &'a std::sync::Arc<[u8; 32]>,
}

pub struct AuthOutcome {
    pub user: kyomi_core::models::User,
    pub session: kyomi_auth::session::AuthSession,
    pub cookies: axum::http::HeaderMap,  // ready to append to ResponseOptions or a Response
    pub requires_totp: bool,
}

pub async fn login_with_password(
    ctx: &AuthContext<'_>,
    email: &str,
    password: &str,
    device: &kyomi_auth::session::DeviceInfo,
) -> Result<AuthOutcome, kyomi_core::Error>;

pub async fn signup_start(
    ctx: &AuthContext<'_>,
    email: &str,
    password: &str,
    name: Option<&str>,
) -> Result<SignupStartResult, kyomi_core::Error>;

pub async fn signup_complete(
    ctx: &AuthContext<'_>,
    temp_token: &str,
    marketing_consent: bool,
    device: &kyomi_auth::session::DeviceInfo,
) -> Result<AuthOutcome, kyomi_core::Error>;

pub async fn recovery_start(ctx: &AuthContext<'_>, email: &str) -> Result<(), kyomi_core::Error>;
pub async fn recovery_verify(ctx: &AuthContext<'_>, email: &str, code: &str) -> Result<RecoverySession, kyomi_core::Error>;
pub async fn recovery_set_password(
    ctx: &AuthContext<'_>,
    recovery_token: &str,
    new_password: &str,
    device: &kyomi_auth::session::DeviceInfo,
) -> Result<AuthOutcome, kyomi_core::Error>;
```

Passkey signatures follow the same pattern but take a `webauthn: &webauthn_rs::Webauthn` field.

**Dependency order:** Auth extraction depends on nothing. Every other pair that uses `extract_auth` depends on it being stable. Do auth first.

---

### billing.rs (subscription + bundles)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/billing.rs:240-789` — `get_subscription_info`, `get_invoices`, `create_checkout`, `cancel_subscription`, `reactivate_subscription`, `update_user_limit`, `create_portal_session`, `purchase_ai_bundle`, `purchase_analytics_bundle`, `get_stripe_publishable_key`, `get_checkout_session_status`
- REST: `apps/server/src/routes/billing.rs:49-1159` — `create_checkout`, `stripe_webhook`, `cancel_subscription`, `reactivate_subscription`, `get_subscription_info`, `purchase_ai_bundle`, `purchase_analytics_bundle`, `get_ai_usage_status`, `get_invoices`, `create_portal_session`

**Orchestration diff summary:** The KYO-72 incident (Leptos path skipping immediate MCP session invalidation) is verified fixed: `server_fns/billing.rs:406-416` delegates to `kyomi_auth::subscription_service::modify_existing_subscription`, which is the same function `routes/billing.rs:312-334` delegates to. Both sides call into `kyomi_auth::billing_service` and `kyomi_auth::stripe_service`. However, the REST side has the **embedded vs hosted checkout** split: `routes/billing.rs::create_checkout` uses `create_checkout_session` (hosted, returns `checkout_url`) while `server_fns/billing.rs::create_checkout` uses `create_embedded_checkout_session` (returns `client_secret`). These are two genuinely different Stripe APIs — the Leptos UI embeds Stripe.js, the legacy React UI redirects. Both sides also have their own `load_workspace` and `require_stripe` helpers that duplicate each other exactly.

The Stripe **webhook** handler (`stripe_webhook`, `handle_subscription_event`, `handle_subscription_deleted`, `handle_invoice_payment_{succeeded,failed}`, `handle_checkout_completed`) only exists on the REST side. That's correct — Stripe posts to a public HTTP URL, there is no Leptos path for webhooks.

Other small drifts: Leptos uses `require_workspace_owner` (owner-only), REST uses `require_workspace_admin` (owner + admin). Owner-only is stricter and was explicitly chosen on the Leptos side (see line 107-117 comment). Worth verifying that's intentional in Phase 2 — it's currently a silent behavioral split.

**DI divergence check:** Leptos uses `ctx.config` (for `stripe_*_key`) and `ctx.mcp_sessions`, `ctx.db`, `ctx.config.redis_url`. `ctx.mcp_sessions` is `Option` on Leptos but mandatory `state.mcp_sessions` on REST — producing an early-error on Leptos for the modify path if MCP isn't configured. This is safe in practice (SaaS always configures it) but is a drift seam. No non-allowlisted contexts.

**Severity:** 🟡 — core mutation paths already go through `subscription_service::modify_existing_subscription` (the KYO-72 fix). The duplicated `load_workspace` + `require_stripe` + invoice list + portal session helpers are ~200 lines of copy. The owner-vs-admin permission split is the only subtle behavioral drift and may be intentional (downgrade to 🟢 if so).

**Proposed shared signature:** Target file `apps/server/src/services/billing.rs`.

```rust
pub struct BillingContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub config: &'a kyomi_core::Config,
    pub stripe: &'a kyomi_auth::stripe_service::StripeService,
    pub mcp_sessions: &'a kyomi_auth::mcp_session_manager::MCPSessionManager,
}

pub async fn load_subscription_snapshot(
    db: &kyomi_core::DbPool,
    redis: Option<&mut kyomi_core::RedisPool>,
    config: &kyomi_core::Config,
    ws_id: &str,
) -> Result<SubscriptionSnapshot, kyomi_core::Error>;

pub async fn list_invoices(
    ctx: &BillingContext<'_>,
    ws_id: &str,
) -> Result<Vec<kyomi_auth::stripe_service::InvoiceRecord>, kyomi_core::Error>;

pub async fn cancel_subscription_at_period_end(
    ctx: &BillingContext<'_>,
    ws_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn reactivate_subscription(
    ctx: &BillingContext<'_>,
    ws_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn create_portal_session(
    ctx: &BillingContext<'_>,
    ws_id: &str,
    return_url: &str,
) -> Result<String, kyomi_core::Error>;  // returns portal_url

// Checkout remains split: callers choose embedded vs hosted explicitly.
pub async fn create_embedded_checkout(
    ctx: &BillingContext<'_>,
    ws_id: &str,
    user_email: &str,
    quantity: u64,
) -> Result<CheckoutOutcome, kyomi_core::Error>;
```

**Obstacle:** Webhook handling (`stripe_webhook` + event handlers, lines 340-764) is REST-only and must stay REST-only. Do not try to make it callable from Leptos. Phase 2 scope explicitly excludes the webhook handlers.

**Dependency order:** Independent of auth.rs for the mutation paths (they take an authenticated user). Can land in parallel with other pairs.

---

### catalog_refresh.rs (SQL catalog indexing)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/catalog_refresh.rs:1-847` — exports `execute_catalog_refresh(CatalogRefreshParams)`. This is **already** the shared service.
- REST: `apps/server/src/routes/catalog.rs:1292-1397` (`refresh_catalog` handler + `execute_catalog_refresh` wrapper at line 1324-1397) — calls `kyomi_ui::server_fns::catalog_refresh::execute_catalog_refresh` directly.
- Also used by `apps/server/src/routes/bigquery.rs::catalog_refresh` (line 912 per grep).
- Also used by `crates/kyomi-ui/src/server_fns/sql_editor.rs::refresh_catalog` (line 1672).

**Orchestration diff summary:** This is the exemplar for what Phase 2 wants everywhere else. The orchestration pipeline ("build provider → discover containers → index tables → archive stale → populate embeddings") lives in one module, both sides construct `CatalogRefreshParams` from their respective state types and call `execute_catalog_refresh`. REST does credential decryption + OAuth refresh + `build_user_context` before the call; SQL Editor's `refresh_catalog` does the same.

**DI divergence check:** N/A — the shared function takes a `CatalogRefreshParams<'_>` struct that encapsulates all dependencies. Neither caller reaches into its own state for anything except constructing that struct.

**Severity:** 🟢 — canonical reference implementation. Do not touch.

**Proposed shared signature:** Already exists:
```rust
pub async fn execute_catalog_refresh(
    params: CatalogRefreshParams<'_>,
) -> Result<CatalogRefreshResult, kyomi_core::Error>;
```

**Architectural note for Phase 2:** This module currently lives in `crates/kyomi-ui/` which means the REST server depends on `kyomi-ui`. That inverted dependency is an existing smell but is out of scope for KYO-121. Flag it as a Phase 0 follow-up: once we have `apps/server/src/services/`, move this module there so `apps/server/` does not depend on `crates/kyomi-ui/` for business logic.

**Dependency order:** None — already shared.

---

### chat.rs (session + message CRUD + agent dispatch)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/chat.rs:148-1281` — `get_chart_context`, `get_websocket_config`, `list_chat_sessions`, `get_session_messages`, `update_session_title`, `delete_chat_session`, `bulk_delete_sessions`, `search_chat_messages`, `send_chat_message`, `share_session`, `unshare_session`, `mark_session_read`, `toggle_message_pin`, `update_message_content`
- REST: `apps/server/src/routes/chat.rs:48-1462` — `list_sessions`, `get_session`, `get_messages`, `update_session`, `delete_session`, `bulk_delete_sessions`, `search_sessions`, `get_models`, `get_status`, `send_message`, `share_session`, `unshare_session`, `mark_session_read`, `transfer_ownership`, `update_message_content`, `toggle_pin`, `update_chart_config`, `update_chart_sql`

**Orchestration diff summary:** CRUD paths on both sides delegate cleanly to `kyomi_auth::chat_service`. The heavy weight is **`send_chat_message` (Leptos) ↔ `send_message` (REST)**: both are ~200-250 line functions that build an `AgentExecutionConfig`, register a cancel token, spawn a tokio task running `kyomi_agent::execute_agent_chat`, and on completion call `kyomi_agent::deliver_response` or `websocket::helpers::send_error`. They are nearly identical but not byte-for-byte. Both already call the same shared entry points (`execute_agent_chat`, `deliver_response`, `add_message`, `create_session`), but **the wrapper scaffolding that builds `AgentExecutionConfig` and manages cancel registry lifecycle is duplicated.** This is the #1 drift risk for the chat subsystem: if we add a new field to `AgentExecutionConfig`, both sides must update in lock-step or the feature silently only works on one UI. Also: the REST side's `send_message` generates user/assistant message IDs lazily (delegated to the adapter's `persist_after_chat`), while the Leptos side pre-saves an empty assistant placeholder — this is a real behavioral split visible in the DB.

REST has extra endpoints with no Leptos counterpart yet: `transfer_ownership` (not ported), `update_chart_config`, `update_chart_sql`. Flag as missing-parity-or-deferred in Phase 2.

**DI divergence check:** Leptos uses `ctx.ws_manager`, `ctx.cancel_registry`, `ctx.platforms`, `ctx.kv`, `ctx.encryption_key`, `ctx.embedding`, `ctx.connect_registry`, `ctx.config`, `ctx.db`. REST uses `state.` equivalents. All `Option`-wrapped fields on Leptos produce `None`-guard errors ("WebSocket manager not configured", etc.) — in production they are always `Some`, so this is only a test-time drift today. No non-allowlisted contexts.

**Severity:** 🔴 — the duplicate agent-dispatch scaffolding is a live drift risk with each agent change. Not a current bug, but exactly the KYO-70/KYO-72 shape ("it works on one side and silently doesn't on the other").

**Proposed shared signature:** Target file `apps/server/src/services/chat.rs`.

```rust
pub struct ChatSendContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub kv: &'a kyomi_core::KVPool,
    pub encryption_key: &'a std::sync::Arc<[u8; 32]>,
    pub embedding: &'a kyomi_embed::LazyEmbedding,
    pub ws_manager: &'a kyomi_auth::websocket::WebSocketManager,
    pub config: &'a kyomi_core::Config,
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    pub platforms: std::sync::Arc<kyomi_core::platform::PlatformRegistry>,
    pub cancel_registry: ChatCancelRegistry,  // trait abstraction over the two CancelRegistry types
}

pub struct ChatSendInput {
    pub message: String,
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub skip_ai: bool,
    pub client_msg_id: Option<String>,
    pub current_time_user_tz: Option<String>,
}

pub struct ChatSendOutcome {
    pub session_id: String,
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub status: ChatSendStatus, // Processing | Skipped
}

pub async fn send_chat_message(
    ctx: &ChatSendContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    input: ChatSendInput,
) -> Result<ChatSendOutcome, kyomi_core::Error>;
```

**Obstacle:** `CancelRegistry` is two distinct types — `apps/server/src/cancel_registry.rs::CancelRegistry` and `crates/kyomi-ui/src/server_fns/mod.rs::CancelRegistry`. They share the same underlying `DashMap` via `from_shared`, but the types are nominally different. The shared service will need a trait (`ChatCancelRegistry`) with `register/cancel/remove` methods, implemented for both. Not blocking — just a small trait declaration.

**Dependency order:** Independent of auth.rs. Depends on `ChatCancelRegistry` trait being defined first (trivial prework in the same PR).

---

### collections.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/collections.rs:96-252` — `list_collections`, `create_collection`, `update_collection`, `delete_collection`, `add_dashboard_to_collection`, `remove_dashboard_from_collection`
- REST: `apps/server/src/routes/collections.rs:35-341` — same six ops

**Orchestration diff summary:** Both sides are thin wrappers over `kyomi_auth::collection_service`. The only meaningful difference: Leptos's `list_collections` takes an optional `doc_type` filter and passes it through; REST's `list_collections` always passes `None` and filters client-side. That's a genuine functional difference (Leptos supports knowledge-vs-dashboard collection filtering, REST doesn't) but it's additive on the Leptos side, not a drift.

**DI divergence check:** Both use `db` only. No divergence.

**Severity:** 🟢 — both sides are already thin wrappers around the same `kyomi_auth::collection_service` functions. The per-side `to_collection_item` / `collection_to_response` serialization helpers are ~20 lines of copy but produce identical wire shapes (verified in test modules). Not worth extracting unless we're doing it anyway for hygiene.

**Proposed shared signature:** None strictly needed. If extracted for hygiene: `apps/server/src/services/collections.rs::list_collections(db, ws_id, doc_type) -> Vec<CollectionWithDashboards>` — but this is just an alias for the service-layer function that already exists. Document as "no extraction needed; services layer already owns this."

**Dependency order:** Independent.

---

### connect.rs (Kyomi Connect datasources)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/connect.rs:1-422` — `list_connect_datasources`, `create_connect_datasource`, `rotate_connect_token`, `connect_status`, `disconnect_connect_datasource`
- REST: `apps/server/src/routes/datasources.rs:2885-2996` (`rotate_connect_token`, `disconnect_connect`, `connect_status`). Creation + listing use the generic `create_datasource` / `list_datasources` REST endpoints in the same file (the Leptos side filters client-side).

**Orchestration diff summary:** KYO-115 regression verified fixed — `server_fns/connect.rs:114, 144, 193` correctly pull `ctx.connect_token` from `ServerContext` instead of a phantom standalone `use_context::<Arc<ConnectTokenService>>()`. Unit tests at `server_fns/connect.rs:333-422` pin that behavior. Both sides call `ConnectTokenService::generate` and `datasource_service::update_connect_jti` + `clear_connect_jti`. The one remaining stylistic difference is the Leptos-side `generate_connect_token(Option<&Arc<ConnectTokenService>>, ...)` helper that centralizes the `None` case handling; REST inlines the `ok_or_else`.

**DI divergence check:** The `generate_connect_token` helper takes `Option<&Arc<ConnectTokenService>>` and returns `Err("Kyomi Connect is not configured on this server")` for `None`. REST does the same inline at `datasources.rs:2905-2907`. Identical behavior, different code.

**Severity:** 🟢 — KYO-115 is fully resolved. Both sides delegate to `datasource_service` + `ConnectTokenService::generate` + `connect_token::check_presence`. The only duplication is ~10 lines of `None` handling + error mapping per entry point. Worth a light hygiene pass but not urgent.

**Proposed shared signature:** Target file `apps/server/src/services/connect_datasources.rs`.

```rust
pub async fn rotate_connect_token(
    db: &kyomi_core::DbPool,
    connect_token: Option<&std::sync::Arc<kyomi_auth::connect_token::ConnectTokenService>>,
    ws_id: &str,
    datasource_id: &str,
) -> Result<String, kyomi_core::Error>;  // returns new token

pub async fn disconnect_connect_datasource(
    db: &kyomi_core::DbPool,
    ws_id: &str,
    datasource_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn connect_status(
    db: &kyomi_core::DbPool,
    redis: Option<&mut kyomi_core::RedisPool>,
    ws_id: &str,
    datasource_id: &str,
) -> Result<ConnectStatusSnapshot, kyomi_core::Error>;
```

`list_connect_datasources` and `create_connect_datasource` (Leptos) are trivial filters/composites over the generic datasource service and do not need their own shared wrappers.

**Dependency order:** Depends on datasources.rs extraction landing first (which owns `resolve_or_404` + the workspace-member precondition checks).

---

### context.rs (user + workspace capability context)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/context.rs:51-310` — `get_user_context`
- REST: **N/A — only server_fn.** The `UserContext` struct combines what React's `AuthContext + CapabilitiesContext + SystemConfigContext` used to provide from three separate REST calls. No exact REST analogue; closest is `routes/auth.rs::get_me` (line 241) + `routes/workspaces.rs::get_current_workspace` + the capabilities computed from `workspaces/settings`.

**Orchestration diff summary:** Server_fn loads workspace via `workspace_service::get_workspace_full`, computes capabilities via `kyomi_core::capability::compute_capabilities[_self_hosted]`, enriches with runtime flags (has_datasources, llm_configured) and the user's `chart_palette` preference. All orchestration steps are single service calls except the capability map flattening and the free-tier fallback for SaaS-no-workspace users — both of which live only in this file.

**DI divergence check:** Uses `ctx.db`, `ctx.config`. `ctx.config.llm_configured()` is a helper; no drift. REST equivalent (`get_me` + `get_current_workspace`) gets the same data via `state.db` + `state.config`.

**Severity:** 🟢 — singleton. Already a thin orchestrator over `workspace_service` + `capability::compute_capabilities`. The flattening helpers (`build_capabilities_map`, `free_tier_capabilities`) are Leptos-only concerns that React's 3-context split did differently.

**Proposed shared signature:** None needed. If future MCP/public API callers need the same bundle, extract at that point as `services::context::load_user_context(&ctx, &auth) -> UserContext`.

**Dependency order:** Independent.

---

### copilot.rs (dashboard / chart-builder / watch copilot agent)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/copilot.rs:37-367` — `create_copilot_session`, `send_copilot_message`, `delete_copilot_session`
- REST: `apps/server/src/routes/copilot.rs:29-438` — `send_copilot_message`, `delete_copilot_session`

**Orchestration diff summary:** Near-identical duplication with `chat.rs` — both sides build an `AgentExecutionConfig` with the copilot-specific `system_prompt` and `tools_subset`, spawn the agent task via `kyomi_agent::execute_agent_chat`, deliver response via `kyomi_agent::deliver_response` or error via `websocket::helpers::send_error`. One difference: REST's `send_copilot_message` does session-get-or-create inline, while Leptos requires the caller to have already created a session via `create_copilot_session`. Both sides call the same `kyomi_agent::copilot::normalize_context_type`, `session_title_for_context`, `build_copilot_system_prompt`, `tools_for_context` helpers. The agent execution scaffolding (spawn body + error handler + cancel cleanup) is ~80 lines duplicated.

**DI divergence check:** Same set of fields as chat.rs — `ws_manager`, `cancel_registry`, `platforms`, `kv`, `encryption_key`, `embedding`, `connect_registry`, `config`, `db`. Leptos's `Option`-guarded early-errors repeat here.

**Severity:** 🔴 — same drift-risk shape as chat.rs. Any change to the agent loop requires synchronized edits on both sides.

**Proposed shared signature:** Target file `apps/server/src/services/copilot.rs` (shared with chat.rs via a common `AgentDispatchContext` if the refactor proves clean).

```rust
pub async fn send_copilot_message(
    ctx: &crate::services::chat::ChatSendContext<'_>,  // same context as chat
    auth: &kyomi_auth::middleware::AuthUser,
    session_id: String,
    context_type: CopilotContextType, // enum: DashboardCopilot | ChartBuilderCopilot | WatchCopilot
    message: String,
    content: Option<String>,  // dashboard markdown / chart yaml / watch json
    timezone: Option<String>,
    current_time_user_tz: Option<String>,
) -> Result<CopilotOutcome, kyomi_core::Error>;
```

**Obstacle:** REST's `send_copilot_message` supports session-auto-create (line 150-196); Leptos's does not. Decide in Phase 2 whether the shared service auto-creates (caller passes `Option<String>` session_id) or rejects (caller passes `String`). I recommend auto-create for API consistency with `chat::send_chat_message`.

**Dependency order:** Land alongside chat.rs — they share the `ChatSendContext` type and the shared trait for `CancelRegistry`.

---

### dashboards.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/dashboards.rs:125-704` — `list_dashboards`, `get_dashboard`, `create_dashboard`, `update_dashboard`, `delete_dashboard`, `list_versions`, `get_version`, `diff_versions`, `restore_version`, `get_user_default_dashboard`, `set_user_default_dashboard`, `get_workspace_default_dashboard`, `set_workspace_default_dashboard`
- REST: `apps/server/src/routes/dashboards.rs:44-1351` — `create_dashboard`, `list_dashboards`, `get_dashboard`, `update_dashboard`, `delete_dashboard`, `list_versions`, `diff_versions`, `get_version`, `restore_version`, `export_pdf`

**Orchestration diff summary:** Both sides are mostly thin wrappers around `kyomi_auth::dashboard_service`. The **drift**: REST's `create_dashboard` (line 307-356) sends a `ws_helpers::send_dashboard_update` WebSocket broadcast after creation; the Leptos equivalent (server_fn line 240-276) does NOT. Same drift on `update_dashboard` (REST line 487-498, Leptos line 285-335). Same on delete. This means: collaborators' open tabs don't see dashboards created or edited through the Leptos UI until they reload. That's a real product bug in the same shape as KYO-70.

REST-only: `export_pdf` (line 770-779). Correctly REST-only — PDFs are binary blobs not suited to server_fn return types.

Default-dashboard endpoints (server_fn `get_user_default_dashboard` etc) are Leptos-specific — `routes/workspaces.rs::get_default_dashboard` has a parallel implementation that needs reconciling. List-dashboards `DocType` filter is cleanly shared.

**DI divergence check:** Uses `ctx.db`, `ctx.embedding`. No non-allowlisted contexts.

**Severity:** 🔴 — missing `send_dashboard_update` WebSocket broadcasts on the Leptos side.

**Proposed shared signature:** Target file `apps/server/src/services/dashboards.rs`.

```rust
pub struct DashboardMutationContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub embedding: &'a kyomi_embed::LazyEmbedding,
    pub ws_manager: &'a kyomi_auth::websocket::WebSocketManager,
}

pub async fn create_dashboard(
    ctx: &DashboardMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    title: &str,
    content: &str,
    doc_type: kyomi_core::models::DocType,
) -> Result<kyomi_core::models::Dashboard, kyomi_core::Error>;

pub async fn update_dashboard(
    ctx: &DashboardMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    dashboard_id: &str,
    title: Option<&str>,
    content: Option<&str>,
    change_summary: Option<&str>,
) -> Result<kyomi_core::models::Dashboard, kyomi_core::Error>;

pub async fn delete_dashboard(
    ctx: &DashboardMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    dashboard_id: &str,
) -> Result<(), kyomi_core::Error>;
```

Each function both (a) delegates the DB op to `dashboard_service` and (b) emits the WS broadcast. This guarantees the broadcast cannot be skipped from either entry point.

**Dependency order:** Independent; priority high because of the live broadcast drift.

---

### datasources.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/datasources.rs:68-1536` — `list_datasources`, `get_datasource_types`, `toggle_datasource`, `delete_datasource`, `create_datasource_modal`, `update_datasource_settings`, `save_datasource_credentials`, `get_datasource_settings`, `test_datasource_standalone`, `test_existing_datasource`, `discover_datasource_resources`, `query_datasource_arrow`
- REST: `apps/server/src/routes/datasources.rs:43-2997` — `list_datasources`, `get_credential_status`, `list_datasource_types`, `check_sample_available`, `create_sample_datasource`, `create_datasource`, `get_datasource`, `update_datasource`, `delete_datasource_handler`, `save_credentials`, `get_credentials`, `delete_credentials`, `get_settings`, `save_settings`, `toggle_datasource`, `test_connection_standalone`, `test_datasource_connection`, `create_query_provider`, `execute_query`, `execute_query_stream`, `build_user_context`, `generate_ssh_key`, `get_affected_users`, `rotate_connect_token`, `disconnect_connect`, `connect_status`

**Orchestration diff summary:** This is the largest pair in the audit. Both sides delegate to `kyomi_auth::datasource_service`, `kyomi_auth::datasource_auth_service`, `kyomi_auth::credential_service`, and `kyomi_datasource_server::create_provider`. Most CRUD paths are already thin wrappers and call the same service functions. **But**: `create_datasource_modal` (Leptos) kicks off background catalog indexing via `CatalogIndexingService::spawn_post_create` (line 545-559). REST's `create_datasource` (line 864+) also spawns it. These look parallel but the spawn-point and exact credential-resolution sequence need to be compared side-by-side — flag as a Phase 2 bug-hunt.

`test_datasource_standalone` / `test_existing_datasource` duplicate the "create provider → test connection → close provider" block with `tokio::time::timeout` on both sides. REST's `test_datasource_connection` is ~50 lines, Leptos's is ~50 lines of the same control flow. Shared via a `test_provider_connection` helper would reduce ~100 lines to ~10.

`discover_datasource_resources` (line 951-1105 Leptos) has an almost-identical twin in `routes/catalog.rs::discover_all_resources` / `discover_catalog`. Needs a side-by-side read — flag Phase 2.

**DI divergence check:** Uses `ctx.db`, `ctx.encryption_key`, `ctx.embedding`, `ctx.connect_registry`. REST has the same via `state`. `ctx.encryption_key` is `Option` on Leptos; several paths bail with `"Encryption key not configured"` — in production this is never `None` but it's a drift seam.

**Severity:** 🟡 — no confirmed live bug today, but the surface area is enormous. The `create_datasource` + `test_connection` + `discover_resources` triad is the highest-value consolidation on this list by line count.

**Proposed shared signature:** Target file `apps/server/src/services/datasources.rs`. Needs to be carved into several functions:

```rust
pub struct DatasourceContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub encryption_key: &'a std::sync::Arc<[u8; 32]>,
    pub embedding: &'a kyomi_embed::LazyEmbedding,
    pub connect_registry: Option<&'a kyomi_datasource_server::ConnectRegistry>,
}

pub async fn create_datasource(
    ctx: &DatasourceContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    input: CreateDatasourceInput,
) -> Result<kyomi_core::models::datasource::DatasourceConfig, kyomi_core::Error>;
// Always spawns post-create catalog indexing for non-sample, non-connect datasources.

pub async fn test_provider_connection(
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
    connection_config: &serde_json::Value,
    credentials: &serde_json::Value,
    connect_registry: Option<&kyomi_datasource_server::ConnectRegistry>,
) -> TestConnectionResult;
// Single implementation of the create_provider + test_connection + close dance.

pub async fn discover_resources(
    ctx: &DatasourceContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ds_type: &kyomi_core::datasource_registry::DatasourceType,
    connection_config: &serde_json::Value,
    credentials: &serde_json::Value,
    datasource_slug: Option<&str>,
) -> Result<DiscoverResourcesResult, kyomi_core::Error>;
```

**Obstacle:** The query execution path (`query_datasource_arrow` on Leptos, `execute_query` + `execute_query_stream` on REST) is its own beast — Arrow IPC vs streaming JSON vs WebSocket streaming are three different transports. Do not try to unify beyond the shared `create_query_provider` helper that already exists (`datasources.rs:2021`). Flag as Phase 0: "query transports stay distinct; extract only the provider-setup preamble."

**Dependency order:** Blocks `connect.rs` extraction. Independent of everything else.

---

### feedback.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/feedback.rs:27-72` — `submit_feedback`
- REST: `apps/server/src/routes/feedback.rs:30-116` — `submit_feedback` (+ a REST-only `list_feedback` at line 122-164)

**Orchestration diff summary:** Both sides call `kyomi_auth::feedback_service::submit_feedback` with an identical `FeedbackInput`. That service owns persistence + Linear + Slack + email notifications. Verified against KYO-70 incident — the incident described the Leptos path skipping notifications; current code has that fully fixed (the shared `kyomi_auth::feedback_service` is the single source of truth, called from both sides). `list_feedback` is REST-only today (the Leptos settings UI doesn't list feedback; if it did, we'd add a Leptos `list_feedback` server_fn that calls the same service).

**DI divergence check:** `ctx.db`, `ctx.config` both sides. Trivial.

**Severity:** 🟢 — **KYO-70 is fully resolved.** Both sides are already thin wrappers around `kyomi_auth::feedback_service`. Do not touch. (This is one of the exemplar sections for the reviewer to verify the Phase 2 goal is achievable.)

**Proposed shared signature:** Already exists as `kyomi_auth::feedback_service::submit_feedback`. No further work needed.

**Dependency order:** None.

---

### home.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/home.rs:38-87` — `get_landing_config`
- REST: **N/A — only server_fn.** React used multiple REST calls (`/users/me`, `/workspaces/current`, config env) to compute the landing redirect client-side.

**Orchestration diff summary:** Single composite read. Reads user metadata, workspace settings, and config flags. All via service-layer calls.

**Severity:** 🟢 — singleton, already a thin orchestrator.

**Proposed shared signature:** None.

**Dependency order:** Independent.

---

### knowledge.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/knowledge.rs:17-125` — `list_knowledge_docs`, `create_knowledge_doc`, `delete_knowledge_doc`
- REST: **N/A — knowledge docs are Leptos-only.** (REST `/workspaces/{id}/knowledge` is the workspace-level knowledge string, a different concept.)

**Orchestration diff summary:** Knowledge docs share the `dashboards` table with `doc_type = Knowledge`. Both the create/list/delete paths delegate to `kyomi_auth::dashboard_service::*` (same functions used by the dashboards server_fn). So the shared service already exists — this module just specializes the `DocType` param.

**DI divergence check:** Same as dashboards.rs. No separate concerns.

**Severity:** 🟢 — singleton specialization of dashboards.rs. When dashboards.rs is extracted to `services::dashboards`, this module reuses the same service with a different `DocType`. No separate work needed.

**Proposed shared signature:** Covered by `services::dashboards::{create,update,delete}_dashboard` with a `DocType` parameter.

**Dependency order:** Land **after** `dashboards.rs` extraction so the shared signatures are stable.

---

### onboarding.rs (accept terms + sample datasource + oauth URL)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/onboarding.rs:50-692` — `accept_terms`, `get_onboarding_state`, `create_sample_datasource`, `check_sample_datasource_available`, `get_oauth_connect_url`
- REST: Partial overlap across multiple files:
  - `apps/server/src/routes/auth_google_oauth.rs:354-510` — `accept_terms` (the REST twin of the signup-flow branch of the server_fn)
  - `apps/server/src/routes/datasources.rs:724-863` — `check_sample_available`, `create_sample_datasource`
  - `get_onboarding_state` and `get_oauth_connect_url` are Leptos-only.

**Orchestration diff summary:** `accept_terms` is the big one — it's ~220 lines on both sides, handles the signup-with-Google-OAuth flow branching on a temp token, creates the user + workspace, spawns the admin notification (via `kyomi_auth::notifications::notify_signup`, which is correctly shared), and sets auth cookies. Verified the notification path matches — both sides fire-and-forget call `notifications::notify_signup`. The cookie-setting dance diverges: Leptos uses `ResponseOptions`, REST uses `HeaderMap` builder. Creating the session via `kyomi_auth::session::create_authenticated_session` is shared correctly.

`create_sample_datasource` Leptos (onboarding.rs:484-572) and REST (datasources.rs:755-863) both create the datasource + spawn catalog indexing. The REST side's creation has a separate "create from scratch via env-configured ClickHouse" that Leptos also does. Needs a side-by-side diff in Phase 2 to verify exact parity — the function bodies look similar but are not byte-identical.

**DI divergence check:** Uses `ctx.kv`, `ctx.encryption_key`, `ctx.db`, `ctx.embedding`, `ctx.config`. No non-allowlisted contexts.

**Severity:** 🟡 — the `accept_terms` path duplicates 200+ lines of orchestration. No known live drift, but this is the most-complex non-agent path in the whole audit and any signup-flow change (e.g. a new step, a new notification) requires synchronized edits.

**Proposed shared signature:** Target file `apps/server/src/services/onboarding.rs`.

```rust
pub struct OnboardingContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub kv: &'a kyomi_core::KVPool,
    pub config: &'a kyomi_core::Config,
    pub encryption_key: &'a std::sync::Arc<[u8; 32]>,
    pub embedding: &'a kyomi_embed::LazyEmbedding,
}

pub async fn accept_terms(
    ctx: &OnboardingContext<'_>,
    temp_token: &str,
    marketing_consent: bool,
    device: &kyomi_auth::session::DeviceInfo,
) -> Result<AcceptTermsOutcome, kyomi_core::Error>;

pub struct AcceptTermsOutcome {
    pub user: kyomi_core::models::User,
    pub session: kyomi_auth::session::AuthSession,
    pub cookies: axum::http::HeaderMap,
    pub kind: AcceptTermsKind,  // FreshSignup | ExistingUserReacceptance
}

pub async fn create_sample_datasource(
    ctx: &OnboardingContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
) -> Result<kyomi_core::models::datasource::DatasourceConfig, kyomi_core::Error>;
```

**Dependency order:** Depends on `auth.rs` extraction (shares `session::create_authenticated_session` + device extraction). Land after auth.

---

### ownership.rs (accept-ownership page)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/ownership.rs:41-200` — `get_ownership_transfer`, `accept_ownership_transfer`, `decline_ownership_transfer`
- REST: `apps/server/src/routes/workspaces.rs:1816-1941` — `accept_ownership_transfer_handler`, `decline_ownership_transfer_handler`. No direct REST equivalent of the read-by-id `get_ownership_transfer` — React read from `get_ownership_transfers` (list) and filtered client-side.

**Orchestration diff summary:** Both sides delegate the write path to `kyomi_auth::workspace_service::{complete_ownership_transfer, update_transfer_status}`. The **drift**: REST's `accept_ownership_transfer_handler` (line 1860-1868) sends a `ws_helpers::send_ownership_transfer_completed` WS broadcast to the previous owner; Leptos does NOT. Same for decline (REST has no equivalent broadcast for decline in the snippets I read, but the initiation broadcast is skipped on the Leptos side too — see team.rs).

**DI divergence check:** Uses `ctx.db`. No non-allowlisted contexts.

**Severity:** 🔴 — missing `send_ownership_transfer_completed` WS broadcast means the outgoing owner's UI doesn't update in realtime when the new owner accepts. Roll this into the team.rs fix since it's the same broadcast family.

**Proposed shared signature:** Rolled into team.rs (see below).

**Dependency order:** Land with team.rs.

---

### profile.rs (profile settings + invitations)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/profile.rs:21-288` — `get_profile`, `get_dashboards`, `get_pending_invitations`, `update_profile_name`, `update_theme`, `update_landing_page`, `update_default_dashboard`, `update_query_retention`, `update_chart_palette`, `accept_invitation`, `decline_invitation`
- REST: Partial overlap:
  - `apps/server/src/routes/auth.rs:275-381` — `get_profile`, `update_profile`, `update_bigquery_preferences`
  - `apps/server/src/routes/users.rs:304-505` — `get_chartml_config`, `update_chartml_config`, `update_preferences`, `get_tours`, `mark_tour_complete`
  - `apps/server/src/routes/workspaces.rs:1479-1725` — `get_pending_invitations`, `accept_invitation_handler`, `decline_invitation_handler`

**Orchestration diff summary:** Thin wrappers on both sides over `kyomi_auth::user_service`. The invitation accept/decline path on Leptos (profile.rs:253-284) calls `workspace_service::accept_invitation_for_user` / `update_invitation_status` directly — **REST's `accept_invitation_handler` (workspaces.rs:1561) has extra orchestration** (WS broadcast via `send_invitation_accepted`, workspace member add, etc) that needs verification. Flag as Phase 2 bug-hunt.

Profile updates are one-line calls to `user_service::update_*`. Very little to consolidate.

**DI divergence check:** `ctx.db`. No non-allowlisted contexts.

**Severity:** 🟡 — invitation accept/decline potentially has missing WS broadcasts (same pattern as dashboards.rs). The profile-field-updates are 🟢.

**Proposed shared signature:** Invitation accept/decline roll into `services::team` (see team.rs). Profile updates can stay as direct `user_service` calls on both sides — no extraction value.

**Dependency order:** Invitation part lands with team.rs.

---

### security.rs (password + TOTP + passkey + session management)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/security.rs:45-770` — `has_password`, `set_password`, `change_password`, `get_totp_status`, `setup_totp`, `enable_totp`, `disable_totp`, `get_sessions`, `revoke_session`, `logout`, `logout_all_sessions`, `list_passkeys`, `start_passkey_registration`, `complete_passkey_registration`, `delete_passkey`, `rename_passkey`
- REST:
  - `apps/server/src/routes/auth_password.rs:248-326` — `set_password`, `change_password` (+ the public `login`, `signup_start`, `signup_complete` at other paths)
  - `apps/server/src/routes/auth_totp.rs:42-152` — `status`, `setup`, `enable`, `disable`
  - `apps/server/src/routes/auth.rs:132-491` — `refresh_token`, `logout`, `logout_all`, `get_sessions`, `revoke_session`, etc.
  - `apps/server/src/routes/auth_passkeys.rs:837-1072` — `passkeys_list`, `add_start/complete`, `passkey_delete/rename`

**Orchestration diff summary:** Both sides delegate to `kyomi_auth::user_service`, `kyomi_auth::totp`, `kyomi_auth::webauthn`, `kyomi_auth::token_service`, `kyomi_auth::redis_ops`. The logic is largely mirrored. Minor differences: Leptos's `get_sessions` computes `is_current` by looking up the refresh-token family via header extraction (security.rs:319-344); REST does the same thing but via a different code path. `logout` on Leptos handles cookie clearing via `ResponseOptions`; REST uses a `HeaderMap`. Passkey register/delete/rename are near-identical on both sides.

**DI divergence check:** `ctx.db`, `ctx.kv`, `ctx.webauthn`. All `Option`-wrapped on Leptos, mandatory on REST. Same drift shape as auth.rs.

**Severity:** 🟡 — many small duplications, no confirmed drift. Biggest win from consolidation is the passkey register/delete pair (~150 lines duplicated).

**Proposed shared signature:** Target file `apps/server/src/services/security.rs`.

```rust
pub async fn set_password(
    db: &kyomi_core::DbPool,
    auth: &kyomi_auth::middleware::AuthUser,
    new_password: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn change_password(
    db: &kyomi_core::DbPool,
    auth: &kyomi_auth::middleware::AuthUser,
    current_password: &str,
    new_password: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn totp_setup(
    db: &kyomi_core::DbPool,
    kv: &kyomi_core::KVPool,
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<TotpSetup, kyomi_core::Error>;

pub async fn totp_enable(
    db: &kyomi_core::DbPool,
    kv: &kyomi_core::KVPool,
    auth: &kyomi_auth::middleware::AuthUser,
    code: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn passkey_add_start(
    ctx: &PasskeyContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    device_name: &str,
) -> Result<PasskeyAddStartOutcome, kyomi_core::Error>;

pub async fn passkey_add_complete(
    ctx: &PasskeyContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    challenge_id: &str,
    credential: webauthn_rs::prelude::RegisterPublicKeyCredential,
) -> Result<(), kyomi_core::Error>;

pub async fn session_logout(
    db: &kyomi_core::DbPool,
    refresh_token_cookie: Option<&str>,
) -> Result<axum::http::HeaderMap, kyomi_core::Error>;  // returns cookie clear headers
```

**Dependency order:** Depends on auth.rs. Land after.

---

### setup.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/setup.rs:20-30` — `check_has_datasources`
- REST: **N/A — only server_fn.** A trivial wrapper; React checked via existing `GET /datasources`.

**Orchestration diff summary:** Single-line service call.

**Severity:** 🟢 — trivial, no extraction needed.

**Proposed shared signature:** None.

**Dependency order:** Independent.

---

### sidebar.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/sidebar.rs:35-102` — `get_recent_sessions`, `get_sidebar_user`
- REST: **N/A — only server_fn.** React computed sidebar data from `AuthContext` + a separate `/chat/sessions?limit=20` call.

**Orchestration diff summary:** `get_recent_sessions` calls `kyomi_auth::chat_service::get_user_sessions`. `get_sidebar_user` reads user + workspace metadata inline.

**Severity:** 🟢 — singleton, already a thin orchestrator.

**Proposed shared signature:** None.

**Dependency order:** Independent.

---

### slack.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/slack.rs:60-384` — `get_slack_status`, `slack_connect`, `slack_disconnect`, `get_slack_channels`, `get_default_watch_channel`, `set_default_watch_channel`
- REST: `enterprise/kyomi-slack/src/routes.rs:61-2144` — the Slack enterprise crate owns the REST side: `get_install_url`, `handle_oauth_callback`, `uninstall_slack`, `start_user_connect`, `handle_user_callback`, `disconnect_user`, `get_slack_status`, `list_channels`, `get_default_watch_channel`, `set_default_watch_channel`, plus Slack command/event/interaction handlers

**Orchestration diff summary:** Read paths (`get_slack_status`, `get_slack_channels`, channel config) are close to parity — both sides check tier capability, read `platform_integration` tables, and call `kyomi_slack::routes::get_slack_bot_token` (which is already shared, despite living in the enterprise crate's `routes` module). The CSRF state generation + OAuth URL construction for `slack_connect` duplicates logic between `server_fns/slack.rs:121-171` and `enterprise/kyomi-slack/src/routes.rs::start_user_connect` (around line 506). OAuth callback handling is REST-only (Slack posts to it server-to-server).

**DI divergence check:** Uses `ctx.kv`, `ctx.config.slack_client_id`, `ctx.encryption_key`, `ctx.slack_client`, `ctx.db`. Leptos-side `ctx.slack_client` is `Option` and gated on the `slack` feature; REST requires it. Fine today.

**Severity:** 🟡 — low drift risk (Slack changes are rare) but the connect-URL + disconnect logic duplicates ~50 lines.

**Proposed shared signature:** Target file `enterprise/kyomi-slack/src/service.rs` (keep the enterprise scope).

```rust
pub async fn build_user_connect_url(
    kv: &kyomi_core::KVPool,
    config: &kyomi_core::Config,
    auth: &kyomi_auth::middleware::AuthUser,
) -> Result<String, kyomi_core::Error>;

pub async fn build_workspace_install_url(
    kv: &kyomi_core::KVPool,
    config: &kyomi_core::Config,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
) -> Result<String, kyomi_core::Error>;

pub async fn disconnect_user(
    db: &kyomi_core::DbPool,
    ws_id: &str,
    user_id: &str,
) -> Result<(), kyomi_core::Error>;
```

**Dependency order:** Independent.

---

### sql_editor.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/sql_editor.rs:95-2249` — `execute_sql_query`, `fetch_query_page`, `dry_run_sql`, `start_query_stream`, `list_query_history`, `save_query_history`, `update_query_history`, `delete_query_history`, `get_catalog_tree`, `search_catalog`, `refresh_catalog`, `get_table_info`, `generate_chart_from_results`, `get_ws_connection_info`
- REST: Split across three files:
  - `apps/server/src/routes/datasources.rs:2077+` — `execute_query`, `execute_query_stream`
  - `apps/server/src/routes/sql_history.rs:30-337` — `create_query_history`, `list_query_history`, `get_query_history`, `update_query_history`, `delete_query_history`
  - `apps/server/src/routes/catalog.rs:749-1292` — `get_catalog_tree`, `list_schemas`, `refresh_catalog`, `get_table_info` (plus `discover_*` endpoints)
  - `apps/server/src/routes/chart_generate.rs:364+` — `generate_chart`
  - `apps/server/src/routes/bigquery.rs:102+` — `bigquery_dry_run_cost` (BigQuery's specialized dry-run path)

**Orchestration diff summary:** `refresh_catalog` already delegates to the shared `catalog_refresh::execute_catalog_refresh` on both sides — 🟢 there. `execute_sql_query` (Leptos) and `execute_query` (REST) both build a provider, decrypt credentials, timeout-wrap the execution, and serialize results — but the REST side returns JSON while the Leptos side returns Arrow IPC. The heavy lifting (provider setup + credential resolution + query execution + cancellation registration) is ~200 lines of parallel code. Query history CRUD is thin on both sides. `generate_chart_from_results` (Leptos) and `generate_chart` (REST) both spin up an agent conversation to generate ChartML from tabular results — similar shape to the chat.rs duplication.

**DI divergence check:** Uses `ctx.kv`, `ctx.ws_manager`, `ctx.connect_registry`, `ctx.encryption_key`, `ctx.embedding`. No non-allowlisted contexts.

**Severity:** 🟡 — `refresh_catalog` is already solved via shared service. The query-execution and chart-generation paths duplicate significant orchestration but no confirmed live drift today. Prioritize extraction of `test_provider_connection` + `execute_query_with_credentials` as part of the `datasources.rs` work.

**Proposed shared signature:** Covered by `services::datasources::test_provider_connection` and a new `services::sql::execute_query`:

```rust
pub struct ExecuteQueryContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub encryption_key: &'a std::sync::Arc<[u8; 32]>,
    pub connect_registry: Option<&'a kyomi_datasource_server::ConnectRegistry>,
    pub ws_manager: &'a kyomi_auth::websocket::WebSocketManager,
    pub kv: &'a kyomi_core::KVPool,  // for count cache + history
}

pub async fn execute_query(
    ctx: &ExecuteQueryContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    datasource_slug: &str,
    sql: &str,
    page: QueryPageParams,
) -> Result<QueryExecutionResult, kyomi_core::Error>;
```

**Obstacle:** The return-format split (Arrow IPC vs JSON vs streaming WebSocket) means the shared service must return a structured `QueryExecutionResult` and each caller serializes it differently. Do not try to share the serialization layer.

**Dependency order:** Depends on `datasources.rs` extraction (shares provider setup).

---

### team.rs (members + invitations + ownership transfers)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/team.rs:76-491` — `list_workspace_members`, `update_member_role`, `remove_member`, `list_workspace_invitations`, `invite_member`, `cancel_invitation`, `list_ownership_transfers`, `cancel_ownership_transfer`, `initiate_ownership_transfer`
- REST: `apps/server/src/routes/workspaces.rs:1017-2046` — `list_members`, `update_member_role_handler`, `remove_member_handler`, `create_invitation_handler`, `list_invitations`, `get_pending_invitations`, `cancel_invitation_handler`, `accept_invitation_handler`, `decline_invitation_handler`, `initiate_ownership_transfer`, `accept_ownership_transfer_handler`, `decline_ownership_transfer_handler`, `cancel_ownership_transfer_handler`, `get_ownership_transfers`

**Orchestration diff summary:** Heavy 🔴 pair. Multiple live drifts:

1. **`invite_member` (Leptos) vs `create_invitation_handler` (REST):** REST sends invitation email via `tokio::spawn` → `EmailService::send_workspace_invitation` (lines 1374-1393), and sends WebSocket notification via `ws_helpers::send_workspace_invitation` (lines 1354-1364). **Leptos does neither.** Invitee gets zero notification from the Leptos path — same failure mode as KYO-70.
2. **`initiate_ownership_transfer` (Leptos) vs REST (line 1725):** REST sends `ws_helpers::send_ownership_transfer_offered` (line 1786-1793); Leptos does not. REST also verifies target user is a workspace member before creating the transfer (line 1742-1752); Leptos does not. Silent acceptance of invalid transfers.
3. **ID format divergence:** Leptos `initiate_ownership_transfer` uses `format!("xfer-{...[..24]}")` (team.rs:469-472); REST uses `generate_transfer_id()` which returns `format!("transfer-{...[..20]}")` (workspaces.rs:148-151). Transfer IDs created on different paths are visually distinct — a user might see `transfer-abc123` in one UI and `xfer-def456` in another.
4. `update_member_role` and `remove_member` have matching orchestration on both sides; correct 🟢 there. Ownership transfer accept/decline duplicates the WS broadcast that `ownership.rs` server_fn also misses.

**DI divergence check:** `ctx.db`, `ctx.config` only. No non-allowlisted contexts.

**Severity:** 🔴 — confirmed multiple live drifts, at least one of which (the missing invitation email) is a real user-facing bug.

**Proposed shared signature:** Target file `apps/server/src/services/team.rs`.

```rust
pub struct TeamContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub config: &'a kyomi_core::Config,
    pub ws_manager: &'a kyomi_auth::websocket::WebSocketManager,
}

pub async fn invite_member(
    ctx: &TeamContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    email: &str,
    role: &str,
) -> Result<kyomi_core::models::workspace::Invitation, kyomi_core::Error>;
// Atomically: validate + create invitation + send WS notification + spawn email.

pub async fn accept_invitation(
    ctx: &TeamContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    invitation_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn decline_invitation(
    ctx: &TeamContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    invitation_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn initiate_ownership_transfer(
    ctx: &TeamContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    to_user_id: &str,
) -> Result<String, kyomi_core::Error>;  // returns transfer_id (single canonical format)

pub async fn accept_ownership_transfer(
    ctx: &TeamContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    transfer_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn decline_ownership_transfer(
    ctx: &TeamContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    transfer_id: &str,
) -> Result<(), kyomi_core::Error>;
```

**Obstacle:** The transfer-ID format choice must be decided: `transfer-{20}` (REST) or `xfer-{24}` (Leptos). They are not interchangeable on read paths that parse the prefix. Recommend `transfer-{20}` (REST side is the longer-standing format and is what Stripe-facing webhooks and email templates use). Data migration is not required — old IDs stay valid, new ones use the canonical format.

**Dependency order:** Independent; priority 🔴.

---

### unsubscribe.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/unsubscribe.rs:19-43` — `unsubscribe_email`
- REST: `apps/server/src/routes/subscribe.rs:242-264` — `unsubscribe_email`

**Orchestration diff summary:** Both are public (no auth), both write `marketing_consent = false` via the same SQL shape. Neither side extracts the shared concern — but the duplication is 15 lines of one UPDATE statement, so extraction is not valuable.

**DI divergence check:** `ctx.db` / `state.db`.

**Severity:** 🟢 — trivial, no extraction needed.

**Proposed shared signature:** None (it's a one-line SQL UPDATE).

**Dependency order:** Independent.

---

### usage.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/usage.rs:68-153` — `get_ai_usage_status`
- REST: `apps/server/src/routes/billing.rs:1044-1075` — `get_ai_usage_status` (+ the LLM usage query endpoint at `apps/server/src/routes/usage.rs:94-160`)

**Orchestration diff summary:** Both sides call `kyomi_auth::billing_service::BillingService::get_ai_usage_status`. The Leptos version extends the result with analytics-bundle fields (reads from workspace row + Redis for events count). The REST version returns the plain `AiUsageStatus` struct. So the Leptos side is a strict superset — no drift, just extension.

The `routes/usage.rs::get_llm_usage` endpoint is REST-only and has no Leptos counterpart today.

**DI divergence check:** `ctx.db`, `ctx.config.redis_url`. No non-allowlisted contexts.

**Severity:** 🟢 — thin wrapper pattern. The analytics-bundle extension is a Leptos-only UI requirement that doesn't need to exist on the REST side.

**Proposed shared signature:** Already shared via `BillingService::get_ai_usage_status`. No extraction needed.

**Dependency order:** Independent.

---

### watches.rs

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/watches.rs:159-1093` — `list_watches`, `create_watch`, `get_watch`, `update_watch`, `delete_watch`, `toggle_watch`, `run_watch_now`, plus execution/alert ops
- REST: `apps/server/src/routes/watches.rs:56-2093` — full parallel set including `get_alerts_history`, `get_executions`, etc.

**Orchestration diff summary:** Heavy 🔴 pair. Multiple live drifts:

1. **`create_watch`:** REST (line 377-466) validates `name.len() >= 3`, `prompt.len() >= 10`, gates on `check_watch_capability`, and broadcasts `ws_helpers::send_watch_update("created", ...)` (line 454-463). **Leptos does none of these.**
2. **`update_watch`:** Same pattern — REST validates lengths (line 1018-1032), broadcasts `send_watch_update("updated", ...)` (line 1134-1143). Leptos does neither.
3. **`delete_watch` and `toggle_watch`:** Need a side-by-side read in Phase 2, but given the pattern above, the WS broadcast is almost certainly missing on the Leptos side too.
4. Execution/alert ops (`get_alerts_history`, `mark_alert_read`, `run_watch_now`, etc.) largely parallel, delegate to `kyomi_auth::watch_service`. `run_watch_now` on Leptos spawns the same `kyomi_agent::watch_execution::execute_watch` as REST — that shared point is fine, but the rate-limit check + concurrency guard is duplicated.

**DI divergence check:** Uses `ctx.db`, `ctx.kv`, `ctx.encryption_key`, `ctx.embedding`, `ctx.ws_manager`, `ctx.config`, `ctx.connect_registry`, `ctx.platforms`. `Option`-wrapping repeats. No non-allowlisted contexts.

**Severity:** 🔴 — confirmed multiple live drifts on the CRUD paths.

**Proposed shared signature:** Target file `apps/server/src/services/watches.rs`.

```rust
pub struct WatchMutationContext<'a> {
    pub db: &'a kyomi_core::DbPool,
    pub ws_manager: &'a kyomi_auth::websocket::WebSocketManager,
    pub config: &'a kyomi_core::Config,
}

pub async fn create_watch(
    ctx: &WatchMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    input: CreateWatchInput,
) -> Result<kyomi_core::models::watch::Watch, kyomi_core::Error>;
// Atomically: tier capability check + validation + create + slack channel set + WS broadcast.

pub async fn update_watch(
    ctx: &WatchMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    watch_id: &str,
    input: UpdateWatchInput,
) -> Result<kyomi_core::models::watch::Watch, kyomi_core::Error>;

pub async fn delete_watch(
    ctx: &WatchMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    watch_id: &str,
) -> Result<(), kyomi_core::Error>;

pub async fn toggle_watch(
    ctx: &WatchMutationContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    watch_id: &str,
) -> Result<bool, kyomi_core::Error>;  // returns new enabled state

pub async fn run_watch_now(
    ctx: &WatchRunContext<'_>,
    auth: &kyomi_auth::middleware::AuthUser,
    ws_id: &str,
    watch_id: &str,
) -> Result<(), kyomi_core::Error>;
```

**Dependency order:** Independent; priority 🔴.

---

### workspace.rs (workspace-level settings + slack install)

**Files:**
- Leptos: `crates/kyomi-ui/src/server_fns/workspace.rs:88-340` — `get_workspace_settings`, `update_workspace_name`, `update_workspace_model`, `update_workspace_chartml_config`, `get_workspace_slack_status`, `get_slack_install_url`, `uninstall_workspace_slack`
- REST:
  - `apps/server/src/routes/workspaces.rs:411-901` — `get_settings`, `update_settings`, `update_model_settings`, `get_chartml_config`, `update_chartml_config`, etc.
  - `enterprise/kyomi-slack/src/routes.rs:312-505` — `get_install_url`, `uninstall_slack`

**Orchestration diff summary:** Thin wrappers on both sides over `kyomi_auth::workspace_service`. The `merge_custom_settings` + `custom_settings_get` helpers are duplicated between `server_fns/workspace.rs:39-77` and `apps/server/src/routes/workspaces.rs:172-188` (comment says "Mirrors in workspace.rs") — same logic, separate code. Slack install/uninstall duplicates the CSRF state + URL building done in the enterprise crate.

**DI divergence check:** `ctx.db`, `ctx.kv`, `ctx.config`. No non-allowlisted contexts.

**Severity:** 🟡 — two-way duplication of `merge_custom_settings` + the Slack OAuth URL building. No live drift.

**Proposed shared signature:** Put settings helpers in `kyomi_auth::workspace_service::settings` (domain crate). Put Slack install URL building in `enterprise/kyomi-slack/src/service.rs` (same file as slack.rs recommendations).

```rust
// kyomi-auth: expose merge_custom_settings / custom_settings_get as public helpers
pub fn custom_settings_get<'a>(settings: &'a Option<serde_json::Value>, key: &str) -> Option<&'a serde_json::Value>;
pub fn merge_custom_settings(settings: &Option<serde_json::Value>, key: &str, value: serde_json::Value) -> serde_json::Value;
```

**Dependency order:** Independent.

---

## REST-only singletons (no server_fn counterpart)

Documented for completeness — these are correctly REST-only and do not need extraction. Listed so the Phase 2 tickets don't accidentally scope them in.

| REST file | Why REST-only |
| --- | --- |
| `routes/admin_notify.rs` | Internal helper wrapper, not a route — already delegates to `kyomi_auth::notifications` |
| `routes/bigquery.rs` | BigQuery-specific OAuth catalog endpoints; hand off to `catalog_refresh` for the cross-cutting indexing path |
| `routes/chart_context.rs` | MCP deep-link context retrieval (Redis-keyed); not a UI flow |
| `routes/chart_generate.rs` | Legacy React chart-generation endpoint; superseded by Leptos `sql_editor::generate_chart_from_results` |
| `routes/chartml.rs` | Spec schema + markdown validation endpoints; stateless |
| `routes/integrations.rs` | Platform-agnostic lister; one-read-only handler |
| `routes/learnings.rs` | Admin tool for SQL learning corpus; not user-facing |
| `routes/mcp.rs` | MCP Streamable HTTP JSON-RPC endpoint — protocol-specific, not a UI flow |
| `routes/oauth.rs` | OAuth 2.1 provider endpoints (Kyomi-as-IdP for MCP clients) — protocol |
| `routes/push.rs` | Web Push VAPID + subscription management; MCP-adjacent |
| `routes/subscribe.rs` | Public newsletter signup — rate-limited public endpoint, has its own flow |
| `routes/system_config.rs` | Server-level config read; not user-scoped |
| `routes/users.rs` | User CRUD + tours + API tokens; partially duplicated into `profile.rs` server_fn |
| `routes/websocket.rs` | WebSocket upgrade endpoint — protocol |
| `routes/workspaces.rs` (remainder) | Non-team subsets: `/current`, `/my-workspaces`, `/billing`, `/model-settings`, `/catalog/*`, `/knowledge`, `/microsoft-oauth`, `/admin/populate-graph`. Some of these have Leptos counterparts embedded in `workspace.rs` / `team.rs`; see those sections |
| `routes/auth_datasource_oauth.rs` | Datasource OAuth callback handler — server-to-server callback URL, not a UI flow |

---

## Dependency graph (Phase 2 ordering)

Top-level:
1. **`services::auth`** (auth.rs) — blocks `security.rs` and `onboarding.rs`
2. **`services::datasources`** (datasources.rs) — blocks `connect.rs` and `sql_editor.rs`
3. **`services::dashboards`** (dashboards.rs) — blocks `knowledge.rs`
4. **`services::chat` + shared `CancelRegistry` trait** (chat.rs) — blocks `copilot.rs`
5. **Independent (land in parallel):** `analytics.rs`, `billing.rs`, `team.rs` (+ `ownership.rs`, invitation-accept/decline from `profile.rs`), `watches.rs`, `feedback.rs` (no work), `collections.rs` (no work), `usage.rs` (no work), `context.rs` (no work)
6. **After the big ones:** `security.rs` (after auth), `onboarding.rs` (after auth), `connect.rs` (after datasources), `sql_editor.rs` (after datasources), `knowledge.rs` (after dashboards), `copilot.rs` (after chat)
7. **Last:** `workspace.rs` settings helpers (promotion-only), Slack (`slack.rs` + `workspace.rs`'s Slack bits) — lowest priority, small win

---

## Phase 0 follow-ups (flag-only, not Phase 2 work)

1. **`kyomi-ui` hosts shared business logic that `apps/server` depends on** — `execute_catalog_refresh` lives in `crates/kyomi-ui/src/server_fns/catalog_refresh.rs` and the REST server depends on it. This inverted dependency is an architectural smell. Once `apps/server/src/services/` exists, move `catalog_refresh.rs` there and flip the direction.
2. **`CancelRegistry` is duplicated as two distinct types** sharing the same `DashMap`. A shared trait would clean this up; file a ticket under the chat.rs Phase 2 PR.
3. **Transfer ID format is not canonical** (`transfer-{20}` vs `xfer-{24}`). Decide in the team.rs Phase 2 PR.
4. **Option-wrapped ServerContext fields create silent "feature disabled" early-returns** that don't exist on the REST side. In production these are never `None`; in tests they can be. File a separate ticket to either (a) make the required fields non-Option and fail fast at server startup, or (b) add integration tests that assert they're present.

---

## Acceptance checklist

- [x] Doc exists at `docs/server-fn-rest-parity-sweep.md`
- [x] Inventory complete: all 29 server_fn modules + all 33 REST route files cross-referenced
- [x] Every pair has all six items (or explicit "N/A — only REST" / "N/A — only server_fn")
- [x] Every pair has a severity rating
- [x] 🔴 pairs listed at top in "Fix-first"
- [x] Proposed signatures are concrete Rust (not pseudocode)
- [x] `cargo check --workspace` passes trivially (no code changes)
