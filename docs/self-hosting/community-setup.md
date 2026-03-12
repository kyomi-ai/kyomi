# Community Edition Setup

This guide covers deploying Kyomi Community Edition in a production environment.

Community Edition runs two containers:
- **PostgreSQL** (with pgvector) -- stores all application data, user accounts, and vector embeddings
- **Kyomi** -- the application server with an embedded frontend (no separate web server needed)

## Compose File Overview

The Community compose file (`docker-compose.community.yml`) defines:

```
postgres (pgvector/pgvector:pg16)
  |- healthcheck: pg_isready
  |- volume: postgres_data

kyomi (ghcr.io/kyomi-ai/kyomi:latest)
  |- depends_on: postgres (healthy)
  |- port: ${KYOMI_PORT:-8080} -> 8003
  |- env_file: .env
  |- volume: kyomi_data
```

The Kyomi container waits for PostgreSQL to be healthy before starting. Database migrations run automatically on startup.

## Step-by-Step Setup

### 1. Prepare the server

Ensure Docker and Docker Compose are installed:

```bash
# Install Docker (if not already installed)
curl -fsSL https://get.docker.com | sh

# Verify Docker Compose plugin is available
docker compose version
```

### 2. Download the deployment files

```bash
mkdir kyomi && cd kyomi
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/docker-compose.community.yml -o docker-compose.yml
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/.env.example -o .env
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy/upgrade.sh -o upgrade.sh
chmod +x upgrade.sh
```

### 3. Generate secrets and configure .env

Generate the required security keys:

```bash
# Generate a strong database password
echo "POSTGRES_PASSWORD=$(openssl rand -base64 32 | tr -dc 'a-zA-Z0-9' | head -c 32)"

# Generate the JWT signing key
echo "JWT_SECRET_KEY=$(openssl rand -base64 32)"

# Generate the encryption key (must be base64url)
echo "ENCRYPTION_KEY=$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=')"
```

Edit `.env` and fill in the generated values along with your LLM provider configuration. See the [Configuration Reference](configuration.md) for all available variables.

At minimum, you need:

```bash
POSTGRES_PASSWORD=<generated>
JWT_SECRET_KEY=<generated>
ENCRYPTION_KEY=<generated>
LLM_PROVIDER=anthropic    # or openai, gemini
LLM_API_KEY=<your-api-key>
KYOMI_URL=http://localhost:8080
WEBAUTHN_RP_ID=localhost
```

### 4. Start Kyomi

```bash
docker compose up -d
```

Check that everything started:

```bash
docker compose ps
docker compose logs -f kyomi
```

You should see log output indicating the database migrations are running and the server is listening. Open `http://localhost:8080` in your browser.

### 5. Create your account

The first user to register becomes the workspace owner. Navigate to the login page and create an account using email/password or a passkey.

## Connecting Datasources

Kyomi supports connecting to your data warehouse directly from the web UI. Go to **Settings > Datasources** and click **Add Datasource**.

### Supported datasource types

| Datasource | Connection Method | Notes |
|---|---|---|
| BigQuery | Google OAuth | Requires `GOOGLE_OAUTH_CLIENT_ID` and `GOOGLE_OAUTH_CLIENT_SECRET` in `.env` |
| PostgreSQL | Direct connection | Kyomi connects directly to your PostgreSQL server |
| MySQL | Direct connection | |
| ClickHouse | Direct connection | HTTP protocol |
| SQL Server | Direct connection | |
| Redshift | Direct connection | Uses PostgreSQL wire protocol |
| Snowflake | Direct connection | |
| Databricks | Direct connection | Uses SQL warehouse HTTP endpoint |
| Azure Synapse | Direct connection | |

### Connecting to databases on the Docker host

If your database runs on the same machine as Kyomi (not in Docker), use the special Docker hostname:

- **Linux**: Use `host.docker.internal` (requires Docker 20.10+), or add `extra_hosts: ["host.docker.internal:host-gateway"]` to the kyomi service in `docker-compose.yml`.
- **macOS**: `host.docker.internal` works out of the box.

### Connecting to databases in private networks

