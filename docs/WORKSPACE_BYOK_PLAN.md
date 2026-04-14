# Workspace-Level BYOK Migration

**Status:** Planned
**Owner:** TBD
**Target branch:** new branch off `poc/leptos-settings-profile`
**Estimated effort:** 1-2 days (1 engineer + code review)

## Problem

Kyomi's AI configuration is currently architected wrong. The product intent is that **every workspace has exactly one AI configuration**, and workspace admins choose between:

1. **Kyomi credits mode** — workspace uses Kyomi's server-side LLM keys (operator env vars). Each AI request debits `workspaces.ai_bundle_balance_usd`. Admins pick a model from Kyomi's supported list.
2. **BYOK mode** — workspace stores its own provider + API key. All workspace members' AI requests are routed through that key. No bundle consumption. The workspace pays the upstream provider directly.

What exists instead:

- **Per-user-per-browser BYOK via localStorage.** `crates/kyomi-ui/src/pages/settings/ai_provider.rs` writes API keys to `localStorage["kyomi_llm_config"]`. This does nothing server-side. It silently fails to apply to other workspace members, other browsers, or any server-side code path. It's effectively a bug wearing a UI.
- **Server-side env var keys** (`ANTHROPIC_API_KEY`, `LLM_API_KEY`) that every workspace shares unconditionally. This is correct for Kyomi-credits mode and should stay.
- **No workspace-level API key storage.** `workspaces.settings` JSON holds `default_model` but has no fields for provider, encrypted key, or base URL.

Result: a workspace admin today cannot actually BYOK for their team. They can only configure a dead-end localStorage entry on their own browser that no one else sees.

## Target architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ workspace (one row)                                             │
│   ai_provider: 'kyomi' | 'anthropic' | 'openai' | 'gemini'     │
│   ai_api_key_encrypted: Option<String>   (AES-GCM, base64)     │
│   ai_base_url: Option<String>                                  │
│   settings.default_model: Option<String>                       │
│   ai_bundle_balance_usd, ai_credits_used_usd (unchanged)       │
└─────────────────────────────────────────────────────────────────┘
          │
          │ every LLM call site in the codebase looks up config here
          ▼
┌─────────────────────────────────────────────────────────────────┐
│ LLM dispatch                                                    │
│   if provider == Kyomi:                                         │
│     use server env vars, charge workspace bundle                │
│   else:                                                         │
│     decrypt key, route through upstream, no bundle charge       │
└─────────────────────────────────────────────────────────────────┘
```

AI is a workspace-level decision. No per-user override. Admin-only to edit. Non-admins see the current config read-only.

## Work breakdown

Five tracks, must land in this order for the system to stay compilable at each step.

### Track 1 — Schema + encryption primitives

**Migration:** `apps/server/migrations/<timestamp>_workspace_ai_config.sql`

```sql
ALTER TABLE workspaces ADD COLUMN ai_provider TEXT NOT NULL DEFAULT 'kyomi';
ALTER TABLE workspaces ADD COLUMN ai_api_key_encrypted TEXT;
ALTER TABLE workspaces ADD COLUMN ai_base_url TEXT;

-- Optional: CHECK constraint on provider values
ALTER TABLE workspaces ADD CONSTRAINT workspaces_ai_provider_check
  CHECK (ai_provider IN ('kyomi', 'anthropic', 'openai', 'gemini'));
