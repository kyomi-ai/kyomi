---
layout: doc
title: "Configuration - Kyomi Connect"
description: "Configure Kyomi Connect with environment variables, the setup wizard, and health check endpoints."
---

# Configuration

Kyomi Connect can be configured through environment variables, a config file written by the setup wizard, or CLI flags. Environment variables take precedence over the config file.

## Environment Variables

| Variable | Required | Default | Description |
|---|---|---|---|
| `KYOMI_TOKEN` | Yes | -- | JWT token from the Kyomi dashboard. Contains the WebSocket endpoint URL and database type. |
| `DB_HOST` | Yes | -- | Database hostname or IP address |
| `DB_PORT` | No | Auto-detected | Database port. Defaults based on database type: PostgreSQL/Redshift=5432, MySQL=3306, ClickHouse=8123, SQL Server/Synapse=1433 |
| `DB_NAME` | Yes | -- | Database name to connect to |
| `DB_USER` | Yes | -- | Database username |
| `DB_PASSWORD` | Yes | -- | Database password |
| `DB_SSL_MODE` | No | `prefer` | TLS mode for the database connection |
| `DB_SSL_CA` | No | -- | Path to a CA certificate file for TLS verification |
| `HEALTH_PORT` | No | `9090` | Port for the HTTP health check endpoint |
| `RUST_LOG` | No | `info` | Log verbosity. Values: `error`, `warn`, `info`, `debug`, `trace` |

### File-Based Secrets

Every `DB_*` variable supports a `_FILE` suffix that reads the value from a file instead:

| Variable | Reads from |
|---|---|
| `DB_PASSWORD_FILE` | Reads password from the file at this path |
| `DB_HOST_FILE` | Reads hostname from the file at this path |
| `DB_USER_FILE` | Reads username from the file at this path |

This is the recommended approach for Docker secrets and Kubernetes secret volumes:

```bash
docker run -d \
  -e KYOMI_TOKEN_FILE="/run/secrets/kyomi_token" \
  -e DB_PASSWORD_FILE="/run/secrets/db_password" \
  -e DB_HOST="postgres.internal" \
  -e DB_PORT="5432" \
  -e DB_NAME="analytics" \
  -e DB_USER="readonly" \
  -v /path/to/secrets:/run/secrets:ro \
  ghcr.io/kyomi-ai/kyomi-connect:latest
```

### SSL Mode Options

| Mode | Behavior |
|---|---|
| `disable` | No TLS. Use only on trusted local networks. |
| `prefer` | Try TLS first, fall back to unencrypted if the server does not support it. This is the default. |
| `require` | Require TLS. Fail if the server does not support it. Recommended for cloud databases. |
| `verify-ca` | Require TLS and verify the server certificate against the CA specified by `DB_SSL_CA`. |
| `verify-full` | Same as `verify-ca`, plus verify that the server hostname matches the certificate. |

## Token Details

The `KYOMI_TOKEN` is a JWT (JSON Web Token) signed with ES256. You do not need to decode or inspect it -- Connect handles verification automatically. However, the token encodes the following information that Connect uses:

- **Database type** -- Determines which database driver to load and the default port
- **WebSocket URL** -- The Kyomi backend endpoint to connect to
- **Issuer URL** -- Used to fetch the public key for signature verification

::: warning
The token is a secret. Treat it like a password. Do not commit it to version control, log it, or expose it in URLs.
:::

### Token Precedence

Connect resolves the token from multiple sources in this order:

1. `--token` CLI flag (highest priority)
2. `KYOMI_TOKEN` environment variable
3. `KYOMI_TOKEN_FILE` environment variable (reads from file)
4. `--token-file` CLI flag
5. Config file at `~/.config/kyomi-connect/config.toml` or `/etc/kyomi-connect/config.toml`

## Config File

The setup wizard saves configuration to `~/.config/kyomi-connect/config.toml` (user-level) with `600` permissions. The file format:

```toml
token = "eyJhbGciOiJFUzI1NiJ9..."
db_host = "prod-db.internal"
db_port = 5432
db_name = "analytics"
db_user = "readonly_user"
db_password_file = "/home/user/.config/kyomi-connect/.db-password"
db_ssl_mode = "prefer"
```

The database password is stored in a separate file (`.db-password`) in the same directory, also with `600` permissions. The config file references the password file path rather than containing the password directly.

Config file search paths (in order):

1. `~/.config/kyomi-connect/config.toml` (user-level)
2. `/etc/kyomi-connect/config.toml` (system-level)

