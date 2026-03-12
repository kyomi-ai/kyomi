---
layout: doc
title: "Installation - Kyomi Connect"
description: "Install Kyomi Connect using the binary installer, Docker, Docker Compose, Kubernetes Helm, or AWS ECS."
---

# Installation

Kyomi Connect can be deployed five ways. Choose the method that fits your infrastructure.

| Method | Best for |
|---|---|
| [Binary (Linux/macOS)](#binary-linux-macos) | VMs, bare-metal servers, quick evaluation |
| [Cargo (Rust)](#cargo-rust) | Rust developers, building from source |
| [Docker](#docker) | Any environment with Docker |
| [Docker Compose](#docker-compose) | Teams with existing Compose stacks |
| [Kubernetes / Helm](#kubernetes-helm) | EKS, GKE, AKS, self-managed clusters |
| [AWS ECS / Fargate](#aws-ecs-fargate) | AWS-native infrastructure |

## Prerequisites

Before installing, you need a Connect token:

1. In Kyomi, go to **Settings > Datasources > Add Datasource**
2. Select your database type and choose **Kyomi Connect** as the connection method
3. Save the datasource -- Kyomi generates a one-time token
4. Copy the token immediately (it will not be shown again)

## Binary (Linux/macOS)

A single command installs the binary and runs the interactive setup wizard:

```bash
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi-connect/main/scripts/install.sh | bash
```

The install script:

1. Detects your OS and architecture (Linux/macOS, x86_64/ARM64)
2. Downloads the latest binary from [GitHub Releases](https://github.com/kyomi-ai/kyomi-connect/releases)
3. Verifies the SHA256 checksum
4. Installs to `/usr/local/bin/kyomi-connect`

Then run the setup wizard:

```bash
kyomi-connect setup
```

If you already have a token, you can pass it directly to skip the browser-based flow:

```bash
curl -fsSL https://raw.githubusercontent.com/kyomi-ai/kyomi-connect/main/scripts/install.sh | bash -s -- --token "eyJ..."
```

### Running as a systemd Service

After setup, install Connect as a systemd service for automatic restarts:

```bash
sudo kyomi-connect service install
sudo systemctl enable --now kyomi-connect
```

Check the service status:

```bash
sudo systemctl status kyomi-connect
```

To uninstall the service:

```bash
sudo kyomi-connect service uninstall
```

### Running in the Foreground

For testing or debugging, run Connect directly:

```bash
kyomi-connect run
```

## Cargo (Rust)

If you have Rust installed, you can build from source via [crates.io](https://crates.io/crates/kyomi-connect):

```bash
cargo install kyomi-connect
```

This compiles the binary from the [open-source repository](https://github.com/kyomi-ai/kyomi-connect) and installs it to `~/.cargo/bin/kyomi-connect`. Then run the setup wizard:

```bash
kyomi-connect setup
```

## Docker

Run Connect as a Docker container with environment variables for all configuration:

```bash
docker run -d \
  --restart=always \
  --name kyomi-connect \
  -e KYOMI_TOKEN="eyJ..." \
  -e DB_HOST="your-database-host" \
  -e DB_PORT="5432" \
  -e DB_USER="readonly_user" \
  -e DB_PASSWORD="your-password" \
  -e DB_NAME="analytics" \
  ghcr.io/kyomi-ai/kyomi-connect:latest
```

::: tip
If your database is running on the Docker host machine (localhost), use `host.docker.internal` as the `DB_HOST` value on Docker Desktop, or `--network=host` on Linux.
:::

### Environment Variable Reference

| Variable | Required | Default | Description |
|---|---|---|---|
| `KYOMI_TOKEN` | Yes | -- | Connect token from the Kyomi dashboard |
| `DB_HOST` | Yes | -- | Database hostname or IP address |
| `DB_PORT` | No | Varies by type | Database port (5432 for Postgres, 3306 for MySQL, etc.) |
| `DB_NAME` | Yes | -- | Database name |
| `DB_USER` | Yes | -- | Database username |
| `DB_PASSWORD` | Yes | -- | Database password |
| `DB_SSL_MODE` | No | `prefer` | SSL mode: `disable`, `prefer`, `require`, `verify-ca`, `verify-full` |
| `DB_SSL_CA` | No | -- | Path to CA certificate file (for `verify-ca` or `verify-full`) |
| `HEALTH_PORT` | No | `9090` | Port for the `/healthz` HTTP endpoint |
| `RUST_LOG` | No | `info` | Log level (`debug`, `info`, `warn`, `error`) |

All `DB_*` variables support a `_FILE` suffix for Docker secrets integration. For example, `DB_PASSWORD_FILE=/run/secrets/db_password` reads the password from that file at startup.

## Docker Compose

Add Connect as a service alongside your database in an existing `docker-compose.yml`:

```yaml
services:
  kyomi-connect:
    image: ghcr.io/kyomi-ai/kyomi-connect:latest
    restart: always
    environment:
      KYOMI_TOKEN: "${KYOMI_TOKEN}"
      DB_HOST: "db"
      DB_PORT: "5432"
      DB_NAME: "analytics"
      DB_USER: "readonly_user"
      DB_PASSWORD_FILE: "/run/secrets/db_password"
    secrets:
      - db_password
    depends_on:
      - db

  db:
    image: postgres:16
    # ... your database configuration

secrets:
  db_password:
    file: ./secrets/db_password.txt
```

Set `KYOMI_TOKEN` in a `.env` file alongside `docker-compose.yml`:

```
KYOMI_TOKEN=eyJhbGciOiJFUzI1NiJ9.eyJkc2lkIjoiZHMt...
```

Start the stack:

```bash
docker compose up -d
```

## Kubernetes / Helm

The Helm chart creates a Deployment, Secret, and ServiceAccount with health check probes pre-configured.

### Step 1: Create Secrets

```bash
kubectl create secret generic kyomi-connect-token \
  --from-literal=token="eyJ..."

kubectl create secret generic kyomi-connect-db \
  --from-literal=password="your-password"
```

### Step 2: Install the Chart

```bash
helm install kyomi-connect \
  oci://ghcr.io/kyomi-ai/charts/kyomi-connect \
  --set existingSecret.name=kyomi-connect-token \
  --set target.host="postgres-service" \
  --set target.port=5432 \
  --set target.database="analytics" \
  --set target.user="readonly_user" \
  --set target.passwordSecretName=kyomi-connect-db
```

### Helm Values Reference

| Value | Default | Description |
|---|---|---|
| `token` | `""` | Connect token (creates a Secret automatically) |
| `existingSecret.name` | `""` | Use an existing Secret instead of `token` |
| `existingSecret.key` | `"token"` | Key within the existing Secret |
| `target.host` | `""` | Database hostname |
| `target.port` | `5432` | Database port |
| `target.database` | `""` | Database name |
| `target.user` | `""` | Database username |
| `target.passwordSecretName` | `""` | Kubernetes Secret containing the database password |
| `target.passwordSecretKey` | `"password"` | Key within the password Secret |
| `image.repository` | `ghcr.io/kyomi-ai/kyomi-connect` | Container image |
| `image.tag` | `latest` | Image tag |
| `healthPort` | `9090` | Health check port |
| `resources.requests.cpu` | `50m` | CPU request |
| `resources.requests.memory` | `64Mi` | Memory request |
| `resources.limits.cpu` | `500m` | CPU limit |
| `resources.limits.memory` | `256Mi` | Memory limit |
| `serviceAccount.create` | `true` | Create a ServiceAccount |

::: warning
Use `existingSecret.name` in production instead of passing the token directly via `--set token`. The `--set` flag records the token value in Helm's release history.
:::

### Raw Manifests (Without Helm)

If you prefer not to use Helm, apply these manifests directly:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: kyomi-connect
stringData:
  token: "eyJ..."
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: kyomi-connect
spec:
  replicas: 1
  selector:
    matchLabels:
      app: kyomi-connect
  template:
    metadata:
      labels:
        app: kyomi-connect
    spec:
      containers:
        - name: kyomi-connect
          image: ghcr.io/kyomi-ai/kyomi-connect:latest
          args: ["run"]
          env:
            - name: KYOMI_TOKEN
              valueFrom:
                secretKeyRef:
                  name: kyomi-connect
                  key: token
            - name: DB_HOST
              value: "postgres-service"
            - name: DB_PORT
              value: "5432"
            - name: DB_USER
              valueFrom:
                secretKeyRef:
                  name: prod-db-creds
                  key: username
            - name: DB_PASSWORD
              valueFrom:
                secretKeyRef:
                  name: prod-db-creds
                  key: password
            - name: DB_NAME
              value: "analytics"
            - name: HEALTH_PORT
              value: "9090"
            - name: RUST_LOG
              value: "info"
          livenessProbe:
            httpGet:
              path: /healthz
              port: 9090
            initialDelaySeconds: 10
            periodSeconds: 30
          readinessProbe:
            httpGet:
              path: /healthz
              port: 9090
            initialDelaySeconds: 5
            periodSeconds: 10
          resources:
            requests:
              cpu: 50m
              memory: 64Mi
            limits:
              cpu: 500m
              memory: 256Mi
```

## AWS ECS / Fargate

Deploy Connect as a Fargate task. Use AWS Secrets Manager for the token and database password.

### Task Definition

```json
{
  "family": "kyomi-connect",
  "containerDefinitions": [
    {
      "name": "kyomi-connect",
      "image": "ghcr.io/kyomi-ai/kyomi-connect:latest",
      "essential": true,
      "secrets": [
        {
          "name": "KYOMI_TOKEN",
          "valueFrom": "arn:aws:secretsmanager:us-east-1:123456789:secret:kyomi/connect-token"
        },
        {
          "name": "DB_PASSWORD",
          "valueFrom": "arn:aws:secretsmanager:us-east-1:123456789:secret:kyomi/db-password"
        }
      ],
      "environment": [
        { "name": "DB_HOST", "value": "prod-db.us-east-1.rds.amazonaws.com" },
        { "name": "DB_PORT", "value": "5432" },
        { "name": "DB_USER", "value": "readonly_user" },
        { "name": "DB_NAME", "value": "analytics" }
      ],
      "healthCheck": {
        "command": ["CMD-SHELL", "curl -f http://localhost:9090/healthz || exit 1"],
        "interval": 30,
        "timeout": 5,
        "retries": 3
      },
      "logConfiguration": {
        "logDriver": "awslogs",
        "options": {
          "awslogs-group": "/ecs/kyomi-connect",
          "awslogs-region": "us-east-1",
          "awslogs-stream-prefix": "connect"
        }
      }
    }
  ],
  "cpu": "256",
  "memory": "512",
  "networkMode": "awsvpc",
  "requiresCompatibilities": ["FARGATE"]
}
```

::: info
The Fargate task must be placed in a subnet that can reach both your RDS instance and the public internet (for the WebSocket connection to Kyomi). A private subnet with a NAT Gateway is the typical setup.
:::

### Networking Notes

- Connect needs **outbound HTTPS/WSS** access to `api.kyomi.ai` (port 443)
- Connect needs **network access** to your database on the configured port
- No inbound ports are required -- Connect only makes outbound connections
- Place the task in the same VPC and security group as your RDS instance

## Verifying the Installation

Regardless of deployment method, verify Connect is running:

1. **Health check** -- `curl http://localhost:9090/healthz` should return:
   ```json
   {"status":"healthy","ws_connected":true,"db_reachable":true}
   ```

2. **Kyomi UI** -- The datasource status should show "Connected" (green indicator)

3. **Test query** -- Open the SQL editor in Kyomi, select the datasource, and run `SELECT 1`
