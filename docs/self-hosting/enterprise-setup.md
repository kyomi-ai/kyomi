# Enterprise Edition Setup

This guide covers deploying Kyomi Enterprise Edition. Enterprise builds on everything in [Community Edition](community-setup.md) and adds:

- **Redis** -- multi-replica state synchronization, WebSocket pub/sub, and caching
- **Slack Integration** -- users interact with Kyomi from Slack channels and receive Watch alerts as Slack messages
- **Multi-replica Support** -- run multiple Kyomi instances behind a load balancer
- **Commercial License** -- for organizations that cannot comply with AGPL-3.0

Contact [sales@kyomi.ai](mailto:sales@kyomi.ai) for Enterprise licensing.

## Compose File Overview

The Enterprise compose file (`docker-compose.enterprise.yml`) defines three services:

```
postgres (pgvector/pgvector:pg16)
  |- healthcheck: pg_isready
  |- volume: postgres_data

redis (redis:7-alpine)
  |- healthcheck: redis-cli ping
  |- volume: redis_data
  |- maxmemory: 256mb, allkeys-lru

kyomi (ghcr.io/kyomi-ai/kyomi:latest)
  |- depends_on: postgres (healthy), redis (healthy)
  |- port: ${KYOMI_PORT:-8080} -> 8003
  |- env_file: .env
  |- volume: kyomi_data
  |- KYOMI_EDITION: enterprise
  |- REDIS_URL: redis://redis:6379/0
```

## Step-by-Step Setup

### 1. Download the deployment files

```bash
mkdir kyomi && cd kyomi
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/docker-compose.enterprise.yml -o docker-compose.yml
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/.env.enterprise.example -o .env
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/upgrade.sh -o upgrade.sh
chmod +x upgrade.sh
```

### 2. Generate secrets and configure .env

