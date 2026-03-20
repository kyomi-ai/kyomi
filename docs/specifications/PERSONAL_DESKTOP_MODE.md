# Personal Desktop Mode — Design Specification

## Overview

Add a "personal" mode to Kyomi that transforms the desktop app into a zero-auth, single-user experience. The user launches the app, enters their LLM API key, connects a datasource, and starts working. External AI tools (Claude Code, Claude Desktop, Cursor) connect to Kyomi's MCP server on localhost to create dashboards, query data, and manage knowledge — all without authentication.

## Primary Use Case

1. User opens Kyomi Desktop
2. First-run: enters API key, connects a datasource
3. Switches to Claude Code
4. Tells Claude Code: "create a dashboard showing daily revenue by region"
5. Claude Code calls Kyomi MCP tools: `browse_catalog` → `query_datasource` → `create_dashboard`
6. User views the dashboard in Kyomi Desktop

## Modes

Kyomi has three deployment modes:

| Mode | Auth | Users | Database | Use case |
|------|------|-------|----------|----------|
| **SaaS** | Full (OAuth, passkeys, password, 2FA) | Multi-tenant | Postgres + Redis | app.kyomi.ai |
| **Self-hosted** | Full (password, optional OAuth) | Multi-user | Postgres + Redis or SQLite | Team server |
| **Personal** | None | Single user | SQLite + in-memory KV | Desktop app |

### Configuration

New enum replaces `self_hosted: bool` + `edition`:

```rust
pub enum KyomiMode {
    Saas,
    SelfHosted,  // replaces self_hosted=true
    Personal,    // new
}
```

- `KYOMI_MODE=personal` — set automatically by `kyomi-desktop`
- Backward compat: `SELF_HOSTED=true` maps to `KyomiMode::SelfHosted`
- No env var / `SELF_HOSTED` not set maps to `KyomiMode::Saas`

## 1. Zero-Auth Auto-Provisioning

### Backend

On first boot in personal mode, if `users` table is empty:

- Create user: `user_id=user-local`, `name="Local User"`, `email="local@localhost"`, `verified=true`, `active=true`
- Create workspace: `workspace_id=workspace-local`, `name="My Workspace"`
- Create workspace membership: owner role
- All capability checks return Enterprise-level (unlimited everything)

### Auth Middleware Bypass

In personal mode, the `AuthUser` extractor skips JWT validation entirely and injects the local user context. No token check, no cookie check. The server only listens on `127.0.0.1` — there is no security boundary.

```rust
// In middleware.rs AuthUser extractor
if config.mode == KyomiMode::Personal {
    return Ok(AuthUser::local_user(db)); // loads user-local from DB
}
// ... existing JWT validation
```

### Frontend

- `GET /api/v1/system/config` returns `{ "mode": "personal", ... }`
- `AuthContext` detects personal mode → sets `isAuthenticated=true` immediately
- `ProtectedRoute` lets everything through
- Login/signup pages never rendered

## 2. Onboarding — Connect Data, Then Connect AI Tool

### First-Run Setup Wizard

Shown on first launch when no datasource is configured:

**Step 1: Connect Data**
- Existing datasource onboarding flow (connect BigQuery, Postgres, etc.)
- OR "Explore with sample data"

**Step 2: Connect Your AI Tool**
- Primary message: "Use Kyomi from Claude Code, Claude Desktop, or Cursor"
- Show MCP config snippet with copy button, pre-filled with `http://localhost:{port}/mcp`
- Tabs for each client:
  - **Claude Code**: "Add to your MCP settings or run: `claude mcp add kyomi http://localhost:3000/mcp`"
  - **Claude Desktop**: JSON snippet for `claude_desktop_config.json`
  - **Cursor**: "Connect with Cursor" deep link button (existing pattern)
- "I've connected" button → done
- Small link at bottom: "Or use Kyomi's built-in chat instead →" (goes to AI Provider settings)

