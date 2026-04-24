#!/usr/bin/env bash
# =============================================================================
# ARC (Actions Runner Controller) v2 — Setup Script
# =============================================================================
# Installs or upgrades ARC controller + all runner scale sets on k3s.
#
# Prerequisites:
#   1. k3s running on the target node (192.168.1.250)
#   2. Helm 3 installed
#   3. GitHub App created and secret configured (see below)
#
# GitHub App Setup (one-time):
#   1. Create a GitHub App at:
#      https://github.com/organizations/kyomi-ai/settings/apps/new
#   2. App name: "Kyomi ARC Runners" (or similar)
#   3. Permissions:
#      - Organization → Self-hosted runners: Read & Write
#   4. Install the app on the kyomi-ai organization
#   5. Generate a private key and download it
#   6. Note the App ID and Installation ID
#   7. Create the k8s secret:
#      kubectl create namespace arc-runners
#      kubectl create secret generic arc-github-app \
#        --namespace arc-runners \
#        --from-literal=github_app_id=<APP_ID> \
#        --from-literal=github_app_installation_id=<INSTALLATION_ID> \
#        --from-file=github_app_private_key=<PATH_TO_PEM>
#
# Usage:
#   ./setup.sh [--controller-only | --runners-only | --desktop-image]
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KUBECONFIG="${KUBECONFIG:-/etc/rancher/k3s/k3s.yaml}"
export KUBECONFIG

CONTROLLER_CHART="oci://ghcr.io/actions/actions-runner-controller-charts/gha-runner-scale-set-controller"
RUNNER_CHART="oci://ghcr.io/actions/actions-runner-controller-charts/gha-runner-scale-set"
CONTROLLER_NS="arc-systems"
RUNNER_NS="arc-runners"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
info()  { echo "==> $*"; }
error() { echo "ERROR: $*" >&2; exit 1; }

check_prereqs() {
    command -v helm >/dev/null 2>&1 || error "helm not found. Install: https://helm.sh/docs/intro/install/"
    command -v kubectl >/dev/null 2>&1 || error "kubectl not found"
    kubectl get nodes >/dev/null 2>&1 || error "Cannot connect to k8s cluster. Check KUBECONFIG=$KUBECONFIG"
}

# ---------------------------------------------------------------------------
# Controller
# ---------------------------------------------------------------------------
install_controller() {
    info "Installing/upgrading ARC controller in namespace $CONTROLLER_NS"
    helm upgrade --install arc \
        --namespace "$CONTROLLER_NS" --create-namespace \
        "$CONTROLLER_CHART" \
        -f "$SCRIPT_DIR/controller-values.yaml" \
        --wait --timeout 120s
    info "Controller ready"
}

# ---------------------------------------------------------------------------
# Runner scale sets
# ---------------------------------------------------------------------------
install_runners() {
    # Verify the GitHub App secret exists
    if ! kubectl get secret arc-github-app -n "$RUNNER_NS" >/dev/null 2>&1; then
        error "Secret 'arc-github-app' not found in namespace '$RUNNER_NS'. Create the GitHub App secret first (see script header)."
    fi

    local github_secret_ref="arc-github-app"

    for values_file in "$SCRIPT_DIR"/runners/*.yaml; do
        local name
        name="$(basename "$values_file" .yaml)"
        info "Installing/upgrading runner scale set: $name"
        helm upgrade --install "$name" \
            --namespace "$RUNNER_NS" --create-namespace \
            "$RUNNER_CHART" \
            -f "$values_file" \
            --set "githubConfigSecret=$github_secret_ref" \
            --wait --timeout 120s
        info "$name ready"
    done
}

# ---------------------------------------------------------------------------
# Desktop runner image
# ---------------------------------------------------------------------------
build_desktop_image() {
    info "Building desktop runner image (Ubuntu 22.04 + Tauri deps)"
    local image="192.168.1.100:6145/arc-runner-desktop:22.04"
    docker build -t "$image" "$SCRIPT_DIR/images/desktop-runner/"
    docker push "$image"
    info "Pushed $image"
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
main() {
    check_prereqs

    case "${1:-all}" in
        --controller-only)
            install_controller
            ;;
        --runners-only)
            install_runners
            ;;
        --desktop-image)
            build_desktop_image
            ;;
        all)
            install_controller
            install_runners
            info "All components installed. Verify with:"
            info "  kubectl get pods -n $CONTROLLER_NS"
            info "  kubectl get pods -n $RUNNER_NS"
            ;;
        *)
            echo "Usage: $0 [--controller-only | --runners-only | --desktop-image]"
            exit 1
            ;;
    esac
}

main "$@"
