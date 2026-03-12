---
layout: doc
title: "Troubleshooting - Kyomi Connect"
description: "Common Kyomi Connect issues and their solutions: token errors, database connection failures, WebSocket problems, and more."
---

# Troubleshooting

Common issues when setting up or running Kyomi Connect, with solutions.

## Token Issues

### "Token signature verification failed"

Connect could not verify the JWT signature against Kyomi's public key.

**Causes and solutions:**

- **Wrong token** -- You may have copied the token from a different datasource or environment. Go to Settings > Datasources in Kyomi, select the correct datasource, and copy the token again.
- **Corrupted paste** -- The token was truncated or modified during copy/paste. JWT tokens are long strings (typically 200+ characters) with three sections separated by dots. Make sure you copied the entire string.
- **Wrong environment** -- If you have both staging and production Kyomi instances, make sure the token came from the same instance Connect is trying to reach.

### "KYOMI_TOKEN not set"

Connect cannot find a token from any source (environment variable, config file, or CLI flag).

**Solutions:**

- Set the `KYOMI_TOKEN` environment variable
- Run `kyomi-connect setup` to configure interactively
- Ensure the config file exists at `~/.config/kyomi-connect/config.toml`

### "No matching key found in JWKS"

The JWT has a `kid` (key ID) that does not match any key in the JWKS endpoint.

**Cause:** This typically happens during key rotation on Kyomi's side. It is transient.

**Solution:** Wait a few minutes and try again. If the problem persists, generate a new token from the Kyomi dashboard.

## Database Connection Issues

### "Database connection failed" During Setup

Connect could not reach your database with the provided credentials.

**Check these in order:**

1. **Is the database running?** -- Verify with `pg_isready -h <host> -p <port>` (PostgreSQL), `mysqladmin ping -h <host>` (MySQL), or the equivalent for your database.

2. **Can Connect reach the database?** -- From the machine running Connect, test network connectivity:
   ```bash
   nc -zv your-db-host 5432
   ```
   If this fails, the database is not reachable from the Connect host. Check firewalls, security groups, and network routing.

3. **Are the credentials correct?** -- Try connecting directly with a database client:
   ```bash
   psql -h your-db-host -p 5432 -U readonly_user -d analytics
   ```

4. **Docker networking** -- If both Connect and the database run in Docker, use the Docker service name (e.g., `db`) as `DB_HOST`, not `localhost`. If the database runs on the host machine, use `host.docker.internal` (Docker Desktop) or `--network=host` (Linux).

5. **SSL mismatch** -- If your database requires TLS, set `DB_SSL_MODE=require`. If it does not support TLS, set `DB_SSL_MODE=disable`. The default `prefer` tries TLS first and falls back, which works for most setups.

### "Connection refused" on a Specific Port

The database server is not listening on the expected port.

**Check the port defaults:**

| Database | Default Port |
|---|---|
| PostgreSQL | 5432 |
| MySQL | 3306 |
| ClickHouse | 8123 |
| SQL Server | 1433 |
| Redshift | 5432 |
| Azure Synapse | 1433 |

If your database runs on a non-standard port, set `DB_PORT` explicitly.

## WebSocket Connection Issues

### "WebSocket connection refused" or "Connection timed out"

Connect cannot establish the WebSocket connection to Kyomi's backend.

**Check these:**

1. **Outbound HTTPS/WSS allowed?** -- Connect needs outbound access to `api.kyomi.ai` on port 443. Check your firewall rules, security groups, and proxy settings.

2. **Proxy required?** -- If your network routes outbound traffic through a proxy, set the `HTTPS_PROXY` environment variable:
   ```bash
   HTTPS_PROXY=http://proxy.internal:8080
   ```

3. **DNS resolution** -- Verify that the Connect host can resolve `api.kyomi.ai`:
   ```bash
   nslookup api.kyomi.ai
   ```

4. **Token expired or revoked** -- If the token's `jti` has been revoked (via "Rotate Token" or "Disconnect" in the Kyomi UI), the WebSocket handshake will be rejected. Generate a new token.

### Connection Drops and Reconnects

Connect automatically reconnects with exponential backoff (1s, 2s, 4s, 8s, up to 60s) when the WebSocket connection drops. Occasional disconnections are normal and expected -- they can be caused by:

- Network interruptions or flapping
- Load balancer idle timeouts
- Kyomi backend deployments (rolling restarts)

**This is not a problem.** Connect reconnects automatically and the datasource returns to "Connected" status within seconds. No action is required unless the disconnections are frequent and sustained.

If you see persistent reconnection loops:

1. Check network stability between the Connect host and `api.kyomi.ai`
2. Check the Connect logs for specific error messages
3. Verify the token has not been revoked

## Kyomi UI Issues

### "Datasource Offline" in Kyomi

The Kyomi dashboard shows the datasource as disconnected.

**Possible causes:**

1. **Connect is not running** -- Check the Connect process or container:
   ```bash
   # systemd
   sudo systemctl status kyomi-connect

   # Docker
   docker ps | grep kyomi-connect

   # Kubernetes
   kubectl get pods -l app.kubernetes.io/name=kyomi-connect
   ```

2. **Network interruption** -- If Connect lost its WebSocket connection, it will reconnect automatically. Wait 30-60 seconds and check again.

3. **Health check failure** -- Query the health endpoint directly:
   ```bash
   curl http://localhost:9090/healthz
   ```
   The response tells you whether the WebSocket and database connections are healthy.

4. **Token rotated, old Connect still running** -- If you rotated the token in the Kyomi UI but did not update Connect, the old token is no longer valid. Update the token and restart Connect.

### "Connected but Database Unreachable"

The WebSocket to Kyomi is working, but Connect cannot reach the database.

**This means the database credentials or network path changed after Connect started.** Common causes:

- Database server restarted or moved to a different host
- Database password was changed
- Network rule change blocked access
- Database ran out of connections

Restart Connect after fixing the underlying issue:

```bash
# systemd
sudo systemctl restart kyomi-connect

# Docker
docker restart kyomi-connect

# Kubernetes
kubectl rollout restart deployment/kyomi-connect
```

## Logs

### Finding Logs

| Deployment | Where to look |
|---|---|
| Foreground (`kyomi-connect run`) | stdout in your terminal |
| systemd | `journalctl -u kyomi-connect -f` |
| Docker | `docker logs kyomi-connect -f` |
| Kubernetes | `kubectl logs deployment/kyomi-connect -f` |
| AWS ECS | CloudWatch Logs at `/ecs/kyomi-connect` |

### Increasing Log Verbosity

Set the `RUST_LOG` environment variable to `debug` for more detail:

```bash
# Environment variable
RUST_LOG=debug kyomi-connect run

# Docker
docker run -e RUST_LOG=debug ghcr.io/kyomi-ai/kyomi-connect:latest

# Kubernetes (patch the deployment)
kubectl set env deployment/kyomi-connect RUST_LOG=debug
```

::: warning
Debug logging may include SQL query text. Do not leave debug logging enabled in production if your queries contain sensitive data.
:::

## Getting Help

If you have exhausted these troubleshooting steps, contact support with:

1. The output of `kyomi-connect status`
2. The output of `curl http://localhost:9090/healthz`
3. Recent Connect logs (with `RUST_LOG=debug` if possible)
4. Your deployment method (Docker, Kubernetes, binary, etc.)