**Step 3: Done** → land on dashboards/explorer view (not chat — they'll chat from their AI tool)

### API Key (Optional — In Settings)

For users who want the in-app chat agent and automated watches, an "AI Provider" section in settings:

- Provider dropdown: Anthropic, OpenAI, Gemini
- API key field (password-masked)
- Model override (optional)
- Base URL override (optional — for Ollama, vLLM, Azure OpenAI)
- "Test Connection" button
- Not required for MCP usage — only for built-in chat and watch execution

### API Key Storage

When configured, LLM provider config stored in `workspace_integrations` table:

```sql
-- Uses existing table, no schema changes
INSERT INTO workspace_integrations (id, workspace_id, platform_type, config, installed_by)
VALUES ('llm-config', 'workspace-local', 'llm_provider', '{
  "provider": "anthropic",
  "api_key": "<encrypted>",
  "model": null,
  "base_url": null
}', 'user-local');
```

- Encrypted with local `ENCRYPTION_KEY` (same pattern as datasource credentials)
- Backend reads from DB on each agent invocation — allows changing without restart
- Falls back to env vars (`LLM_API_KEY`, `ANTHROPIC_API_KEY`) if no DB config exists

## 3. MCP Server — Zero-Auth Localhost

### Streamable HTTP (Primary)

The existing `/mcp` endpoint works as-is. In personal mode, auth middleware bypass means no OAuth flow needed.

**Claude Code config** (`~/.claude.json` or project MCP settings):
```json
{
  "mcpServers": {
    "kyomi": {
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

**Claude Desktop config** (`claude_desktop_config.json`):
```json
{
  "mcpServers": {
    "kyomi": {
      "url": "http://localhost:3000/mcp"
    }
  }
}
```

### Port Strategy

- Desktop app defaults to port `3000`
- If `3000` is taken, picks the next available port and displays it prominently in the app
- MCP settings panel in the app shows the current URL and copy-paste config snippets
- User can override port via `PORT` env var if needed

### OAuth Discovery in Personal Mode

- `/.well-known/oauth-authorization-server` — not mounted (or returns empty/404)
- MCP clients that check for OAuth discover it's not required and connect directly
- `Mcp-Session-Id` still works for session management

### stdio Transport (Future)

Add `kyomi-desktop --mcp-stdio` flag that:
- Reads JSON-RPC from stdin
- Proxies to `http://localhost:{port}/mcp`
- Writes responses to stdout

This enables the standard MCP pattern where the client spawns the server as a subprocess. Lower priority since Claude Code and Claude Desktop both support HTTP URLs.

### Available MCP Tools

All existing MCP tools work in personal mode (25+ tools):
- `query_datasource`, `validate_sql`, `get_table_info`, `browse_catalog`, `list_datasources`
- `create_dashboard`, `modify_dashboard`, `delete_dashboard`, `search_dashboards`, `get_dashboard_info`
- `render_chart`, `get_chartml_spec`
- `create_watch`, `update_watch`, `delete_watch`, `search_watches`, `get_watch_info`, `trigger_watch`
- `search_knowledge`, `read_knowledge_file`, `list_knowledge_files`
- `forecast_data`
- `list_analytics_sites`, `create_analytics_site`, `update_analytics_site`, `delete_analytics_site`

## 4. UI Changes

### LLM-Required Features — Empty State with CTA

When no LLM provider is configured, the following features show a helpful empty state instead of broken UI:

| Feature | Empty State Message |
|---------|-------------------|
| **Chat** | "Chat requires an AI provider. You can use Kyomi from Claude Code via MCP, or add your own API key in Settings → AI Provider." |
| **Watches** (create/edit) | "Watches require an AI provider to analyze results. Add your API key in Settings → AI Provider." |
| **Copilot** (dashboard/chart builder sidebars) | "Copilot requires an AI provider. Add your API key in Settings → AI Provider." |

Each empty state includes:
- A button linking to AI Provider settings
- A secondary link to the MCP connection docs ("Or use Claude Code instead →")

When an LLM provider IS configured, these features work normally.

### Hide in Personal Mode

| Feature | Reason |
|---------|--------|
| Login / signup pages | No auth |
| Logout button | Single user, can't log out |
| User profile / avatar menu | Simplify to settings gear |
| Billing / subscription | Everything unlocked |
| Team / invite members | Single user |
| Workspace switcher | Single workspace |
| Slack integration | No server for webhooks |
| Watch email alerts | No SMTP |
| Push notifications | Localhost, no service worker |
| Password / 2FA / passkey settings | No auth |
| Google OAuth settings | No auth |
| Website analytics | No public site |

### Show in Personal Mode (New or Promoted)

| Feature | Description |
|---------|-------------|
| **AI Provider** (settings, optional) | Provider, API key, model, base URL — only needed for built-in chat and watches |

### MCP Connection Panel (Existing — Adapt)

`ProfileSettings.jsx` already has an MCP Connection section with the server URL, copy button, and Cursor deep link. For personal mode:
- Remove "You'll be prompted to authorize via your browser" text (no auth)
- URL auto-detects correctly (`http://localhost:{port}/mcp`)
- Add Claude Code and Claude Desktop config snippets alongside the existing Cursor button

## 5. Implementation Plan

### Phase 1: Backend — Personal Mode Core
1. Add `KyomiMode` enum to config, wire up `KYOMI_MODE=personal`
2. Auto-provision user + workspace on first boot
3. Auth middleware bypass for personal mode
4. System config endpoint returns `mode: "personal"`
5. LLM provider config read from `workspace_integrations` (with env var fallback)
6. Skip OAuth discovery endpoints in personal mode

### Phase 2: Frontend — Setup Wizard & Auth Bypass
1. `AuthContext` / `SystemConfigContext` detect personal mode
2. Skip login routing, auto-authenticate
3. Setup wizard: Step 1 connect datasource, Step 2 MCP connection instructions
4. Detect "needs setup" state (no datasources configured)

### Phase 3: Frontend — UI Cleanup
1. Hide auth/billing/team UI elements in personal mode
2. New AI Provider settings section
3. New MCP Connection settings section
4. Simplified sidebar (no user menu, workspace switcher)

### Phase 4: Desktop Binary Updates
1. `kyomi-desktop` sets `KYOMI_MODE=personal`
2. Fixed port 3000 with fallback
3. Port display in window title or status bar

## 6. Files to Modify

### Backend
- `kyomi-core/src/config.rs` — `KyomiMode` enum, parsing
- `kyomi-core/src/standalone.rs` — set personal mode for desktop
- `kyomi-auth/src/middleware.rs` — bypass auth in personal mode
- `kyomi-api/src/routes/system_config.rs` — return mode
- `kyomi-api/src/routes/mcp.rs` — skip OAuth in personal mode
- `kyomi-api/src/lib.rs` — auto-provision on startup
- `kyomi-agent/src/provider.rs` — read LLM config from DB
- `kyomi-desktop/src/main.rs` — set `KYOMI_MODE=personal`, fixed port

### Frontend
- `context/AuthContext.jsx` — personal mode auto-auth
- `context/SystemConfigContext.jsx` — expose mode
- `App.jsx` — skip login routes in personal mode
- `pages/Login.jsx` — never rendered in personal mode
- `components/Sidebar.jsx` — hide team/billing items
- New: `pages/SetupWizard.jsx` — first-run API key + datasource
- New: `components/settings/AIProviderSettings.jsx`
- `components/settings/ProfileSettings.jsx` — adapt MCP section for personal mode (no auth text, add Claude Code/Desktop snippets)

## 7. Non-Goals (Future Work)

- **Cloud-connected desktop** — Tauri webview pointed at app.kyomi.ai or self-hosted URL (modes 2 & 3). Separate effort, no embedded server needed.
- **stdio MCP transport** — `kyomi-desktop --mcp-stdio` proxy. Nice to have, HTTP works for all current clients.
- **Multi-provider simultaneous** — e.g., Anthropic for chat + OpenAI for embeddings. One provider at a time for v1.
- **Offline/local LLM** — Ollama integration via base URL override works today, but no dedicated UI for it.
