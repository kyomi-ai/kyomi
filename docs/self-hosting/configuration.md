# Configuration Reference

All Kyomi configuration is done through environment variables, set in the `.env` file that Docker Compose loads automatically.

The `.env.example` (Community) and `.env.enterprise.example` (Enterprise) files in the `deploy/` directory contain all available variables with comments.

## Required Variables

These must be set for Kyomi to start.

### Database

| Variable | Type | Description |
|---|---|---|
| `POSTGRES_PASSWORD` | string | Password for the PostgreSQL database. Used by both the `postgres` container and Kyomi's `DATABASE_URL`. Generate with: `openssl rand -base64 32 \| tr -dc 'a-zA-Z0-9' \| head -c 32` |

> **Note:** You do not need to set `DATABASE_URL` when using the provided Docker Compose files. It is constructed automatically from `POSTGRES_PASSWORD`.

### Security Keys

| Variable | Type | Description |
|---|---|---|
| `JWT_SECRET_KEY` | string | Secret key for signing JWT access and refresh tokens. Generate with: `openssl rand -base64 32` |
| `ENCRYPTION_KEY` | string | AES-256-GCM key for encrypting sensitive data at rest (datasource credentials). Must be base64url-encoded. Generate with: `openssl rand -base64 32 \| tr '+/' '-_' \| tr -d '='` |

### LLM Provider

| Variable | Type | Description |
|---|---|---|
| `LLM_PROVIDER` | string | The LLM provider to use. One of: `anthropic`, `openai`, `gemini`. For OpenAI-compatible APIs (Ollama, vLLM, etc.), use `openai` and set `LLM_BASE_URL`. |
| `LLM_API_KEY` | string | API key for the chosen LLM provider. |

## Server

| Variable | Default | Description |
|---|---|---|
| `PORT` | `8003` | Internal port the Kyomi server listens on. You should not need to change this -- use `KYOMI_PORT` in your `.env` to control the host-side port. |
| `KYOMI_PORT` | `8080` | Host port that Docker maps to the Kyomi container. This is what you access in your browser. |
| `SELF_HOSTED` | `false` | Set to `true` to enable self-hosted mode. The provided compose files set this automatically. |
| `KYOMI_EDITION` | `community` | Edition identifier. Set to `community` or `enterprise`. The provided compose files set this automatically. |
| `FRONTEND_URL` | -- | The external URL users access Kyomi at (e.g., `https://kyomi.example.com`). Used for generating links in emails, Slack messages, and OAuth redirects. |
| `BASE_URL` | -- | Same as `FRONTEND_URL`. Both should be set to the same value. |
| `WEBAUTHN_RP_ID` | `localhost` | The hostname used for passkey (WebAuthn) registration. Must match the hostname users access in their browser. For `https://kyomi.example.com`, set this to `kyomi.example.com`. |
| `ENABLE_SCHEDULERS` | `true` | Whether to run background schedulers (watch execution, maintenance tasks). Disable if running multiple replicas and only one should run schedulers. |

## LLM

| Variable | Default | Description |
|---|---|---|
| `LLM_PROVIDER` | -- | **Required.** See above. |
| `LLM_API_KEY` | -- | **Required.** See above. |
| `LLM_MODEL` | Provider default | Override the default model. For example: `claude-sonnet-4-20250514`, `gpt-4o`, `gemini-2.5-pro`. |
| `LLM_BASE_URL` | Provider default | Custom API base URL. Use this for OpenAI-compatible providers. Example: `http://host.docker.internal:11434/v1` for Ollama running on the Docker host. |
| `ANTHROPIC_API_KEY` | -- | Legacy variable. If set and `LLM_API_KEY` is not, this is used for the Anthropic provider. Prefer `LLM_API_KEY` for new installations. |

## Authentication