```

Apply to both Postgres and SQLite branches of the migration (follow existing migration patterns in the file — Kyomi supports both backends).

**Encryption module:** `crates/kyomi-auth/src/workspace_secrets.rs` (new file)

- Use the `aes-gcm` crate. Add to `crates/kyomi-auth/Cargo.toml` if not already present.
- Reads a base64-encoded 32-byte master key from env var `WORKSPACE_SECRETS_KEY`. Fail fast at server startup if the env var is missing AND the server is in SaaS mode (`config.self_hosted == false`). In self-hosted mode, absence of the key means BYOK is disabled — return a clear error if a workspace tries to use it.
- Public API:
  ```rust
  pub fn encrypt_secret(plaintext: &str) -> Result<String, WorkspaceSecretError>;
  pub fn decrypt_secret(encoded: &str) -> Result<String, WorkspaceSecretError>;
  pub fn is_available() -> bool;  // true if WORKSPACE_SECRETS_KEY is set
  ```
- Encoding format: `base64(nonce [12B] || ciphertext || tag [16B])`
- Tests:
  - roundtrip encrypt/decrypt
  - wrong master key → decrypt fails
  - tampered ciphertext (flip a byte) → decrypt fails (AEAD integrity)
  - empty string roundtrips
  - missing env var → `is_available() == false`

**Env var documentation:** Add `WORKSPACE_SECRETS_KEY` to `.env.example` with a generation command in a comment:

```
# Master key for encrypting workspace-level secrets (API keys).
# Generate with: openssl rand -base64 32
# REQUIRED in SaaS mode. Without it, the server will refuse to start.
WORKSPACE_SECRETS_KEY=
```

**Config loader:** `crates/kyomi-core/src/config.rs`

- Add `pub workspace_secrets_key: Option<String>` field
- Load from `WORKSPACE_SECRETS_KEY` env var
- Add a startup check in `apps/server/src/main.rs` (or wherever the server boots) that errors out if `config.self_hosted == false && workspace_secrets_key.is_none()`.

### Track 2 — Workspace AI config service

**New service module:** `crates/kyomi-auth/src/workspace_ai_config.rs`

Types:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceAiProvider {
    Kyomi,
    Anthropic,
    OpenAI,
    Gemini,
}

impl WorkspaceAiProvider {
    pub fn as_str(&self) -> &'static str { /* "kyomi", "anthropic", ... */ }
    pub fn from_str(s: &str) -> Result<Self, WorkspaceAiConfigError>;
}

#[derive(Clone, Debug)]
pub struct WorkspaceAiConfig {
    pub provider: WorkspaceAiProvider,
    pub model: Option<String>,         // from workspaces.settings.default_model
    pub api_key: Option<String>,       // DECRYPTED. None for Kyomi mode.
    pub base_url: Option<String>,
}

impl WorkspaceAiConfig {
    /// True if this workspace uses its own provider (not Kyomi).
    pub fn is_byok(&self) -> bool { self.provider != WorkspaceAiProvider::Kyomi }
}
```

Functions:

```rust
pub async fn load(db: &DbPool, workspace_id: &str) -> Result<WorkspaceAiConfig, ...>;

pub struct UpdateWorkspaceAiConfigInput {
    pub provider: WorkspaceAiProvider,
    pub api_key: Option<String>,       // plaintext, will be encrypted
    pub base_url: Option<String>,
    pub model: Option<String>,
}

pub async fn update(db: &DbPool, workspace_id: &str, input: UpdateWorkspaceAiConfigInput)
    -> Result<(), ...>;
```

Semantics:

- `load` reads the workspace row, decrypts the stored key if present, returns the config. If the provider is Kyomi, api_key is always None (even if there's a leftover encrypted blob, which there shouldn't be).
- `update` validates:
  - If provider is Kyomi, clear `ai_api_key_encrypted` and `ai_base_url`
  - If provider is BYOK, require `api_key` to be present (or an existing stored key — allow updates that change only the model without re-entering the key)
  - Encrypts `api_key` before writing
- Uses `update_workspace_settings` or similar existing helper for writing `default_model` into the settings JSON
- Tests: roundtrip load/update, switching from Kyomi to BYOK and back, updating model without changing key

**Register module:** `crates/kyomi-auth/src/lib.rs` — add `pub mod workspace_ai_config;` and `pub mod workspace_secrets;`.

### Track 3 — LLM routing audit

Every code path that currently calls an LLM using `ctx.config.anthropic_api_key` or `ctx.config.llm_api_key` must be updated to:

1. Look up `WorkspaceAiConfig` for the current workspace
2. If `provider == Kyomi`: use server env vars (current behavior). Continue to log AI usage and debit `ai_credits_used_usd` via existing helpers.
3. If BYOK: build the LLM client with the workspace-provided key/base_url. Do NOT log to `ai_credits_used_usd`. Log request count only.

**Call site audit list** (grep and update each):

