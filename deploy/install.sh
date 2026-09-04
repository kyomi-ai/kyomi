#!/bin/sh
# =============================================================================
# Kyomi — Install Script
# =============================================================================
# Downloads and configures Kyomi for self-hosted deployment via Docker Compose.
#
# Usage:
#   curl -fsSL https://get.kyomi.ai | sh
#   — or —
#   sh install.sh
#
# Pin to a specific version:
#   KYOMI_VERSION=2.0.0 curl -fsSL https://get.kyomi.ai | sh
#
# Requirements: Docker with Compose plugin, openssl, curl
#
# THIS FILE IS THE SINGLE SOURCE OF TRUTH for the installer (KYO-641). The
# marketing site (kyomi-ai/kyomi-private, .github/workflows/deploy-marketing.yml)
# is intended to copy this exact file to apps/marketing/public/install.sh at
# deploy time, serving it at https://kyomi.ai/install.sh. That copy will be
# generated and gitignored there — edit this file only, never the served copy.
# =============================================================================
set -e

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
KYOMI_VERSION="${KYOMI_VERSION:-latest}"
GITHUB_RAW_BASE="https://raw.githubusercontent.com/kyomi-ai/kyomi/main/deploy"
INSTALL_DIR="kyomi"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { printf "\033[1;34m==>\033[0m %s\n" "$1"; }
ok()    { printf "\033[1;32m==>\033[0m %s\n" "$1"; }
warn()  { printf "\033[1;33m==>\033[0m %s\n" "$1"; }
error() { printf "\033[1;31m==>\033[0m %s\n" "$1" >&2; }
fatal() { error "$1"; exit 1; }

# Prompt with a default value. Usage: result=$(prompt "Question" "default")
prompt() {
    printf "\033[1m%s\033[0m" "$1" > /dev/tty
    if [ -n "$2" ]; then
        printf " [%s]" "$2" > /dev/tty
    fi
    printf ": " > /dev/tty
    read -r _answer < /dev/tty
    if [ -z "$_answer" ]; then
        echo "$2"
    else
        echo "$_answer"
    fi
}

# Generate a random alphanumeric string of given length using openssl
random_alphanum() {
    openssl rand -base64 "$1" | tr -dc 'a-zA-Z0-9' | head -c "$1"
}

# ---------------------------------------------------------------------------
# Prerequisite checks
# ---------------------------------------------------------------------------
check_prerequisites() {
    info "Checking prerequisites..."

    # prompt() reads and writes /dev/tty directly (so prompts survive
    # `curl | sh`, where stdin is the script itself). Without a controlling
    # terminal — e.g. `curl ... | sh < /dev/null`, a Docker build step, or
    # headless CI — the first `read -r ... < /dev/tty` would otherwise abort
    # with a raw "/dev/tty: No such device or address" from the shell instead
    # of one of this script's own error messages. Check both directions here,
    # before any prompt runs. The probe runs in a child `sh -c` rather than
    # redirecting /dev/tty directly on this shell: under `set -e`, a failed
    # `< /dev/tty` / `> /dev/tty` redirection aborts the whole script
    # immediately with that same raw error — even as the condition of an
    # `if` — which is the exact failure this guard exists to prevent.
    # Confining the failing redirection to a child process's own exit status
    # keeps it a normal, catchable command result instead.
    if ! sh -c ': < /dev/tty' 2>/dev/null || ! sh -c ': > /dev/tty' 2>/dev/null; then
        fatal "This installer requires an interactive terminal (no /dev/tty available). Run it directly in a terminal — it cannot run non-interactively (piped stdin, CI, or a container build step)."
    fi

    if ! command -v docker >/dev/null 2>&1; then
        fatal "Docker is not installed. Please install Docker first: https://docs.docker.com/get-docker/"
    fi

    # Check for Docker Compose (plugin or standalone)
    if docker compose version >/dev/null 2>&1; then
        COMPOSE_CMD="docker compose"
    elif command -v docker-compose >/dev/null 2>&1; then
        COMPOSE_CMD="docker-compose"
    else
        fatal "Docker Compose is not installed. Please install it: https://docs.docker.com/compose/install/"
    fi

    if ! command -v openssl >/dev/null 2>&1; then
        fatal "openssl is not installed. It is required to generate security keys."
    fi

    if ! command -v curl >/dev/null 2>&1; then
        fatal "curl is not installed. It is required to download Kyomi files."
    fi

    # Verify Docker daemon is running
    if ! docker info >/dev/null 2>&1; then
        fatal "Docker daemon is not running. Please start Docker and try again."
    fi

    ok "All prerequisites met (Docker, Compose, openssl, curl)"
}

# ---------------------------------------------------------------------------
# Detect existing installation
# ---------------------------------------------------------------------------
detect_existing() {
    if [ -f "$INSTALL_DIR/.env" ] && [ -f "$INSTALL_DIR/docker-compose.yml" ]; then
        return 0
    fi
    return 1
}

