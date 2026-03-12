# Upgrading Kyomi

Kyomi follows semantic versioning. Patch and minor updates are safe to apply without manual intervention. Major version upgrades may require additional steps, which will be noted in the release notes.

## Standard Upgrade (Docker Compose)

Pull the latest images and restart:

```bash
docker compose pull
docker compose up -d
```

Database migrations run automatically when Kyomi starts. No manual migration step is needed.

## Using the Install Script

If you used the install script for your initial setup, you can use it to upgrade as well:

```bash
curl -fsSL https://get.kyomi.ai | sh
```

The script detects your existing installation, pulls updated images, and restarts services. Your `.env` file and data volumes are preserved.

## Verifying the Upgrade

After restarting, check that all services are healthy:

```bash
docker compose ps
```

All services should show `Up (healthy)` or `Up`.

Check the application logs for any migration or startup errors:

```bash
docker compose logs -f kyomi
```

Look for:
- `migrations applied successfully` -- database is up to date
- `Kyomi Rust backend listening on port 8003` -- the server started correctly

Verify the health endpoint:

```bash
curl http://localhost:8080/api/health
```

## Pinning a Specific Version

By default, `docker compose pull` fetches the `latest` tag. To pin a specific version, edit `docker-compose.yml` and change the image tag:

```yaml
services:
  kyomi:
    image: ghcr.io/kyomi-ai/kyomi:1.4.2
```

Then run:

```bash
docker compose up -d
```

## Rolling Back

If an upgrade causes issues, you can roll back to the previous version:

1. Stop the current containers:

```bash
docker compose down
```

2. Change the image tag in `docker-compose.yml` to the previous version:

```yaml
services:
  kyomi:
    image: ghcr.io/kyomi-ai/kyomi:1.4.1
```

3. Start with the old version:

```bash
docker compose up -d
```

**Important:** Database migrations are forward-only. If a new version applied migrations, rolling back the application to an older version will leave newer migration entries in the database. In most cases this is harmless -- the older binary simply ignores tables and columns it does not know about. If a migration made breaking changes (e.g., dropped a column), the release notes will include specific rollback instructions.

## Major Version Upgrades

Major version upgrades (e.g., 1.x to 2.x) may introduce:

- New required environment variables
- Changes to the Docker Compose file structure
- Data format changes requiring a one-time conversion

When a major version is released, the release notes will include step-by-step upgrade instructions. In most cases, re-running the install script will handle everything:

```bash
curl -fsSL https://get.kyomi.ai | sh
```

## Checking Your Current Version

The Kyomi UI displays the version in **Settings**. You can also check programmatically:

```bash
curl -s http://localhost:8080/api/health | jq .version
```