| Variable | Default | Description |
|---|---|---|
| `PASSWORD_AUTH_ENABLED` | `true` | Enable password-based authentication (email + password). |
| `PASSKEYS_ENABLED` | `true` | Enable passkey (WebAuthn) authentication. Requires HTTPS in production (passkeys do not work over plain HTTP, except on `localhost`). |
| `GOOGLE_OAUTH_CLIENT_ID` | -- | Google OAuth 2.0 client ID. Enables "Sign in with Google" and is also required for connecting BigQuery datasources. Create credentials at [Google Cloud Console](https://console.cloud.google.com/apis/credentials). |
| `GOOGLE_OAUTH_CLIENT_SECRET` | -- | Google OAuth 2.0 client secret. |

## Email (SMTP)

Configure SMTP to enable email notifications from Watches and other features.

| Variable | Default | Description |
|---|---|---|
| `SMTP_HOST` | -- | SMTP server hostname (e.g., `smtp.gmail.com`, `smtp.sendgrid.net`). |
| `SMTP_PORT` | `587` | SMTP server port. Use `587` for STARTTLS or `465` for implicit TLS. |
| `SMTP_USER` | -- | SMTP authentication username. |
| `SMTP_PASSWORD` | -- | SMTP authentication password or app-specific password. |
| `SMTP_FROM_EMAIL` | `noreply@kyomi.ai` | The "From" email address for outgoing messages. |
| `SMTP_FROM_NAME` | `Kyomi` | The "From" display name for outgoing messages. |

## Slack Integration (Enterprise Only)

Slack integration allows users to interact with Kyomi from Slack channels and receive Watch alerts as Slack messages.

| Variable | Default | Description |
|---|---|---|
| `SLACK_CLIENT_ID` | -- | Slack app OAuth client ID. |
| `SLACK_CLIENT_SECRET` | -- | Slack app OAuth client secret. |
| `SLACK_SIGNING_SECRET` | -- | Slack app signing secret, used to verify incoming requests from Slack. |

See the [Enterprise Setup guide](enterprise-setup.md) for instructions on creating and configuring a Slack app.

## Push Notifications

Web push notifications allow Watches to send alerts directly to users' browsers.

| Variable | Default | Description |
|---|---|---|
| `VAPID_PRIVATE_KEY` | -- | VAPID private key for web push. Generate a keypair with: `npx web-push generate-vapid-keys` |
| `VAPID_CONTACT` | -- | Contact URL or email for the VAPID key (e.g., `mailto:admin@example.com`). Required by the web push protocol. |

## Redis (Enterprise Only)

Redis is used for multi-replica state synchronization, WebSocket pub/sub, and caching.

| Variable | Default | Description |
|---|---|---|
| `REDIS_URL` | -- | Redis connection URL. The Enterprise compose file sets this to `redis://redis:6379/0` automatically. Format: `redis://[user:password@]host:port/db` |

## Chart Renderer (Enterprise Only)

The chart renderer generates server-side PNG images of charts for use in Slack messages and email alerts.

| Variable | Default | Description |
|---|---|---|
| `CHART_RENDERER_URL` | -- | Internal URL of the chart renderer service. The Enterprise compose file sets this to `http://chart-renderer:3030` automatically. |

## Stripe (Not Used in Self-Hosted)

The `STRIPE_*` variables are used by Kyomi's SaaS offering and are not relevant for self-hosted deployments. You can safely ignore them.

## Example: Minimal .env

```bash
# Database
POSTGRES_PASSWORD=your-strong-password-here

# Security
JWT_SECRET_KEY=your-jwt-secret-here
ENCRYPTION_KEY=your-encryption-key-here

# LLM
LLM_PROVIDER=anthropic
LLM_API_KEY=sk-ant-your-key-here

# URL (change if using a domain)
KYOMI_URL=http://localhost:8080
WEBAUTHN_RP_ID=localhost
```

## Example: Production .env with All Optional Features

```bash
# Database
POSTGRES_PASSWORD=a-very-strong-32-char-password!!

# Security
JWT_SECRET_KEY=base64-encoded-secret
ENCRYPTION_KEY=base64url-encoded-key

# LLM
LLM_PROVIDER=anthropic
LLM_API_KEY=sk-ant-your-key-here

# URLs
KYOMI_URL=https://kyomi.example.com
WEBAUTHN_RP_ID=kyomi.example.com

# Email
SMTP_HOST=smtp.sendgrid.net
SMTP_PORT=587
SMTP_USER=apikey
SMTP_PASSWORD=SG.your-sendgrid-key
SMTP_FROM_EMAIL=kyomi@example.com
SMTP_FROM_NAME=Kyomi

# Push Notifications
VAPID_PRIVATE_KEY=your-vapid-private-key
VAPID_CONTACT=mailto:admin@example.com

# Google OAuth (for Google sign-in and BigQuery)
GOOGLE_OAUTH_CLIENT_ID=123456.apps.googleusercontent.com
GOOGLE_OAUTH_CLIENT_SECRET=GOCSPX-your-secret

# Slack (Enterprise only)
SLACK_CLIENT_ID=123456.789012
SLACK_CLIENT_SECRET=your-slack-secret
SLACK_SIGNING_SECRET=your-signing-secret
```