- `crates/kyomi-agent/src/anthropic.rs` — Anthropic client construction
- `crates/kyomi-agent/src/openai.rs` — OpenAI client construction
- `crates/kyomi-agent/src/gemini.rs` — Gemini client construction
- `crates/kyomi-agent/src/prompt.rs` — model selection / dispatch
- `crates/kyomi-agent/src/watch.rs` — watch LLM calls
- `crates/kyomi-agent/src/chart_builder_copilot.rs` — chart builder copilot
- `crates/kyomi-agent/src/dashboard_copilot.rs` — dashboard copilot
- `crates/kyomi-agent/src/knowledge_graph.rs` — knowledge graph builder
- `crates/kyomi-auth/src/billing_service.rs` — `log_ai_usage`, cost calculation
- `crates/kyomi-ui/src/server_fns/chat.rs`
- `crates/kyomi-ui/src/server_fns/copilot.rs`
- `crates/kyomi-ui/src/server_fns/home.rs`
- `crates/kyomi-ui/src/server_fns/trial.rs`
- Any service constructor in kyomi-agent that currently takes `&Config` — change to take `WorkspaceAiConfig` or accept both

Audit command (grep for everything):

```bash
rg -n "anthropic_api_key|llm_api_key|llm_base_url|llm_provider" crates/ apps/
```

**Do not** just update the obvious ones. Anything that constructs an LLM client anywhere in the workspace must go through workspace config lookup. If the audit is too large to complete in a session, STOP and report — don't leave half the code on the old path.

**Billing implication:** `log_ai_usage` (in `billing_service.rs`) needs a `is_byok: bool` parameter. When true, don't update `ai_credits_used_usd`. Optionally update a new `ai_byok_request_count` column for diagnostics (add to the same migration if desired, or defer).

**Trial chat:** `server_fns/trial.rs` — trial chat is unauthenticated (no workspace). It should continue to use Kyomi's server-side keys and not go through workspace lookup. Leave it alone but note in a comment.

### Track 4 — Server functions

**New file:** `crates/kyomi-ui/src/server_fns/ai.rs`