Follow the same secret generation steps as the [Community setup](community-setup.md#3-generate-secrets-and-configure-env).

The Enterprise `.env` has the same required variables as Community, plus optional sections for Slack integration. See the [Configuration Reference](configuration.md) for all variables.

### 3. Start Kyomi

```bash
docker compose up -d
```

Verify all three services are running:

```bash
docker compose ps
```

You should see `postgres`, `redis`, and `kyomi` all in a healthy/running state.

## Redis Configuration

Redis is used by Enterprise Edition for:

- **WebSocket pub/sub** -- routes real-time events across multiple Kyomi replicas
- **Session state** -- shares ephemeral state (rate limits, active connections) across replicas
- **Caching** -- reduces redundant LLM calls and database queries

The compose file configures Redis with sensible defaults:

```yaml
command: >
  redis-server
  --appendonly yes
  --maxmemory 256mb
  --maxmemory-policy allkeys-lru
```

- `appendonly yes` -- enables AOF persistence so data survives Redis restarts
- `maxmemory 256mb` -- caps memory usage (adjust based on your available RAM)
- `allkeys-lru` -- evicts least-recently-used keys when memory limit is reached

### Customizing Redis

To increase the memory limit or add a password, edit the `redis` service in `docker-compose.yml`:

```yaml
redis:
  image: redis:7-alpine
  command: >
    redis-server
    --appendonly yes
    --maxmemory 512mb
    --maxmemory-policy allkeys-lru
    --requirepass your-redis-password
```

If you add a password, update the `REDIS_URL` in the kyomi service:

```yaml
REDIS_URL: redis://:your-redis-password@redis:6379/0
```

### Using an external Redis

If you have an existing Redis instance, remove the `redis` service from the compose file and set `REDIS_URL` in your `.env`:

```bash
REDIS_URL=redis://:password@your-redis-host:6379/0
```

## Slack Integration

Setting up Slack integration allows your users to:
- Ask Kyomi data questions from any Slack channel
- Receive Watch alert notifications in Slack channels
- Share charts and dashboard snapshots in Slack

### 1. Create a Slack App

1. Go to [https://api.slack.com/apps](https://api.slack.com/apps) and click **Create New App**
2. Choose **From scratch**
3. Name it (e.g., "Kyomi") and select your Slack workspace

### 2. Configure OAuth & Permissions

1. In the Slack app settings, go to **OAuth & Permissions**
2. Under **Redirect URLs**, add:
   ```
   https://kyomi.example.com/api/v1/slack/oauth/callback
   ```
   Replace `kyomi.example.com` with your actual Kyomi URL.

3. Under **Bot Token Scopes**, add these scopes:
   - `chat:write` -- send messages
   - `commands` -- handle slash commands
   - `app_mentions:read` -- respond when mentioned
   - `channels:read` -- list channels for watch alert configuration
   - `files:write` -- upload chart images
   - `users:read` -- resolve user identities

### 3. Configure Event Subscriptions

1. Go to **Event Subscriptions** and enable events
2. Set the **Request URL** to:
   ```
   https://kyomi.example.com/api/v1/slack/events
   ```
3. Under **Subscribe to bot events**, add:
   - `app_mention`

### 4. Configure Slash Commands (optional)

1. Go to **Slash Commands** and create a new command:
   - Command: `/kyomi`
   - Request URL: `https://kyomi.example.com/api/v1/slack/commands`
   - Description: "Ask Kyomi a data question"

### 5. Get your credentials

1. Go to **Basic Information** in your Slack app settings
2. Copy the following values:
   - **Client ID** (under App Credentials)
   - **Client Secret** (under App Credentials)
   - **Signing Secret** (under App Credentials)

### 6. Set environment variables

Add these to your `.env`:

```bash
SLACK_CLIENT_ID=your-client-id
SLACK_CLIENT_SECRET=your-client-secret
SLACK_SIGNING_SECRET=your-signing-secret
```

Then restart Kyomi:

```bash
docker compose restart kyomi
```

### 7. Install the app to your workspace

Users can connect Slack from **Settings > Integrations** in the Kyomi web UI. This triggers the OAuth flow to install the Slack app to their workspace.

## Multi-Replica Deployment

Enterprise Edition supports running multiple Kyomi replicas behind a load balancer for high availability.

### Prerequisites

- Redis must be configured (all replicas share state through Redis)
- A load balancer that supports WebSocket connections (sticky sessions are NOT required -- Redis pub/sub handles cross-replica routing)

### Configuration

Scale the kyomi service in your compose file:

```yaml
kyomi:
  deploy:
    replicas: 2
```

Or scale dynamically:

```bash
docker compose up -d --scale kyomi=2
```

### Scheduler considerations

Background schedulers (Watch execution, maintenance tasks) should only run on one replica to avoid duplicate work. Set `ENABLE_SCHEDULERS=false` on all replicas except one:

```yaml
# In docker-compose.yml, create two service definitions:

kyomi-primary:
  image: ghcr.io/kyomi-ai/kyomi:latest
  environment:
    ENABLE_SCHEDULERS: "true"
    # ... other env vars ...

kyomi-worker:
  image: ghcr.io/kyomi-ai/kyomi:latest
  deploy:
    replicas: 2
  environment:
    ENABLE_SCHEDULERS: "false"
    # ... other env vars ...
```

### Load balancer configuration

Your load balancer must forward WebSocket upgrade requests. Example nginx upstream configuration:

```nginx
upstream kyomi {
    server 127.0.0.1:8080;
    server 127.0.0.1:8081;
}

server {
    listen 443 ssl http2;
    server_name kyomi.example.com;

    ssl_certificate     /etc/ssl/certs/kyomi.example.com.pem;
    ssl_certificate_key /etc/ssl/private/kyomi.example.com.key;

    location / {
        proxy_pass http://kyomi;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket support
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";

        proxy_read_timeout 300s;
        proxy_send_timeout 300s;
    }
}
```

## Upgrading from Community to Enterprise

Upgrading from Community to Enterprise preserves all your data. No migration is needed.

1. Download the Enterprise compose file:
   ```bash
   cd kyomi
   curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/docker-compose.enterprise.yml -o docker-compose.yml
   ```

2. Your existing `.env` continues to work. The Enterprise-specific variable (`REDIS_URL`) is set in the compose file itself.

3. If you want Slack integration, add the Slack variables to your `.env` (see [Slack Integration](#slack-integration) above).

4. Restart with the new compose file:
   ```bash
   docker compose up -d
   ```

The new Redis service will start alongside your existing PostgreSQL and Kyomi containers. Your database volume is preserved.
