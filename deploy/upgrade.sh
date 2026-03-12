#!/bin/sh
# =============================================================================
# Kyomi — Upgrade Script
# =============================================================================
# Pulls the latest Kyomi images and restarts services.
#
# Usage:
#   cd kyomi/          # directory containing docker-compose.yml and .env
#   sh upgrade.sh
#
# Your .env configuration and database volumes are preserved.
# =============================================================================
set -e

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf "\033[1;34m==>\033[0m %s\n" "$1"; }
ok()    { printf "\033[1;32m==>\033[0m %s\n" "$1"; }
warn()  { printf "\033[1;33m==>\033[0m %s\n" "$1"; }
error() { printf "\033[1;31m==>\033[0m %s\n" "$1" >&2; }
fatal() { error "$1"; exit 1; }

# ---------------------------------------------------------------------------
# Detect compose command
# ---------------------------------------------------------------------------
if docker compose version >/dev/null 2>&1; then
    COMPOSE_CMD="docker compose"
elif command -v docker-compose >/dev/null 2>&1; then
    COMPOSE_CMD="docker-compose"
else
    fatal "Docker Compose is not installed."
fi

# ---------------------------------------------------------------------------
# Validate we're in the right directory
# ---------------------------------------------------------------------------
if [ ! -f "docker-compose.yml" ]; then
    fatal "docker-compose.yml not found in the current directory. Run this script from your Kyomi install directory."
fi

if [ ! -f ".env" ]; then
    fatal ".env not found in the current directory. Run this script from your Kyomi install directory."
fi

# ---------------------------------------------------------------------------
# Detect edition
# ---------------------------------------------------------------------------
if grep -q "KYOMI_EDITION: enterprise" docker-compose.yml 2>/dev/null; then
    EDITION="enterprise"
else
    EDITION="community"
fi

# ---------------------------------------------------------------------------
# Upgrade
# ---------------------------------------------------------------------------
echo ""
info "Upgrading Kyomi ($EDITION edition)..."
echo ""

info "Pulling latest images..."
$COMPOSE_CMD pull

echo ""
info "Restarting services with new images..."
$COMPOSE_CMD up -d

echo ""
info "Current status:"
$COMPOSE_CMD ps

echo ""
ok "Upgrade complete!"
echo ""
echo "  View logs:  $COMPOSE_CMD logs -f"
echo ""
