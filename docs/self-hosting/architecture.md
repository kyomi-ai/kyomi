# Architecture Overview

## How Kyomi Works

Kyomi is a data intelligence platform that connects to your existing data warehouses and uses AI to help you explore, understand, and monitor your data. All your data stays in your infrastructure -- the only external calls are to your chosen LLM provider for AI features.

## System Diagram

```
                          +-------------------+
                          |   LLM Provider    |
                          | (Anthropic/OpenAI |
                          |  /Gemini/Ollama)  |
                          +---------+---------+
                                    |
                                    | API calls (BYOK)
                                    |
+------------+            +---------+---------+            +------------------+
|            |  HTTP/WS   |                   |  SQL/API   |                  |
|   Browser  +----------->+   Kyomi Server    +----------->+ Your Data        |
|            |<-----------+   (port 8080)     |<-----------+ Warehouses       |
|            |            |                   |            | (Postgres, BQ,   |
+------------+            +---------+---------+            |  ClickHouse ...) |
                                    |                      +------------------+
                                    |
                          +---------+---------+
                          |    PostgreSQL     |
                          |  (application DB) |
                          +-------------------+
```

## Community Edition

The Community edition is designed for simplicity. A single container serves both the frontend and the API.

```
docker-compose.yml:
  - kyomi        (application: frontend + API, port 8080)
  - postgres     (database)
```

**What runs inside the Kyomi container:**

- **Axum HTTP server** -- serves the React frontend as static files and handles all API requests on the same port
- **WebSocket server** -- powers real-time streaming for AI conversations and live notifications
- **Embedded embedding model** -- BGE-small-en-v1.5 (ONNX) generates vector embeddings locally for knowledge retrieval, no external API needed
- **Watch scheduler** -- runs background jobs to monitor your data on a schedule
- **In-memory KV store** -- session management and rate limiting for single-instance deployments

**PostgreSQL stores everything:**

- User accounts and authentication credentials
- Workspace and datasource configurations
- Dashboards, charts, and ChartML specifications
- AI conversation history
- Learnings, knowledge graph, and vector embeddings (via pgvector)
- Watch configurations, schedules, and alert history

## Enterprise Edition

The Enterprise edition adds services for production deployments that need multi-replica scaling and Slack integration.

```
docker-compose.yml:
  - kyomi           (application: frontend + API, port 8080)
  - postgres        (database)
  - redis           (session store, pub/sub, multi-replica coordination)
```

**What Redis adds:**

- **Cross-replica session state** -- when running multiple Kyomi replicas behind a load balancer, Redis ensures sessions work regardless of which replica handles a request
- **Pub/sub for WebSockets** -- broadcasts real-time events across all replicas so every connected browser receives updates
- **Rate limiting** -- distributed rate limit counters that are consistent across replicas

Chart rendering is handled natively by the Kyomi binary via chartml-rs — no separate service is needed.

## Data Flow

### AI Conversation

1. User types a question in the browser
2. Browser sends the message over WebSocket to Kyomi
3. Kyomi retrieves relevant context from the knowledge graph (vector search + relationship expansion in PostgreSQL)
4. Kyomi sends the question + context + available tools to the LLM provider
5. The LLM decides which tools to use (query data, create chart, look up schema, etc.)
6. Kyomi executes the tool calls -- running SQL against your data warehouse, reading catalog metadata, etc.
7. Tool results are sent back to the LLM for interpretation
8. The LLM's response streams back to the browser in real time

### Datasource Connections

Kyomi connects directly to your data warehouses using standard database protocols:

- **PostgreSQL / MySQL / ClickHouse / SQL Server / Redshift** -- direct TCP connections using native drivers
- **BigQuery** -- Google Cloud REST API with OAuth credentials
- **Snowflake / Databricks / Azure Synapse** -- HTTPS API connections

Connection credentials are encrypted and stored in PostgreSQL. Kyomi never copies your data -- it runs queries on demand and streams results back to the user.

### Knowledge System

Every conversation teaches Kyomi something about your data. The knowledge system works as follows:

1. **Catalog indexing** -- Kyomi periodically scans your datasource schemas (tables, columns, types, descriptions) and generates vector embeddings
2. **Learning creation** -- when the AI produces useful insights (metric definitions, business rules, query patterns), they are stored as learnings with embeddings
3. **Retrieval** -- before each AI conversation turn, Kyomi searches six vector indexes in parallel (table names, table descriptions, column names, column descriptions, learnings, metrics) and expands via relationship traversal
4. **Context injection** -- the most relevant context is injected into the LLM prompt, so the AI knows which tables to query and how your metrics are defined

This knowledge compounds over time. The more your team uses Kyomi, the better it understands your data.

## What Leaves Your Infrastructure

Kyomi is designed to keep your data private. Here is exactly what crosses your network boundary:

| Destination | What is sent | Why |
|-------------|-------------|-----|
| Your LLM provider | User questions, tool schemas, retrieved context, query results | AI features require an LLM |
| Nothing else | -- | All other processing happens locally |

- Database queries run inside your network
- Vector embeddings are generated locally (embedded ONNX model)
- No telemetry, analytics, or usage data is sent anywhere
- Docker images are pulled from GitHub Container Registry during install/upgrade only

If you use Ollama or another local LLM, nothing leaves your infrastructure at all.

## Port Reference

| Service | Default Port | Configurable Via |
|---------|-------------|-----------------|
| Kyomi (HTTP + WebSocket) | 8080 | `docker-compose.yml` port mapping |
| PostgreSQL | 5432 | `docker-compose.yml` (internal to Docker network) |
| Redis (Enterprise) | 6379 | `docker-compose.yml` (internal to Docker network) |

Only the Kyomi port (8080) needs to be exposed. PostgreSQL and Redis communicate over the internal Docker network and should not be exposed to the internet.