For databases behind a firewall or in a private cloud (AWS VPC, GCP VPC, etc.), use [Kyomi Connect](https://github.com/kyomi-ai/kyomi-connect). Kyomi Connect is a lightweight agent that runs inside your network and creates a secure tunnel to your Kyomi instance.

## Reverse Proxy Setup

For production deployments, you should put Kyomi behind a reverse proxy to handle TLS termination, custom domains, and other concerns.

### nginx example

```nginx
server {
    listen 443 ssl http2;
    server_name kyomi.example.com;

    ssl_certificate     /etc/ssl/certs/kyomi.example.com.pem;
    ssl_certificate_key /etc/ssl/private/kyomi.example.com.key;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket support (required for real-time features)
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";

        # Increase timeouts for long-running AI requests
        proxy_read_timeout 300s;
        proxy_send_timeout 300s;
    }
}

# Redirect HTTP to HTTPS
server {
    listen 80;
    server_name kyomi.example.com;
    return 301 https://$host$request_uri;
}
```

After setting up the reverse proxy, update your `.env`:

```bash
KYOMI_URL=https://kyomi.example.com
WEBAUTHN_RP_ID=kyomi.example.com
```

Then restart Kyomi:

```bash
docker compose restart kyomi
```

### Important notes for reverse proxies

- **WebSocket support is required.** Kyomi uses WebSockets for real-time streaming of AI responses. The `Upgrade` and `Connection` headers must be forwarded.
- **Increase read timeouts.** AI responses can take 30-60 seconds. Set your proxy timeout to at least 300 seconds.
- **Passkeys require HTTPS.** WebAuthn (passkeys) will not work over plain HTTP, except on `localhost`. If you disable HTTPS, set `PASSKEYS_ENABLED=false` and use password authentication.

## Persistent Storage and Volumes

The Community compose file creates two Docker volumes:

| Volume | Contents | Purpose |
|---|---|---|
| `postgres_data` | PostgreSQL data directory | All application data: users, workspaces, conversations, dashboards, watches, knowledge |
| `kyomi_data` | Kyomi application data | Embedded model files, local state |

### Backup

Back up the PostgreSQL database regularly:

```bash
# Create a backup
docker compose exec postgres pg_dump -U kyomi kyomi > "backup-$(date +%Y%m%d).sql"

# Restore from a backup
docker compose exec -T postgres psql -U kyomi kyomi < backup-20260308.sql
```

### Moving data to a different server

1. Back up the database on the old server
2. Copy the backup file and your `.env` to the new server
3. Set up Kyomi on the new server (steps 1-4 above)
4. Stop Kyomi: `docker compose down`
5. Restore the backup: `docker compose up -d postgres && sleep 5 && docker compose exec -T postgres psql -U kyomi kyomi < backup.sql`
6. Start Kyomi: `docker compose up -d`

## Monitoring

Kyomi exposes health check endpoints:

| Endpoint | Description |
|---|---|
| `GET /health` | Basic health check. Returns 200 when the server is running. |
| `GET /api/health` | Detailed health check with component status (database, optional services). |

Use these with your monitoring tools (Uptime Kuma, Prometheus blackbox exporter, etc.).

## Troubleshooting

### Kyomi container exits immediately

Check the logs:

```bash
docker compose logs kyomi
```

Common causes:
- Missing required environment variables (`POSTGRES_PASSWORD`, `JWT_SECRET_KEY`, `ENCRYPTION_KEY`, `LLM_PROVIDER`, `LLM_API_KEY`)
- PostgreSQL not ready yet (wait a few seconds and try `docker compose up -d` again)
- Port conflict on 8080 (change `KYOMI_PORT` in `.env`)

### "Connection refused" when adding a datasource

If your database is on the Docker host, you need to use `host.docker.internal` as the hostname, not `localhost`. See "Connecting to databases on the Docker host" above.

### AI features return errors

Check that your LLM API key is valid and has sufficient credits. View the Kyomi logs for detailed error messages:

```bash
docker compose logs kyomi | grep -i "llm\|anthropic\|openai\|error"
```

### Passkeys not working

Passkeys (WebAuthn) require HTTPS, except on `localhost`. Ensure:
1. You have a valid TLS certificate on your reverse proxy
2. `WEBAUTHN_RP_ID` matches the hostname users see in their browser
3. Users are accessing via HTTPS, not HTTP