Environment variables always override config file values when both are present.

## Health Check Endpoint

Connect exposes an HTTP health check at `GET /healthz` on the configured port (default 9090).

### Healthy Response (HTTP 200)

```json
{
  "status": "healthy",
  "ws_connected": true,
  "db_reachable": true
}
```

### Unhealthy Response (HTTP 503)

```json
{
  "status": "unhealthy",
  "ws_connected": false,
  "db_reachable": true
}
```

The endpoint reports two independent health signals:

| Field | Meaning |
|---|---|
| `ws_connected` | Whether the WebSocket connection to Kyomi's backend is established |
| `db_reachable` | Whether the database connection is working |

Use this endpoint for:

- **Kubernetes liveness/readiness probes** (the Helm chart configures these automatically)
- **Docker health checks** via `HEALTHCHECK` directive
- **Load balancer health checks** in AWS ECS or similar
- **Monitoring** with tools like Prometheus, Datadog, or Uptime Kuma

### Example: Docker Healthcheck

```dockerfile
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD curl -f http://localhost:9090/healthz || exit 1
```

## Interactive Setup Wizard

When you run `kyomi-connect setup` (or just `kyomi-connect` without a config file), the setup wizard walks you through configuration step by step.

### What the Wizard Does

**Step 1: Token**
- Opens your browser to the Kyomi Connect setup page for token authorization
- Alternatively, you can paste a token directly from the terminal
- If an existing config file is found, offers to keep the current token or enter a new one

**Step 2: Verify Token**
- Fetches the public key from Kyomi's JWKS endpoint
- Verifies the JWT signature (refuses to continue if invalid)
- Calls the Kyomi API to fetch datasource metadata (name, type, workspace)
- Displays the datasource name and workspace so you can confirm you have the right token

**Step 3: Database Connection**
- Prompts for host, port, database name, username, and password
- Default port is set automatically based on the database type from the token
- If an existing config file is found, current values are shown as defaults
- Prompts for SSL mode with descriptions for each option

**Step 4: Test Connection**
- Tests the database connection with the provided credentials
- If the test fails, shows the error and still saves the config (you can fix it and re-run setup later)

**Step 5: Save Config**
- Writes `config.toml` to `~/.config/kyomi-connect/`
- Saves the password to a separate file with `600` permissions
- Starts the Connect agent automatically

### Non-Interactive Mode

Every wizard prompt maps to a CLI flag, so you can script the entire setup:

```bash
kyomi-connect setup \
  --token "eyJ..." \
  --db-host "prod-db.internal" \
  --db-port 5432 \
  --db-name "analytics" \
  --db-user "readonly_user" \
  --db-password-file /run/secrets/db_password \
  --db-ssl-mode "require"
```

### Re-Running Setup

You can re-run `kyomi-connect setup` at any time to change the configuration. The wizard loads existing values as defaults, so you only need to change what is different.

## CLI Reference

| Command | Description |
|---|---|
| `kyomi-connect` | Auto-detect: runs the agent if configured, or starts the setup wizard if not |
| `kyomi-connect run` | Run the agent directly (fails if not configured) |
| `kyomi-connect setup` | Run the setup wizard (reconfigure token, database credentials) |
| `kyomi-connect status` | Show current connection status, datasource info, and last heartbeat |
| `kyomi-connect service install` | Generate and install a systemd unit file |
| `kyomi-connect service uninstall` | Remove the systemd unit file |
| `kyomi-connect --version` | Print version and build info |

## Logging

Connect writes structured JSON logs to stdout. This is the standard format for containerized workloads and integrates with Docker logs, Kubernetes pod logs, and log aggregation tools.

Example log output:

```json
{"timestamp":"2026-03-01T10:00:00.000Z","level":"INFO","message":"Database connection verified"}
{"timestamp":"2026-03-01T10:00:00.100Z","level":"INFO","message":"WebSocket connected to Kyomi"}
```

Control log verbosity with the `RUST_LOG` environment variable:

| Level | What is logged |
|---|---|
| `error` | Only errors |
| `warn` | Errors and warnings |
| `info` | Connection events, command execution summaries, errors (default) |
| `debug` | Everything above plus SQL statements and detailed request/response data |
| `trace` | Full wire-level detail (very verbose) |

::: warning
The `debug` and `trace` levels may log SQL query text. Do not use these levels in production if your queries contain sensitive data.
:::

Connect never logs database credentials, tokens, or query result data at any log level.
