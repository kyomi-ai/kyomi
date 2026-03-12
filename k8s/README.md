# Kyomi Kubernetes Deployment

## Prerequisites

1. k3s installed on the server
2. Registry configured in `/etc/rancher/k3s/registries.yaml`:
   ```yaml
   mirrors:
     "YOUR_REGISTRY:PORT":
       endpoint:
         - "http://YOUR_REGISTRY:PORT"
   ```
3. Longhorn distributed storage installed (see below)
4. `open-iscsi` installed on the server: `sudo apt install open-iscsi`

## Storage: Longhorn

Kyomi uses Longhorn for distributed storage on `/data/longhorn/`. This enables:
- Node failure tolerance (when multiple nodes are added)
- Easy horizontal scaling

### Install Longhorn

```bash
kubectl apply -f https://raw.githubusercontent.com/longhorn/longhorn/v1.7.2/deploy/longhorn.yaml
```

### Configure Longhorn to use /data

After installation, add `/data/longhorn/` as a disk and disable the default disk:

```bash
# Create directory on server
sudo mkdir -p /data/longhorn

# Patch Longhorn node to use /data
kubectl patch nodes.longhorn.io prod -n longhorn-system --type='json' -p='[
  {"op": "add", "path": "/spec/disks/data-disk", "value": {
    "allowScheduling": true,
    "path": "/data/longhorn/",
    "storageReserved": 0
  }},
  {"op": "replace", "path": "/spec/disks/default-disk-xxx/allowScheduling", "value": false}
]'

# Set replica count to 1 for single-node (increase when adding nodes)
kubectl patch setting default-replica-count -n longhorn-system --type='merge' -p '{"value": "1"}'
```

### Adding a Second Node

```bash
# On new node, join cluster
curl -sfL https://get.k3s.io | K3S_URL=https://<master-ip>:6443 K3S_TOKEN=<token> sh -

# Increase replica count for redundancy
kubectl patch setting default-replica-count -n longhorn-system --type='merge' -p '{"value": "2"}'
```

## Deployment

### 1. Create the secret

```bash
# Copy template
cp secret.yaml.template secret.yaml

# Generate base64 encoded values
echo -n 'your-postgres-password' | base64
echo -n 'postgresql://kyomi:password@postgres:5432/kyomi' | base64
# Edit secret.yaml and replace placeholders
nano secret.yaml

# Apply secret
kubectl apply -f secret.yaml
```

### 2. Deploy all resources

```bash
# Using kustomize (built into kubectl)
kubectl apply -k .

# Or apply individually in order
kubectl apply -f namespace.yaml
kubectl apply -f configmap.yaml
kubectl apply -f secret.yaml
kubectl apply -f storage.yaml
kubectl apply -f postgres.yaml
kubectl apply -f redis.yaml
kubectl apply -f trial-clickhouse.yaml
kubectl apply -f analytics-clickhouse.yaml
kubectl apply -f chart-renderer.yaml
kubectl apply -f analytics-collector.yaml
kubectl apply -f kyomi-api.yaml
kubectl apply -f ingress.yaml
```

### 3. Restore database (if migrating)

```bash
# Port forward postgres for restore
kubectl port-forward svc/postgres 5432:5432 -n kyomi

# In another terminal, restore from backup
pg_restore -h localhost -U kyomi -d kyomi /path/to/kyomi-postgres.dump
```

### 4. Run migrations

```bash
# Get pod name
POD=$(kubectl get pods -n kyomi -l app=backend -o jsonpath='{.items[0].metadata.name}')

# Run alembic migrations
kubectl exec -n kyomi $POD -- alembic upgrade head
```

## Updating

When new images are pushed to the registry:

```bash
# Rolling restart to pull new image
kubectl rollout restart deployment/backend -n kyomi
kubectl rollout restart deployment/chart-renderer -n kyomi

# Or update to specific tag
kubectl set image deployment/backend backend=YOUR_REGISTRY:PORT/kyomi-backend:abc123 -n kyomi
```

## Monitoring

```bash
# View all resources
kubectl get all -n kyomi

# View backend logs
kubectl logs -f deployment/backend -n kyomi

# View analytics collector logs
kubectl logs -f deployment/analytics-collector -n kyomi

# Check pod status
kubectl describe pod -l app=backend -n kyomi

# Port forward for local testing
kubectl port-forward svc/backend 8001:8001 -n kyomi
```

## Architecture

```
                    ┌─────────────────┐
                    │   Traefik       │
                    │   (Ingress)     │
                    └────────┬────────┘
                             │
     ┌───────────────┬───────┴───────┬───────────────┐
     │               │               │               │
     ▼               ▼               ▼               ▼
┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
│ Kyomi API│  │ Analytics│  │  Chart   │  │  Trial   │
│  (Rust)  │  │ Collector│  │ Renderer │  │ClickHouse│
└────┬─────┘  └────┬─────┘  └──────────┘  └──────────┘
     │              │
     │    ┌─────────┘
     │    │
     ▼    ▼
┌──────────┐  ┌──────────┐
│PostgreSQL│  │ Analytics│
│(pgvector)│  │ClickHouse│
└────┬─────┘  └──────────┘
     │
     ▼
┌──────────┐
│  Redis   │
└──────────┘
```

## Troubleshooting

### Pod stuck in ImagePullBackOff
```bash
# Check if registry is accessible
curl http://YOUR_REGISTRY:PORT/v2/_catalog

# Check k3s registry config
cat /etc/rancher/k3s/registries.yaml
sudo systemctl restart k3s
```

### Backend can't connect to database
```bash
# Check postgres is healthy
kubectl get pods -n kyomi -l app=postgres
kubectl logs deployment/postgres -n kyomi

# Verify DATABASE_URL in secret
kubectl get secret kyomi-secrets -n kyomi -o yaml
```

### Migrations fail
```bash
# Check backend logs for error details
kubectl logs deployment/backend -n kyomi

# Try running migrations interactively
kubectl exec -it deployment/backend -n kyomi -- alembic upgrade head
```
