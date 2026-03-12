# Kyomi Scripts

All scripts are organized by environment. **Every script is environment-specific** - no ambiguity about which database/services they target.

## Directory Structure

```
scripts/
├── dev/          # Development environment scripts
│   └── dangerous/    # Risky dev operations (data loss, resets)
└── prod/         # Production environment scripts
    └── dangerous/    # Risky prod operations (deployments, backups)

## Development Scripts (`scripts/dev/`)

All dev scripts use `.env` and target:
- PostgreSQL: localhost:5433
- Redis: localhost:6380
- Backend: localhost:8002
- Frontend: localhost:5173

### Service Management
- **`start.sh`** - Start all dev services (PostgreSQL, Redis, backend, frontend)
- **`stop.sh`** - Stop all dev services
- **`restart.sh`** - Restart all dev services
- **`start-backend.sh`** - Start just the backend API
- **`start-frontend.sh`** - Start just the frontend dev server
- **`start-services.sh`** - Start just PostgreSQL + Redis containers
- **`stop-services.sh`** - Stop PostgreSQL + Redis containers
- **`restart-backend.sh`** - Restart backend API
- **`restart-frontend.sh`** - Restart frontend dev server

### Database Management
- **`setup-database.sh`** - Initialize database schema
- **`migrate-database.sh`** - Run database migrations

### Utilities
- **`populate-cache.sh`** - Populate BigQuery cache in dev database
- **`build-frontend.sh`** - Build frontend in dev mode

### Dangerous Operations (`scripts/dev/dangerous/`)
⚠️ **These can cause data loss or major changes**
- **`reset-database.sh`** - Wipe and rebuild database (ALL DATA LOST!)
- **`reset-tours.sh`** - Reset UI tours for all users
- **`export-beta-signups.sh`** - Export beta signup emails

## Production Scripts (`scripts/prod/`)

Production runs on a k3s cluster. Deployments are handled by CI/CD (GitHub Actions).
These scripts are for manual operations via `kubectl`.

### Database Management
- **`setup-database.sh`** - Initial database setup via kubectl
- **`populate-cache.sh`** - Populate BigQuery cache in prod database
- **`index-datasets.sh`** - Index BigQuery public datasets in prod
- **`seed-trial-data.sh`** - Seed trial ClickHouse with sample data

### Dangerous Operations (`scripts/prod/dangerous/`)
⚠️ **These affect production - use with caution**
- **`backup.sh`** - Backup production database via kubectl
- **`restore.sh`** - Restore production database from backup

## Quick Start

### Development

```bash
# Start everything
./scripts/dev/start.sh

# Or start services individually
./scripts/dev/start-services.sh  # PostgreSQL + Redis
./scripts/dev/start-backend.sh   # Backend API
./scripts/dev/start-frontend.sh  # Frontend dev server

# Setup database (first time)
./scripts/dev/setup-database.sh

# Stop everything
./scripts/dev/stop.sh
```

### Production

Production is deployed via CI/CD (push to `main`). For manual operations:

```bash
# View pods and logs
kubectl get pods -n kyomi
kubectl logs -f deployment/backend -n kyomi

# Backup database
./scripts/prod/dangerous/backup.sh

# Run migrations manually
kubectl exec -n kyomi deployment/backend -- alembic upgrade head
```

## Key Principles

1. **Every script is environment-specific** - No "neutral" scripts that could target either dev or prod
2. **Dev scripts run on host** - Use .env, connect to localhost:5433 (PostgreSQL), localhost:6380 (Redis)
3. **Prod scripts run in Docker** - Use .env.production, run inside containers
4. **No ambiguity** - If it's in `scripts/dev/`, it targets dev. If it's in `scripts/prod/`, it targets prod.

## Migration Notes

All scripts have been reorganized:
- Old `/scripts/*.sh` → `scripts/dev/` or `scripts/prod/`
- Old `/scripts/db/*.sh` → duplicated into both `scripts/dev/` and `scripts/prod/`
- Old `/apps/backend/scripts/*.sh` → moved to `scripts/dev/` (only `entrypoint.sh` remains for Docker)

Update any automation or documentation that references old paths.
