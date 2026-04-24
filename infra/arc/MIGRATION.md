# Migrating GitHub Actions Runners: NAS → k3s (ARC)

## Overview

Migrating self-hosted runners from the Synology NAS (192.168.1.100, Celeron J3455
4-core) to the k3s prod machine (192.168.1.250, Ryzen 7 7730U 16-core) using
Actions Runner Controller (ARC) v2.

**Three runner scale sets:**

| Scale Set | Label | Purpose | Image |
|-----------|-------|---------|-------|
| `kyomi-ci` | `kyomi-ci` | Clippy, lint, tests | `ghcr.io/actions/actions-runner:latest` |
| `kyomi-build` | `kyomi-build` | Docker image builds (DinD) | `ghcr.io/actions/actions-runner:latest` + DinD sidecar |
| `kyomi-desktop` | `kyomi-desktop` | Tauri desktop builds (Ubuntu 22.04) | `192.168.1.100:6145/arc-runner-desktop:22.04` |

## Step 1: Create GitHub App (manual — browser required)

1. Go to https://github.com/organizations/kyomi-ai/settings/apps/new
2. Fill in:
   - **Name**: `Kyomi ARC Runners`
   - **Homepage URL**: `https://kyomi.ai`
   - **Webhook → Active**: uncheck (not needed)
3. Permissions:
   - **Organization permissions → Self-hosted runners**: Read & Write
4. **Where can this app be installed?** → Only on this account
5. Click **Create GitHub App**
6. Note the **App ID** (displayed at the top)
7. Scroll down → **Generate a private key** → save the `.pem` file
8. Click **Install App** → install on **kyomi-ai** organization → **All repositories**
9. Note the **Installation ID** from the URL:
   `https://github.com/organizations/kyomi-ai/settings/installations/<INSTALLATION_ID>`

## Step 2: Install Helm on prod machine

```bash
ssh 192.168.1.250
curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | bash
```

## Step 3: Create k8s secret with GitHub App credentials

```bash
ssh 192.168.1.250

export KUBECONFIG=/etc/rancher/k3s/k3s.yaml

# Create namespace
sudo kubectl create namespace arc-runners

# Copy the .pem file to the prod machine first, then:
sudo kubectl create secret generic arc-github-app \
  --namespace arc-runners \
  --from-literal=github_app_id=<APP_ID> \
  --from-literal=github_app_installation_id=<INSTALLATION_ID> \
  --from-file=github_app_private_key=/path/to/kyomi-arc-runners.pem
```

## Step 4: Build and push the desktop runner image

From the dev machine (or any machine with Docker):

```bash
cd infra/arc/images/desktop-runner
docker build -t 192.168.1.100:6145/arc-runner-desktop:22.04 .
docker push 192.168.1.100:6145/arc-runner-desktop:22.04
```

The k3s node pulls from this registry (already configured as an insecure
registry for the kyomi-api images).

## Step 5: Run the setup script

```bash
ssh 192.168.1.250

# Clone or copy the repo, then:
cd /path/to/kyomi/infra/arc
sudo ./setup.sh
```

This installs:
1. ARC controller in `arc-systems` namespace
2. All three runner scale sets in `arc-runners` namespace
3. Creates work directories on `/data/arc-runners/`

## Step 6: Verify runners are registered

```bash
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml

# Check controller
sudo kubectl get pods -n arc-systems

# Check runner listeners (one per scale set)
sudo kubectl get pods -n arc-runners

# Trigger a test CI run by pushing to a PR branch
```

In the GitHub org settings (https://github.com/organizations/kyomi-ai/settings/actions/runners),
you should see three new runner groups with the `kyomi-ci`, `kyomi-build`, and
`kyomi-desktop` labels.

## Step 7: Decommission NAS runners

Once ARC runners are verified working:

```bash
ssh 192.168.1.100

# Stop kyomi runners (keep others for now)
sudo /usr/local/bin/docker stop runner-kyomi runner-kyomi-2 runner-kyomi-private
sudo /usr/local/bin/docker rm runner-kyomi runner-kyomi-2 runner-kyomi-private
```

The other runners (chartml, territoryguru, alytic) can be migrated later by
adding their repos to the GitHub App installation and updating their workflows
to use ARC labels.

## Verify the k3s insecure registry config

The desktop runner image is hosted on the local registry at `192.168.1.100:6145`.
k3s needs this configured as an insecure registry. Check/create:

```bash
# /etc/rancher/k3s/registries.yaml
mirrors:
  "192.168.1.100:6145":
    endpoint:
      - "http://192.168.1.100:6145"
```

Then restart k3s: `sudo systemctl restart k3s`

## Rollback

If ARC runners fail, re-enable the NAS runners:

```bash
ssh 192.168.1.100
# Re-create the containers with the same env vars
# (the PAT and config are preserved in Docker)
sudo /usr/local/bin/docker start runner-kyomi runner-kyomi-2 runner-kyomi-private
```

And revert the workflow `runs-on:` labels back to `self-hosted`.
