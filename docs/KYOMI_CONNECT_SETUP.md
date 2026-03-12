# Kyomi Connect - User Setup Guide

## For Curious Users (One Command)

Once releases are set up, users will be able to install and run Kyomi Connect with a single command:

```bash
curl -fsSL https://connect.kyomi.ai/install.sh | sh
```

Or for macOS with Homebrew (future):
```bash
brew install kyomi-connect
kyomi-connect
```

This single command:
1. ✅ Detects your OS and architecture
2. ✅ Downloads the latest Kyomi Connect binary
3. ✅ Runs an interactive setup wizard
4. ✅ Launches the application

## Current Development Setup

For local development, build from source:

```bash
cd apps/backend-rust
cargo build --release -p kyomi-connect
./target/release/kyomi-connect setup
```

## How It Works

### 1. One-Time Installation
The installer script:
- **Detects your system** - Linux or macOS, x86_64 or ARM
- **Downloads the binary** - From github.com/kyomi-ai/kyomi/releases (or builds from source in dev)
- **Validates it** - Checks integrity and permissions

### 2. Interactive Setup
Walks through three simple steps:

```
Step 1: Kyomi API Connection
  → Kyomi WebSocket URL (auto-detects localhost for dev)
  → One-time Connect token (paste from Kyomi UI)

Step 2: Database Type
  → Choose: PostgreSQL, MySQL, ClickHouse, SQL Server, or Redshift

Step 3: Database Connection
  → Host, port, database name, username, password
  → Optional: health check port
```

### 3. Launches the App
- Shows connection summary
- Starts Kyomi Connect
- Displays health check endpoint
- Outputs logs to console

## User Journey

### Before (Without Kyomi Connect)
```
User: "I want to query my database privately"
→ No option available
→ Must send credentials to cloud
```

### After (With Kyomi Connect)
```
User: "I want to query my database privately"
→ Create Connect datasource in Kyomi
→ Copy token from UI
→ Run: curl -fsSL https://connect.kyomi.ai/install.sh | sh
→ Paste token when prompted
→ Choose database type
→ Enter database details
→ Running! Queries execute locally.
```

**Time to working setup: ~2 minutes**

## Architecture of the Installer

```
User runs: curl -fsSL https://connect.kyomi.ai/install.sh | sh
                    ↓
            connect-install.sh
                    ↓
        ┌─────────────────────────────┐
        │  Detect OS + Architecture   │
        └─────────────────────────────┘
                    ↓
        ┌──────────────────────────────────────┐
        │  Download binary from GitHub Releases │
        └──────────────────────────────────────┘
                    ↓
        ┌─────────────────────────────┐
        │  Install to /usr/local/bin   │
        └─────────────────────────────┘
                    ↓
        ┌─────────────────────────────┐
        │  kyomi-connect setup        │
        │  (interactive wizard)        │
        └─────────────────────────────┘
```

## Deployment Scenarios

### Local Development
```bash
# In Kyomi UI: Create a Connect datasource
# In terminal: Build and run
cd apps/backend-rust && cargo build --release -p kyomi-connect
./target/release/kyomi-connect setup
# → Connects to localhost Kyomi and local database
```

### Company Database (On-Prem)
```bash
# In Kyomi UI: Create a Connect datasource
# On company server: Run the installer
curl -fsSL https://connect.kyomi.ai/install.sh | sh
# → Connects to cloud Kyomi, queries company database locally
# → Credentials never leave the company network
```

### Docker/Kubernetes
```bash
docker run -e KYOMI_TOKEN="<token>" \
           -e DB_HOST="postgres.internal" \
           -e DB_PORT="5432" \
           -e DB_NAME="analytics" \
           -e DB_USER="kyomi" \
           -e DB_PASSWORD="<password>" \
           ghcr.io/kyomi-ai/kyomi-connect:latest
```

## Setup Flow Example

```
╔════════════════════════════════════════════════════╗
║    Kyomi Connect - Installation & Setup             ║
╚════════════════════════════════════════════════════╝

Detecting system...
  Platform: linux
  Architecture: x86_64

Building kyomi-connect...
Compiling kyomi-connect v0.1.0
Finished `release` profile in 2m 13s
Build complete!

────────────────────────────────────────────────────

Step 1: Kyomi API Connection
Kyomi WebSocket URL (default: ws://localhost:8003/connect/v1):
Kyomi Connect Token (from Kyomi datasource setup): ••••••••••••••••••••••

Step 2: Database Type
Select your database:
  1) PostgreSQL
  2) MySQL
  3) ClickHouse
  4) SQL Server
  5) Redshift

Enter choice (1-5): 1

Step 3: Database Connection
Database host (default: localhost):
Database port (default: 5432):
Database name: mydb
Database user: postgres
Database password: ••••••••••

Step 4: Optional Settings
Health check port (default: 9090):

Configuration Summary:
  Kyomi: ws://localhost:8003/connect/v1
  Database: postgres://postgres@localhost:5432/mydb
  Health Port: 9090

Start Kyomi Connect? (y/n): y

Starting Kyomi Connect...
Health check: curl http://localhost:9090/healthz

{"timestamp":"2026-02-27T06:00:00.000000Z","level":"INFO",...}
{"message":"Database connection verified",...}
{"message":"WebSocket connected to Kyomi",...}
```

## Next Steps (Post-v1.4)

1. **Homebrew Formula** (optional)
   - `brew install kyomi-connect`
   - Makes it even easier for macOS users

4. **NPM/Docker Hub Publishing** (optional)
   - `npx kyomi-connect` (npm)
   - Docker Hub for container users

5. **Marketing Site Integration**
   - "Get Started" button on kyomi.ai homepage
   - Links to install command
   - Links to documentation

## Files

- **Install script**: `scripts/connect-install.sh` (served at connect.kyomi.ai/install.sh)
- **Binary source**: `apps/backend-rust/crates/kyomi-connect/`
- **Documentation**: `docs/KYOMI_CONNECT_SETUP.md` (this file)
- **README**: `apps/backend-rust/crates/kyomi-connect/README.md`
- **Helm chart**: `charts/kyomi-connect/`
- **Container image**: `ghcr.io/kyomi-ai/kyomi-connect`
- **CI — build**: `.github/workflows/build-connect.yml`
- **CI — release**: `.github/workflows/release-connect.yml`

## Testing the Installer

```bash
# Test locally
sh scripts/connect-install.sh

# Test via connect.kyomi.ai (after DNS + nginx setup)
curl -fsSL https://connect.kyomi.ai/install.sh | sh
```

## Troubleshooting

### Setup script hangs at password prompt
- Normal behavior — it's reading from terminal securely
- Type your password (it won't be echoed)
- Press Enter
