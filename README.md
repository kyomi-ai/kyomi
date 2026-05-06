# Kyomi — The Data Intelligence Platform

Kyomi brings intelligence to your data. It's an AI-powered platform that learns your data warehouse, answers questions in natural language, monitors your metrics proactively, and integrates into the tools you already use.

## What Kyomi Does

- **Accumulated Knowledge** — Kyomi learns continuously from every conversation. It remembers which tables matter, what fields mean, how metrics are calculated, and the quirks of your data. This institutional knowledge compounds over time and serves your whole organization.

- **Proactive Monitoring (Watches)** — AI agents that scan your data on a schedule, detect anomalies, and alert you when something needs attention. Kyomi watches your business while you sleep.

- **Always-On Data Assistant** — Natural language access to your data warehouse. Ask any data question, any time, without needing to know SQL or which tables to query.

- **Workflow Integration** — Kyomi meets you where you work — Slack, MCP-compatible AI assistants, and more. Data intelligence embedded in your daily workflow.

## Supported Data Sources

PostgreSQL, MySQL, BigQuery, ClickHouse, SQL Server, Snowflake, Databricks, Synapse, Redshift — with more coming via the [Kyomi Connect](https://github.com/kyomi-ai/kyomi-connect) agent.

## Self-Hosting Quick Start

Kyomi ships as a single Docker image. The fastest way to get started:

```bash
# Create a data directory
mkdir -p kyomi-data

# Generate a config file
docker run --rm ghcr.io/kyomi-ai/kyomi-standalone:latest --init > kyomi-data/config.toml

# Edit config.toml with your LLM API key and database credentials
# At minimum, set [llm].api_key

# Run Kyomi
docker run -d \
  --name kyomi \
  -p 3000:3000 \
  -v ./kyomi-data:/data \
  ghcr.io/kyomi-ai/kyomi-standalone:latest
```

Then open [http://localhost:3000](http://localhost:3000) and create your first account.

For detailed setup including Kubernetes/Helm, external PostgreSQL, Redis, and production configuration, see the [self-hosting guide](https://kyomi.ai/self-hosting).

## Architecture

Kyomi is built with:

- **Backend**: Rust (axum) — API server, agent runtime, authentication, billing
- **Frontend**: React + Vite + Tailwind CSS — responsive web application
- **Database**: PostgreSQL (metadata, auth, knowledge) + pgvector (semantic search)
- **Embeddings**: BGE-small-en-v1.5 via ONNX Runtime (runs locally, no external API needed)
- **LLM Providers**: Anthropic Claude, OpenAI, Google Gemini
- **Data Visualization**: [ChartML](https://github.com/kyomi-ai/chartml) — declarative chart specification

## Development

### Prerequisites

- Rust 1.85+ (2024 edition)
- Node.js 20+
- PostgreSQL 15+ with pgvector extension
- Redis 7+

### Local Setup

```bash
# Start infrastructure (PostgreSQL + Redis)
docker compose -f docker-compose.dev.yml up -d postgres redis

# Start the backend
bash scripts/dev/start-rust-backend.sh

# Start the frontend (in another terminal)
bash scripts/dev/start-frontend.sh
```

The frontend runs at http://localhost:5173 and the API at http://localhost:8002.

## Contributing

We welcome contributions. Please read the [Contributor License Agreement](CLA.md) before submitting a pull request.

1. Fork the repository
2. Create a feature branch
3. Make your changes with tests
4. Submit a pull request

## License

Kyomi is dual-licensed:

- **Open Source**: [GNU Affero General Public License v3.0](LICENSE) (AGPL-3.0-or-later)
- **Commercial**: Available for organizations that cannot comply with AGPL terms. See [LICENSE-COMMERCIAL.md](LICENSE-COMMERCIAL.md) for details.

The `enterprise/` directory is licensed separately under a proprietary license. See [enterprise/LICENSE](apps/backend-rust/enterprise/LICENSE).

Copyright 2025-2026 Alytic Pty Ltd.
