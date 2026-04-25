# Self-Hosting Kyomi

## Run Kyomi in 30 seconds

```bash
curl -fsSL https://get.kyomi.ai | sh
```

The installer walks you through edition selection, LLM provider setup, and starts Kyomi automatically. Open `http://localhost:8080` when it finishes.

## What You Get

Kyomi is an AI-powered data intelligence platform that connects to your data warehouse. Ask questions in natural language, build dashboards, and set up automated monitoring -- all powered by the LLM provider of your choice (bring your own API key).

- **AI Chat** -- natural language access to your data warehouse
- **Dashboards** -- build and share interactive data visualizations
- **Watches** -- scheduled AI agents that monitor your data and alert you
- **SQL Editor** -- write and run queries directly against your datasources
- **MCP Server** -- connect Kyomi to Claude Desktop, Cursor, and other MCP clients
- **Kyomi Connect** -- lightweight agent that runs inside your network to securely connect private databases
- **Knowledge System** -- Kyomi learns from every conversation, building institutional knowledge about your data

## Quick Start with Docker Compose

If you prefer to set things up manually instead of using the install script:

```bash
# 1. Download the compose file and env template
mkdir kyomi && cd kyomi
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/docker-compose.community.yml -o docker-compose.yml
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/.env.example -o .env

# 2. Edit .env — fill in the required values (see comments in the file)
#    At minimum: POSTGRES_PASSWORD, JWT_SECRET_KEY, ENCRYPTION_KEY, LLM_PROVIDER, LLM_API_KEY

# 3. Start Kyomi
docker compose up -d
```

Open `http://localhost:8080` in your browser. The first user to register becomes the workspace owner.

## Prerequisites

| Requirement | Minimum |
|---|---|
| Docker | 20.10+ with Compose plugin |
| RAM | 2 GB |
| Architecture | x86_64 (amd64) |
| OS | Linux or macOS |
| Network | Outbound HTTPS to your LLM provider |

ARM64 (Apple Silicon, Raspberry Pi) is not yet supported for the pre-built images.

## Editions

Kyomi is available in two editions. Both are deployed the same way (Docker Compose), and you can upgrade from Community to Enterprise at any time without losing data.

| Feature | Community (Free) | Enterprise |
|---|---|---|
| AI Chat (BYOK) | Yes | Yes |
| Dashboards | Yes | Yes |
| Watches (scheduled monitoring) | Yes | Yes |
| SQL Editor | Yes | Yes |
| MCP Server | Yes | Yes |
| Kyomi Connect | Yes | Yes |
| Web Push Notifications | Yes | Yes |
| Knowledge System | Yes | Yes |
| Email Alerts | With SMTP | With SMTP |
| Server-side Chart Rendering | Yes | Yes |
| Slack Integration | -- | Yes |
| Multi-replica Support | -- | Yes (Redis) |
| Commercial License | -- | Yes |

**Community Edition** is licensed under AGPL-3.0. It runs Kyomi and PostgreSQL -- two containers, no other dependencies.

**Enterprise Edition** adds Redis (for multi-replica state and real-time features) and Slack integration. Contact [sales@kyomi.ai](mailto:sales@kyomi.ai) for licensing.

## Supported LLM Providers

Kyomi requires an LLM API key to power AI features. You bring your own key -- Kyomi never routes your data through our servers.

| Provider | `LLM_PROVIDER` value | Notes |
|---|---|---|
| Anthropic (Claude) | `anthropic` | Recommended. Best results with Claude Sonnet 4 or Opus 4. |
| OpenAI (GPT) | `openai` | GPT-4o and later models supported. |
| Google Gemini | `gemini` | Gemini 2.5 Pro and later supported. |
| OpenAI-compatible APIs | `openai` | Set `LLM_BASE_URL` to your endpoint. Works with Ollama, vLLM, LiteLLM, etc. |

## Upgrading

Pull the latest images and restart:

```bash
cd kyomi
sh upgrade.sh
```

Or manually:

```bash
cd kyomi
docker compose pull
docker compose up -d
```

Your `.env` configuration and database volumes are preserved across upgrades. Database migrations run automatically on startup.

## Backing Up

Your data lives in Docker volumes. To back up the database:

```bash
# Dump the database to a file
docker compose exec postgres pg_dump -U kyomi kyomi > backup.sql

# Restore from a backup
docker compose exec -T postgres psql -U kyomi kyomi < backup.sql
```

## Further Reading

- [Configuration Reference](configuration.md) -- all environment variables
- [Community Edition Setup](community-setup.md) -- production deployment guide
- [Enterprise Edition Setup](enterprise-setup.md) -- Enterprise features and deployment
