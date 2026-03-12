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
# Requirements: Docker with Compose plugin, openssl, curl
# =============================================================================
set -e

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
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
    printf "\033[1m%s\033[0m" "$1"
    if [ -n "$2" ]; then
        printf " [%s]" "$2"
    fi
    printf ": "
    read -r _answer
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
# Edition selection
# ---------------------------------------------------------------------------
choose_edition() {
    echo ""
    echo "============================================="
    echo "  Which edition would you like to install?"
    echo "============================================="
    echo ""
    echo "  [1] Community  (free, AGPL-3.0 license)"
    echo "      Postgres + Kyomi. Everything you need to get started."
    echo ""
    echo "  [2] Enterprise (commercial license required)"
    echo "      Adds Redis, chart renderer, Slack integration."
    echo ""
    _edition=$(prompt "Select edition" "1")
    case "$_edition" in
        1) EDITION="community" ;;
        2) EDITION="enterprise" ;;
        *) fatal "Invalid selection. Please choose 1 or 2." ;;
    esac

    ok "Selected: $EDITION edition"
}

# ---------------------------------------------------------------------------
# Download files
# ---------------------------------------------------------------------------
download_files() {
    info "Downloading Kyomi $EDITION compose file..."

    mkdir -p "$INSTALL_DIR"

    COMPOSE_FILE="docker-compose.${EDITION}.yml"
    if [ "$EDITION" = "community" ]; then
        ENV_EXAMPLE=".env.example"
    else
        ENV_EXAMPLE=".env.enterprise.example"
    fi

    curl -fsSL "${GITHUB_RAW_BASE}/${COMPOSE_FILE}" -o "${INSTALL_DIR}/docker-compose.yml"
    curl -fsSL "${GITHUB_RAW_BASE}/${ENV_EXAMPLE}" -o "${INSTALL_DIR}/.env.example"
    curl -fsSL "${GITHUB_RAW_BASE}/upgrade.sh" -o "${INSTALL_DIR}/upgrade.sh"
    chmod +x "${INSTALL_DIR}/upgrade.sh"

    ok "Downloaded compose file and .env example"
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
    echo ""
    echo "============================================="
    echo "  LLM Provider Configuration"
    echo "============================================="
    echo ""
    echo "  Kyomi uses an LLM to power AI features."
    echo ""
    echo "  [1] Anthropic (Claude)"
    echo "  [2] OpenAI (GPT)"
    echo "  [3] Google Gemini"
    echo "  [4] Other (OpenAI-compatible API)"
    echo ""
    _provider=$(prompt "Select provider" "1")
    case "$_provider" in
        1) LLM_PROVIDER="anthropic" ;;
        2) LLM_PROVIDER="openai" ;;
        3) LLM_PROVIDER="gemini" ;;
        4) LLM_PROVIDER="openai" ;;
        *) fatal "Invalid selection. Please choose 1-4." ;;
    esac

    echo ""
    LLM_API_KEY=$(prompt "Enter your API key" "")
    if [ -z "$LLM_API_KEY" ]; then
        fatal "An API key is required. You can add it later by editing $INSTALL_DIR/.env"
    fi

    LLM_BASE_URL=""
    LLM_MODEL=""
    if [ "$_provider" = "4" ]; then
        echo ""
        LLM_BASE_URL=$(prompt "Enter the API base URL (e.g. http://host.docker.internal:11434/v1)" "")
        if [ -z "$LLM_BASE_URL" ]; then
            fatal "A base URL is required for OpenAI-compatible providers."
        fi
        LLM_MODEL=$(prompt "Enter the model name" "")
        if [ -z "$LLM_MODEL" ]; then
            fatal "A model name is required for OpenAI-compatible providers."
        fi
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

    printf '\n# --- LLM Provider ---\n' >> "$_envfile"
    printf 'LLM_PROVIDER=%s\n' "$LLM_PROVIDER" >> "$_envfile"
    printf 'LLM_API_KEY=%s\n' "$LLM_API_KEY" >> "$_envfile"

    # Append optional LLM fields only when set
    if [ -n "$LLM_BASE_URL" ]; then
        printf 'LLM_BASE_URL=%s\n' "$LLM_BASE_URL" >> "$_envfile"
    fi
    if [ -n "$LLM_MODEL" ]; then
        printf 'LLM_MODEL=%s\n' "$LLM_MODEL" >> "$_envfile"
    fi

    printf '\n# --- Application URL ---\n' >> "$_envfile"
    printf 'KYOMI_URL=%s\n' "$KYOMI_URL" >> "$_envfile"
    printf 'WEBAUTHN_RP_ID=%s\n' "$WEBAUTHN_RP_ID" >> "$_envfile"

    cat >> "$_envfile" <<'ENVFOOTER'

# --- Optional settings (uncomment to enable) ---
# See .env.example for all available options including:
#   SMTP, Web Push, Google OAuth, Slack (Enterprise only)
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
    ok "Kyomi is installed!"
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
    echo "  _  __                   _ "
    echo " | |/ /  _ _  _ ___  _ __ (_)"
    echo " | ' < || | '_ \\ _ \\| '  \\| |"
    echo " |_|\\_\\_, | .__/\\___/|_|_|_|_|"
    echo "      |__/|_|"
    echo ""
    echo "  The Data Intelligence Platform"
    echo ""

    check_prerequisites

    if detect_existing; then
        handle_existing_install

        # For updates: re-download compose file but preserve .env
        # Detect edition from existing compose file
        if grep -q "KYOMI_EDITION: enterprise" "$INSTALL_DIR/docker-compose.yml" 2>/dev/null; then
            EDITION="enterprise"
        else
            EDITION="community"
        fi
        info "Detected $EDITION edition"
        download_files
        start_services
        print_success
    else
        choose_edition
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