handle_existing_install() {
    warn "Existing Kyomi installation detected in ./$INSTALL_DIR/"
    echo ""
    echo "  Your .env file and data will be preserved."
    echo "  This will re-download the compose file and restart services."
    echo ""
    _update=$(prompt "Continue with update? (y/n)" "y")
    case "$_update" in
        y|Y|yes|Yes|YES) return 0 ;;
        *) echo "Aborted."; exit 0 ;;
    esac
}

# ---------------------------------------------------------------------------
# Download files
# ---------------------------------------------------------------------------
download_files() {
    info "Downloading Kyomi compose file..."

    mkdir -p "$INSTALL_DIR"

    curl -fsSL "${GITHUB_RAW_BASE}/docker-compose.community.yml" -o "${INSTALL_DIR}/docker-compose.yml"
    curl -fsSL "${GITHUB_RAW_BASE}/.env.example" -o "${INSTALL_DIR}/.env.example"
    curl -fsSL "${GITHUB_RAW_BASE}/upgrade.sh" -o "${INSTALL_DIR}/upgrade.sh"
    chmod +x "${INSTALL_DIR}/upgrade.sh"

    # Pin the image tag if a specific version was requested
    if [ "$KYOMI_VERSION" != "latest" ]; then
        sed -i.bak "s|ghcr.io/kyomi-ai/kyomi:latest|ghcr.io/kyomi-ai/kyomi:${KYOMI_VERSION}|g" \
            "${INSTALL_DIR}/docker-compose.yml"
        rm -f "${INSTALL_DIR}/docker-compose.yml.bak"
        ok "Downloaded compose file (pinned to v${KYOMI_VERSION})"
    else
        ok "Downloaded compose file (using latest)"
    fi
}

# ---------------------------------------------------------------------------
# Generate secrets
# ---------------------------------------------------------------------------
generate_secrets() {
    info "Generating security keys..."
    POSTGRES_PASSWORD=$(random_alphanum 32)
    JWT_SECRET_KEY=$(openssl rand -base64 32)
    # ENCRYPTION_KEY must be base64url (the backend decodes with URL_SAFE base64)
    ENCRYPTION_KEY=$(openssl rand -base64 32 | tr '+/' '-_' | tr -d '=')
    ok "Security keys generated"
}

# ---------------------------------------------------------------------------
# LLM provider configuration
# ---------------------------------------------------------------------------
configure_llm() {
    LLM_PROVIDER=""
    LLM_API_KEY=""

    echo ""
    echo "============================================="
    echo "  AI Configuration (optional)"
    echo "============================================="
    echo ""
    echo "  Kyomi uses an LLM for AI features (chat, alerts, etc.)."
    echo "  You can configure this now or later in Settings → AI."
    echo ""
    _setup=$(prompt "Configure AI now? (y/n)" "n")
    case "$_setup" in
        y|Y|yes|Yes|YES) ;;
        *) ok "Skipping AI setup — configure later in Settings → AI"; return ;;
    esac

    echo ""
    echo "  [1] Anthropic (Claude)"
    echo "  [2] OpenAI (GPT)"
    echo "  [3] Google Gemini"
    echo ""
    _provider=$(prompt "Select provider" "1")
    case "$_provider" in
        1) LLM_PROVIDER="anthropic" ;;
        2) LLM_PROVIDER="openai" ;;
        3) LLM_PROVIDER="gemini" ;;
        *) fatal "Invalid selection. Please choose 1-3." ;;
    esac

    echo ""
    LLM_API_KEY=$(prompt "Enter your API key" "")
    if [ -z "$LLM_API_KEY" ]; then
        warn "No API key entered — AI features will be disabled until configured in Settings → AI"
        LLM_PROVIDER=""
        return
    fi

    ok "LLM provider: $LLM_PROVIDER"
}

# ---------------------------------------------------------------------------
# URL / domain configuration
# ---------------------------------------------------------------------------
configure_url() {
    echo ""
    echo "============================================="
    echo "  Access URL"
    echo "============================================="
    echo ""
    echo "  What URL will users use to access Kyomi?"
    echo "  Examples: http://localhost:8080, https://kyomi.example.com"
    echo ""
    KYOMI_URL=$(prompt "URL" "http://localhost:8080")

    # Extract hostname from URL for WebAuthn RP ID
    # Strip protocol prefix, then strip port and path
    WEBAUTHN_RP_ID=$(echo "$KYOMI_URL" | sed -e 's|^https\?://||' -e 's|[:/].*||')

    ok "Access URL: $KYOMI_URL (RP ID: $WEBAUTHN_RP_ID)"
}

