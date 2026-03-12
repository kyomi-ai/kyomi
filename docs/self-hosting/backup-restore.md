# Backup and Restore

Kyomi stores all persistent data in PostgreSQL. Backing up the database is sufficient to protect against data loss. This guide covers backup strategies for both Community and Enterprise editions.

## Database Backup

### SQL Dump (Recommended)

Create a logical backup using `pg_dump`:

```bash
docker compose exec postgres pg_dump -U kyomi kyomi > backup-$(date +%Y%m%d).sql
```

This produces a plain SQL file that can be restored to any PostgreSQL instance.

### Compressed Backup

For larger databases, compress the output:

```bash
docker compose exec postgres pg_dump -U kyomi -Fc kyomi > backup-$(date +%Y%m%d).dump
```

The `-Fc` flag produces a custom-format archive that is smaller and supports parallel restore.

## Database Restore

### From SQL Dump

Stop Kyomi first to prevent writes during restore:

```bash
docker compose stop kyomi
```

Drop and recreate the database, then restore:

```bash
docker compose exec postgres dropdb -U kyomi kyomi
docker compose exec postgres createdb -U kyomi kyomi
docker compose exec -T postgres psql -U kyomi kyomi < backup-20260308.sql
```

Start Kyomi again:

```bash
docker compose up -d kyomi
```

Kyomi will run any pending migrations on startup (there should be none if the backup was from the same version).

### From Compressed Dump

```bash
docker compose stop kyomi
docker compose exec postgres dropdb -U kyomi kyomi
docker compose exec postgres createdb -U kyomi kyomi
docker compose exec -T postgres pg_restore -U kyomi -d kyomi < backup-20260308.dump
docker compose up -d kyomi
```

## Volume Backup

If you prefer a filesystem-level backup of the PostgreSQL data directory:

```bash
docker compose stop postgres
docker run --rm \
  -v kyomi_postgres_data:/data \
  -v $(pwd):/backup \
  alpine tar czf /backup/postgres-volume-$(date +%Y%m%d).tar.gz -C / data
docker compose up -d postgres
```

**Note:** The PostgreSQL container must be stopped before taking a volume backup to ensure data consistency.

### Volume Restore

```bash
docker compose down
docker volume rm kyomi_postgres_data
docker volume create kyomi_postgres_data
docker run --rm \
  -v kyomi_postgres_data:/data \
  -v $(pwd):/backup \
  alpine tar xzf /backup/postgres-volume-20260308.tar.gz -C /
docker compose up -d
```

## Automated Backups with Cron

Create a script at `/opt/kyomi/backup.sh`:

```bash
#!/bin/bash
set -euo pipefail

BACKUP_DIR="/opt/kyomi/backups"
RETENTION_DAYS=30

mkdir -p "$BACKUP_DIR"

# Create backup
docker compose -f /opt/kyomi/docker-compose.yml exec -T postgres \
  pg_dump -U kyomi -Fc kyomi > "$BACKUP_DIR/backup-$(date +%Y%m%d-%H%M%S).dump"

# Remove backups older than retention period
find "$BACKUP_DIR" -name "backup-*.dump" -mtime +$RETENTION_DAYS -delete

echo "Backup completed: $(date)"
```

Make it executable and add a cron job:

```bash
chmod +x /opt/kyomi/backup.sh
```

Edit crontab (`crontab -e`) and add a daily backup at 2 AM:

```cron
0 2 * * * /opt/kyomi/backup.sh >> /var/log/kyomi-backup.log 2>&1
```

## Testing Your Backups

Backups are only useful if you can restore from them. Periodically verify your backups by restoring to a separate database:

```bash
docker compose exec postgres createdb -U kyomi kyomi_test
docker compose exec -T postgres pg_restore -U kyomi -d kyomi_test < backup-20260308.dump
docker compose exec postgres dropdb -U kyomi kyomi_test
```

If the restore completes without errors, your backup is valid.

## What Is Backed Up

A database backup includes all Kyomi data:

- User accounts and credentials
- Workspace configurations
- Datasource connection settings (encrypted credentials)
- Dashboards and charts
- AI conversation history
- Learnings and knowledge graph
- Watch configurations and alert history
- Catalog metadata and embeddings

## What Is NOT Backed Up

- **LLM API keys and secrets** -- these are stored in your `.env` file, not the database. Back up `.env` separately.
- **Docker images** -- re-pulled during restore with `docker compose pull`.
- **Logs** -- container logs are ephemeral unless you have configured a log driver.

Keep a copy of your `.env` file in a secure location alongside your database backups.