Types:

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceAiConfigView {
    pub provider: String,              // "kyomi" | "anthropic" | "openai" | "gemini"
    pub model: Option<String>,
    pub has_api_key: bool,             // never return the key itself
    pub base_url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestAiConfigResult {
    pub ok: bool,
    pub message: String,               // error text if !ok, "Connection OK" if ok
}
```

Server functions:

```rust
/// Read current config. All workspace members can read.
#[server(prefix = "/leptos-api")]
pub async fn get_workspace_ai_config() -> Result<WorkspaceAiConfigView, ServerFnError>;

/// Update config. Admin-only.
/// api_key: None means "don't change the stored key". Some(k) means encrypt and store k.
/// To switch to Kyomi mode, pass provider="kyomi" — the stored key will be cleared.
#[server(prefix = "/leptos-api")]
pub async fn update_workspace_ai_config(
    provider: String,
    api_key: Option<String>,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<WorkspaceAiConfigView, ServerFnError>;

/// Test a candidate config without persisting. Admin-only.
/// Makes a minimal chat completion against the provider with the given creds
/// and returns ok/error. Never writes to DB.
#[server(prefix = "/leptos-api")]
pub async fn test_workspace_ai_config(
    provider: String,
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
) -> Result<TestAiConfigResult, ServerFnError>;
```

Auth: `update` and `test` require workspace admin OR owner. `get` requires any authenticated workspace member.

Register all three in `crates/kyomi-ui/src/lib.rs` via `register_explicit::<...>()`.

**Delete:** The existing `test_ai_provider` server fn in `crates/kyomi-ui/src/server_fns/ai_provider.rs` if it's only used by the old localStorage flow. Check for other callers first.

### Track 5 — Frontend

**New AI settings page:** rewrite `crates/kyomi-ui/src/pages/settings/ai.rs` entirely.

Structure (top to bottom):

1. **Page header** — "AI" in Instrument Serif, subtitle in DM Sans: "Choose how AI is billed for your workspace and which model to use."

2. **Status banner** — loaded from `get_workspace_ai_config`. Always visible.
   - Kyomi mode: `bg-surface-alt border-border`, text: "Using Kyomi credits · Claude Sonnet 4 · $5.00 remaining in token bundle". Include a small link "Buy more" that deep-links to `/settings/billing` (visible only to owners).
   - BYOK mode: `bg-accent-light border-accent/40`, text: "Using your Anthropic API key · Claude Sonnet 4 · Workspace bundle not consumed"
   - Loading skeleton while resource is pending.

3. **Mode selector** — two radio-style cards in a 2-column grid.
   - Card A: "Kyomi credits" — title in DM Sans 600, body text in DM Sans 400 muted: "Pay Kyomi per request. Use our infrastructure, no setup."
   - Card B: "Your own API key" — title, body: "Bring your own Anthropic, OpenAI, or Gemini key. Pay the provider directly."
   - Selected state: `border-2 border-accent bg-accent-light/30` + checkmark icon top-right
   - Unselected: `border border-border hover:border-border-strong`
   - Non-admin: cards are shown but not clickable, cursor not-allowed, with a helper text below the whole selector: "Only workspace admins can change AI configuration."

4. **Active mode panel** — swaps based on selected mode. Wrapped in a Card.

   **Kyomi credits mode:**
   - Label: "Model"
   - Grouped dropdown: only Kyomi-supported models (the curated list — see "Model catalog" below)
   - Helper text: "Kyomi provides the LLM infrastructure. Your admin picks the model; all workspace members use it."
   - Save button appears when dirty
   - Admin-only to edit. Non-admins see the model as read-only text.

   **BYOK mode:**
   - Label: "Provider" — dropdown: Anthropic / OpenAI / Gemini
   - Label: "API key" — password input, masked. Shows `••••••••` if an existing key is stored. Placeholder: "sk-ant-..." (or similar). Buttons next to the input: "Test" and "Save".
   - Label: "Model" — grouped dropdown scoped to the selected provider, with an "Advanced: custom model ID" option revealing a text input
   - Collapsed disclosure "Advanced ▸": base URL override input
   - Helper text under the API key: "Stored encrypted. All workspace members automatically use this key for AI requests."
   - Test button calls `test_workspace_ai_config`. Shows a green check + "Connection OK" or a red X + error message inline.
   - Save button calls `update_workspace_ai_config`. On success, refreshes the status banner and shows a toast.
   - Admin-only. Non-admins see the fields disabled.

5. **Feature context footer** — muted one-liner: "Powers: Chat · Watch · Dashboard Copilot · Chart Builder"

**Model catalog:** `crates/kyomi-ui/src/pages/settings/ai_models.rs` (new file)

```rust
pub struct ModelOption { pub id: &'static str, pub label: &'static str }

pub const ANTHROPIC_MODELS: &[ModelOption] = &[
    ModelOption { id: "claude-sonnet-4-5-20250929", label: "Claude Sonnet 4.5" },
    ModelOption { id: "claude-opus-4-20250514", label: "Claude Opus 4" },
    ModelOption { id: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5" },
];

pub const OPENAI_MODELS: &[ModelOption] = &[
    ModelOption { id: "gpt-4o", label: "GPT-4o" },
    ModelOption { id: "gpt-4o-mini", label: "GPT-4o mini" },
];

pub const GEMINI_MODELS: &[ModelOption] = &[
    ModelOption { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
    ModelOption { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash" },
];

/// Models Kyomi supports in credits mode (curated subset).
pub const KYOMI_CREDITS_MODELS: &[ModelOption] = &[
    ModelOption { id: "claude-sonnet-4-5-20250929", label: "Claude Sonnet 4.5" },
    ModelOption { id: "claude-haiku-4-5-20251001", label: "Claude Haiku 4.5" },
];
```

Cross-check these model IDs against `crates/kyomi-agent/src/{anthropic,openai,gemini}.rs` for canonical strings. Fix any that drift.

**Visual conformance:** use the tokens from `DESIGN.md`:
- Headings: Instrument Serif
- Body/labels: DM Sans
- Model IDs in helper text and advanced fields: Geist Mono tabular-nums
- Accent: `--accent` (#D97706) for selected states and BYOK active banner
- Surface-alt (#F5F3EF) for Kyomi mode banner

**Deletions:**

- Delete `crates/kyomi-ui/src/pages/settings/ai_provider.rs` entirely (the whole file)
- Remove `pub mod ai_provider;` from `crates/kyomi-ui/src/pages/settings/mod.rs`
- Remove any import of `AiProviderCard` anywhere in the codebase — grep first
- Delete `check_byok_key` and `LLM_CONFIG_STORAGE_KEY` usage in `crates/kyomi-ui/src/pages/settings/billing.rs`. The "BYOK configured" badge on the AI Credits card should either go away entirely or be replaced with a derivation from the workspace AI config (e.g., "Workspace uses BYOK — credits not consumed" shown when `provider != kyomi`).

## Data flow: a worked example

Admin Alice toggles the workspace from Kyomi credits to BYOK with an OpenAI key:

1. Alice opens `/settings/ai`. Page calls `get_workspace_ai_config()`. Returns `{ provider: "kyomi", model: "claude-sonnet-4-5-20250929", has_api_key: false, base_url: None }`. Status banner shows "Using Kyomi credits".
2. Alice clicks the "Your own API key" mode card. Active panel swaps to the BYOK form.
3. Alice selects Provider: OpenAI, pastes her key, selects Model: GPT-4o. Clicks "Test".
4. Frontend calls `test_workspace_ai_config("openai", "<key>", None, Some("gpt-4o"))`. Server makes a minimal chat completion to OpenAI. Returns `{ ok: true, message: "Connection OK" }`. UI shows green check.
5. Alice clicks "Save".
6. Frontend calls `update_workspace_ai_config("openai", Some("<key>"), None, Some("gpt-4o"))`. Server:
   - Validates admin role
   - Encrypts the key with `WORKSPACE_SECRETS_KEY`
   - Writes `ai_provider='openai'`, `ai_api_key_encrypted='<base64>'`, `ai_base_url=NULL`, and `settings.default_model='gpt-4o'`
   - Returns the updated view: `{ provider: "openai", model: "gpt-4o", has_api_key: true, base_url: None }`
7. UI refreshes the status banner — now shows "Using your OpenAI API key · GPT-4o · Workspace bundle not consumed" on amber background.
8. Teammate Bob opens a chat. His chat handler loads `WorkspaceAiConfig` for the workspace, sees `provider=openai`, decrypts the key, routes the chat completion through OpenAI with the workspace key. No bundle debit. Bob's chat just works — no config required on his end.

## Security considerations

- **Master key handling:** `WORKSPACE_SECRETS_KEY` must be in a secrets manager in production, not committed anywhere. Document this in `.env.example` and in the SUBSCRIPTION_FLOW.md sibling doc.
- **Key rotation:** not in scope for this migration but note in the encryption module comments: to rotate the master key, decrypt all `ai_api_key_encrypted` values with the old key, re-encrypt with the new key, update env var, redeploy. Low risk because workspaces that can't decrypt just fall back to Kyomi mode (with a clear error telling the admin to re-enter their key).
- **API key never leaves the server.** `get_workspace_ai_config` returns a boolean `has_api_key`, never the key. The frontend never sees plaintext.
- **Logs:** scrub API keys from error messages. Never `tracing::info!` the key even in debug builds.
- **Trial chat isolation:** trial chat currently uses Kyomi server env vars and has no workspace. Keep that path untouched.

## Testing plan

### Unit tests

- `workspace_secrets`: roundtrip, wrong key, tampered ciphertext, empty string
- `workspace_ai_config::update`: Kyomi → BYOK → Kyomi transitions; model update without key change
- LLM client construction: given a WorkspaceAiConfig, the client targets the right provider/base_url
- `log_ai_usage`: BYOK mode does not touch `ai_credits_used_usd`

### Integration tests

- `get_workspace_ai_config` returns the right view for a Kyomi-mode workspace
- `update_workspace_ai_config` as non-admin returns "Workspace admin access required"
- `update_workspace_ai_config` switching to BYOK stores encrypted key, `has_api_key=true`
- `update_workspace_ai_config` switching back to Kyomi clears the stored key
- `test_workspace_ai_config` with a bad key returns `ok: false` with a useful error

### Manual smoke test

- Fresh workspace in Kyomi mode → chat works, AI usage logs hit `ai_credits_used_usd`
- Switch to BYOK (use a real test key) → chat works, usage logs do NOT hit `ai_credits_used_usd`
- Second member of the same workspace sees BYOK active in the status banner and their chats also work without any per-user setup
- Non-admin member cannot edit the config (buttons disabled, direct server calls rejected)

### Build verification

- `cargo check --workspace` clean
- `cargo check -p kyomi-ui --target wasm32-unknown-unknown` clean
- `cd crates/kyomi-ui && trunk build --public-url /leptos/` clean
- `cargo build --profile dev-server -p kyomi-server` clean

## Rollout

1. Ship schema migration — new workspaces get `ai_provider='kyomi'` by default, existing workspaces same. No behavior change.
2. Ship encryption module + config service — no call sites use it yet. No behavior change.
3. Ship LLM routing audit — every call site loads `WorkspaceAiConfig`. In Kyomi mode, behavior is identical to before. In BYOK mode, the feature becomes usable. Ship this as one atomic change — half-migrated is worse than unmigrated.
4. Ship new AI settings page + server functions — users can now configure BYOK. Delete old localStorage code.

Optional feature flag: gate the BYOK mode card on a `byok_enabled` workspace capability so you can roll it out to a subset of workspaces first. Add via the existing `capabilities` map in UserContext. Cut the flag after a week of clean operation.

## Open questions to resolve before coding

1. **Crypto library:** `aes-gcm` (RustCrypto) is the default recommendation. Is there a preferred crypto crate already in the workspace? Check `Cargo.lock`.
2. **Feature flag:** ship atomically or behind `capabilities["byok_enabled"]`?
3. **Existing workspaces on migration:** default to `kyomi` provider (safe, matches current behavior). Confirm no opt-in needed.
4. **Model catalog truth source:** does the ChartML/agent crate already have a canonical list of models per provider? If so, reuse it instead of hardcoding `ai_models.rs`. If not, the plan stays as written.
5. **BYOK test billing:** when an admin clicks "Test" on a BYOK key, it makes a real LLM call that costs the workspace a few cents on their own account. Is that OK, or should Test be limited to a free endpoint (e.g., Anthropic has a `/models` GET that's free — consider preferring that where available)?
6. **Trial chat:** confirmed out of scope (no workspace, uses server keys). Any other anonymous code paths we need to worry about?

## Out of scope

- Key rotation tooling
- Per-user AI overrides (intentionally rejected — product is workspace-level)
- UI for inspecting BYOK request counts / diagnostics
- Provider-specific rate limits or cost caps on BYOK mode
- Audit log of config changes (would be nice but separate work)
- Migration of any existing localStorage BYOK configs (delete and ignore — they were never really working)

## Files touched (expected)

**New:**
- `apps/server/migrations/<ts>_workspace_ai_config.sql`
- `crates/kyomi-auth/src/workspace_secrets.rs`
- `crates/kyomi-auth/src/workspace_ai_config.rs`
- `crates/kyomi-ui/src/server_fns/ai.rs`
- `crates/kyomi-ui/src/pages/settings/ai_models.rs`

**Modified:**
- `crates/kyomi-auth/src/lib.rs` (new modules)
- `crates/kyomi-auth/Cargo.toml` (aes-gcm dep)
- `crates/kyomi-core/src/config.rs` (`workspace_secrets_key` field)
- `apps/server/src/main.rs` or equivalent (startup check)
- `.env.example` (new env var)
- `crates/kyomi-auth/src/billing_service.rs` (`log_ai_usage` BYOK flag)
- `crates/kyomi-agent/src/{anthropic,openai,gemini,prompt,watch,chart_builder_copilot,dashboard_copilot,knowledge_graph}.rs` (workspace config lookup)
- `crates/kyomi-ui/src/server_fns/{chat,copilot,home}.rs` (workspace config lookup)
- `crates/kyomi-ui/src/lib.rs` (register new server fns)
- `crates/kyomi-ui/src/pages/settings/ai.rs` (full rewrite)
- `crates/kyomi-ui/src/pages/settings/mod.rs` (remove ai_provider, add ai_models)
- `crates/kyomi-ui/src/pages/settings/billing.rs` (remove `check_byok_key` and related)

**Deleted:**
- `crates/kyomi-ui/src/pages/settings/ai_provider.rs`
- `crates/kyomi-ui/src/server_fns/ai_provider.rs` (if unused after deletion above — check)

## Success criteria

- A workspace admin can set a BYOK OpenAI key once and every workspace member's AI requests route through it automatically.
- Switching between Kyomi credits and BYOK is a single admin action with no code changes, no redeploys, no env var edits.
- No plaintext API keys are stored anywhere in the database.
- `cargo check --workspace` is clean; `trunk build` is clean.
- Non-admin members cannot change the config and see a clear message explaining why.
- Deleting the localStorage BYOK code is complete — no references to `kyomi_llm_config` remain in the codebase.
- Code review by `code-review-architect` passes with zero critical or major issues.
