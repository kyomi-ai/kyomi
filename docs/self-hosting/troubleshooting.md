# Troubleshooting

## Viewing Logs

Most issues can be diagnosed from the application logs:

```bash
# All services
docker compose logs -f

# Kyomi only
docker compose logs -f kyomi

# Last 100 lines
docker compose logs --tail 100 kyomi
```

## Checking Service Health

```bash
# Container status
docker compose ps

# Health endpoint
curl http://localhost:8080/api/health
```

The health endpoint returns the status of each component (database, LLM provider, optional services). Use this to identify which part of the system is misconfigured.

---

## Common Issues

### Container exits immediately

**Symptom:** `docker compose up -d` starts the container, but it exits within seconds.

**Diagnosis:**

```bash
docker compose logs kyomi
```

**Common causes:**

- Missing required environment variables (`DATABASE_URL`, `JWT_SECRET_KEY`). The install script generates these automatically; if you set up manually, verify all required variables are present in `.env`.
- Database is not reachable. Ensure the `postgres` service is running and healthy before Kyomi starts. The default `docker-compose.yml` includes `depends_on` with a health check, but if you are using an external database, verify connectivity.

---

### Cannot connect to database

**Symptom:** Logs show `error connecting to database` or `connection refused`.

**Diagnosis:**

1. Verify PostgreSQL is running:
   ```bash
   docker compose ps postgres
   ```

2. Verify the password matches between the `postgres` service and the `DATABASE_URL` in `.env`:
   ```bash
   # Check what DATABASE_URL is set to
   grep DATABASE_URL .env

   # Check the postgres password
   grep POSTGRES_PASSWORD .env
   ```

3. If using an external PostgreSQL, verify the host is reachable from inside the Docker network:
   ```bash
   docker compose exec kyomi sh -c "nc -zv your-db-host 5432"
   ```

---

### AI chat says "LLM not configured"

**Symptom:** Conversations return an error about the LLM provider not being configured.

**Solution:** Set `LLM_PROVIDER` and `LLM_API_KEY` in your `.env` file. See [LLM Provider Setup](llm-providers.md) for detailed instructions.

Verify the configuration is loaded:

```bash
curl -s http://localhost:8080/api/health | grep llm
```

If it shows `"not configured"`, restart after updating `.env`:

```bash
docker compose up -d
```

---

### Passkeys do not work

**Symptom:** Passkey registration or authentication fails. The browser shows an error or nothing happens when attempting to use a passkey.

**Cause:** WebAuthn requires the Relying Party ID (RP ID) to match the hostname the browser sees.

**Solution:** Set `WEBAUTHN_RP_ID` to the hostname users access Kyomi at (without the protocol or port):

```env
# If users access Kyomi at https://kyomi.example.com
WEBAUTHN_RP_ID=kyomi.example.com
FRONTEND_URL=https://kyomi.example.com
```

If `WEBAUTHN_RP_ID` is not set, Kyomi infers it from `FRONTEND_URL`. Ensure `FRONTEND_URL` matches the actual URL users type in their browser.

**Important:** Passkeys require HTTPS in production. Browsers will refuse to register passkeys on HTTP origins (except `localhost` for development).

---

### Email not sending

**Symptom:** Users do not receive verification or password reset emails.

**Diagnosis:**

1. Verify all SMTP variables are set:
   ```bash
   grep SMTP .env
   ```
   At minimum, `SMTP_HOST`, `SMTP_USER`, and `SMTP_PASSWORD` must be set.

2. Check logs for SMTP errors:
   ```bash
   docker compose logs kyomi | grep -i smtp
   ```

3. Common SMTP issues:
   - **Port 25 blocked** -- many cloud providers block outbound port 25. Use port 587 (STARTTLS) or 465 (SMTPS) instead.
   - **Authentication failed** -- verify your SMTP username and password. Some providers require an app-specific password.
   - **TLS required** -- ensure `SMTP_PORT=587` (STARTTLS) or `SMTP_PORT=465` (implicit TLS).

---

### Permission denied on .env

**Symptom:** Kyomi cannot read the `.env` file, or you see permission errors in logs.

**Solution:** The `.env` file contains secrets and should have restricted permissions, but must be readable by the user running Docker:

```bash
chmod 600 .env
ls -la .env
```

If you are running Docker as a non-root user, ensure the file is owned by that user:

```bash
chown $(whoami) .env
```

---

### WebSocket connection fails

**Symptom:** The Kyomi UI shows a "disconnected" indicator, or real-time features (streaming AI responses, live notifications) do not work.

**Common causes:**

- **Reverse proxy not forwarding WebSocket upgrades.** If you have a reverse proxy (nginx, Caddy, Traefik) in front of Kyomi, it must be configured to forward WebSocket connections. For nginx:
  ```nginx
  location / {
      proxy_pass http://localhost:8080;
      proxy_http_version 1.1;
      proxy_set_header Upgrade $http_upgrade;
      proxy_set_header Connection "upgrade";
      proxy_set_header Host $host;
  }
  ```

- **`FRONTEND_URL` mismatch.** The WebSocket connection URL is derived from `FRONTEND_URL`. If this does not match the URL in the browser, the connection will fail due to CORS.

---

### Ollama connection refused

**Symptom:** AI chat fails with a connection error when using Ollama as the LLM provider.

**Cause:** The Kyomi container cannot reach the Ollama service on the host machine.

**Solutions:**

- Use `host.docker.internal` (works on Docker Desktop for Mac/Windows and recent Docker Engine on Linux):
  ```env
  LLM_BASE_URL=http://host.docker.internal:11434/v1
  ```

- On Linux, if `host.docker.internal` does not resolve, add it to your Docker Compose:
  ```yaml
  services:
    kyomi:
      extra_hosts:
        - "host.docker.internal:host-gateway"
  ```

- Alternatively, run Ollama in the same Docker network:
  ```yaml
  services:
    ollama:
      image: ollama/ollama
      volumes:
        - ollama_data:/root/.ollama
    kyomi:
      environment:
        LLM_BASE_URL: http://ollama:11434/v1
  ```

---

## Getting Help

If your issue is not covered here:

1. Check the logs: `docker compose logs -f kyomi` -- most problems are visible in the output
2. Check the health endpoint: `curl http://localhost:8080/api/health` -- identifies misconfigured components
3. Search existing issues at [github.com/kyomi-ai/kyomi/issues](https://github.com/kyomi-ai/kyomi/issues)
4. Open a new issue with your logs and health endpoint output