# ---------------------------------------------------------------------------
# Write .env file
# ---------------------------------------------------------------------------
write_env() {
    info "Writing configuration to $INSTALL_DIR/.env..."

    _envfile="${INSTALL_DIR}/.env"

    # Use printf for each line to avoid shell interpretation of special chars
    # in generated secrets or user-provided API keys.
    cat > "$_envfile" <<'ENVHEADER'
# =============================================================================
# Kyomi — Generated by install.sh
# =============================================================================
# Edit this file to change settings, then restart with:
#   docker compose restart
# For full configuration reference, see .env.example
# =============================================================================
ENVHEADER

    printf '\n# --- Database ---\n' >> "$_envfile"
    printf 'POSTGRES_PASSWORD=%s\n' "$POSTGRES_PASSWORD" >> "$_envfile"

    printf '\n# --- Security Keys ---\n' >> "$_envfile"
    printf 'JWT_SECRET_KEY=%s\n' "$JWT_SECRET_KEY" >> "$_envfile"
    printf 'ENCRYPTION_KEY=%s\n' "$ENCRYPTION_KEY" >> "$_envfile"

    if [ -n "$LLM_PROVIDER" ]; then
        printf '\n# --- LLM Provider ---\n' >> "$_envfile"
        printf 'LLM_PROVIDER=%s\n' "$LLM_PROVIDER" >> "$_envfile"
        printf 'LLM_API_KEY=%s\n' "$LLM_API_KEY" >> "$_envfile"
    else
        printf '\n# --- LLM Provider (configure in Settings → AI, or uncomment below) ---\n' >> "$_envfile"
        printf '# LLM_PROVIDER=anthropic\n' >> "$_envfile"
        printf '# LLM_API_KEY=your-api-key\n' >> "$_envfile"
    fi

    printf '\n# --- Application URL ---\n' >> "$_envfile"
    printf 'KYOMI_URL=%s\n' "$KYOMI_URL" >> "$_envfile"
    printf 'WEBAUTHN_RP_ID=%s\n' "$WEBAUTHN_RP_ID" >> "$_envfile"

    cat >> "$_envfile" <<'ENVFOOTER'

# --- Optional settings (uncomment to enable) ---
# See .env.example for all available options including:
#   SMTP, Web Push, Google OAuth
ENVFOOTER

    # Restrict permissions — file contains secrets
    chmod 600 "$_envfile"

    ok "Configuration written"
}

# ---------------------------------------------------------------------------
# Start services
# ---------------------------------------------------------------------------
start_services() {
    info "Starting Kyomi..."
    (cd "$INSTALL_DIR" && $COMPOSE_CMD up -d)
    ok "Kyomi is starting up"
}

# ---------------------------------------------------------------------------
# Success message
# ---------------------------------------------------------------------------
print_success() {
    echo ""
    echo "============================================="
    echo ""
    if [ "$KYOMI_VERSION" != "latest" ]; then
        ok "Kyomi v${KYOMI_VERSION} is installed!"
    else
        ok "Kyomi is installed!"
    fi
    echo ""
    echo "  Open ${KYOMI_URL} in your browser to get started."
    echo "  (It may take a minute for the database to initialize.)"
    echo ""
    echo "  Useful commands:"
    echo "    cd ${INSTALL_DIR}"
    echo "    $COMPOSE_CMD logs -f          # view logs"
    echo "    $COMPOSE_CMD ps               # check status"
    echo "    $COMPOSE_CMD down             # stop Kyomi"
    echo "    $COMPOSE_CMD up -d            # start Kyomi"
    echo "    sh upgrade.sh                 # upgrade to latest"
    echo ""
    echo "  Configuration:  ${INSTALL_DIR}/.env"
    echo "  Compose file:   ${INSTALL_DIR}/docker-compose.yml"
    echo ""
    echo "  Documentation:  https://docs.kyomi.ai"
    echo ""
    echo "============================================="
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    echo ""
    echo "  _  __                    _ "
    echo " | |/ /  _  _  ___  _ __ (_)"
    echo " | ' /  | || |/ _ \\| '  \\| |"
    echo " |_|\\_\\  \\_, |\\___/|_|_|_|_|"
    echo "         |__/"
    echo ""
    echo "  The Data Intelligence Platform"
    echo ""

    check_prerequisites

    if detect_existing; then
        handle_existing_install

        # download_files() below unconditionally fetches the Community compose
        # file. Without this check, re-running install.sh against an existing
        # Enterprise install would silently overwrite docker-compose.yml and
        # downgrade it to Community — dropping redis, its depends_on entries,
        # REDIS_URL and the Slack env vars — right before start_services()
        # brings it up. Refuse instead; this installer only manages Community
        # installs (KYO-641). Day-2 upgrades of an existing install (Community
        # or Enterprise) go through upgrade.sh, which detects the edition and
        # never rewrites docker-compose.yml.
        if grep -q "KYOMI_EDITION: enterprise" "$INSTALL_DIR/docker-compose.yml" 2>/dev/null; then
            fatal "Existing installation is Enterprise edition. This installer only manages Community installs, and continuing would downgrade your deployment. To upgrade an existing install, run: ./$INSTALL_DIR/upgrade.sh"
        fi

        download_files
        start_services
        print_success
    else
        download_files
        generate_secrets
        configure_llm
        configure_url
        write_env
        start_services
        print_success
    fi
}

main
