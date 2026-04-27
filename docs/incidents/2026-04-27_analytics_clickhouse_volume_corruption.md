# Incident: Analytics ClickHouse Volume Corruption

**Date:** 2026-04-27
**Severity:** Data loss (analytics only)
**Duration:** ~2 hours investigation + recovery
**Data lost:** ~3,428 analytics events (Feb–Apr 2026)

## Symptoms

Charts sourcing from the analytics ClickHouse instance failed with:

```
Code: 76. DB::Exception: Cannot open file /var/lib/clickhouse/store/9b5/...all_3119_3119_1/data.cmrk4:
errno: 5, strerror: Input/output error
```

The error surfaced when ClickHouse's background merge tried to compact across a damaged region — the corruption had been silently present for months.

## Investigation Timeline

### 1. Initial error analysis

The error was on the `site_c034dde57a4b3b46` database, specifically a materialized view inner table (`.inner_id.f7195be9-d80a-4a12-85ed-3bd3a65c4e27`). Attempted to `DROP PART` but that also failed with I/O errors — the corruption wasn't limited to one part.

### 2. Checked ClickHouse logs

```
kubectl logs -n kyomi analytics-clickhouse-... | grep -i error
```

Found ongoing I/O errors during background merges:

```
filesystem error: in create_directories: Input/output error
["/var/lib/clickhouse/store/9b5/.../tmp_merge_all_1_3119_1047"]
```

### 3. Checked Longhorn volume health

```
kubectl -n longhorn-system get volumes.longhorn.io
```

- Analytics volume (`pvc-7115f254`) reported **healthy** with **1 replica**
- Two other volumes (minecraft, territoryguru) reported **degraded** — these had `replicas: 3` on a single-node cluster, so 2 replicas could never schedule. This was a misconfiguration, not corruption.

### 4. Checked physical disk (NVMe)

```
smartctl -a /dev/nvme0n1
```

NVMe is healthy: 0 media errors, 0 data integrity errors, 100% available spare, 3% worn. **Not a hardware issue.**

However, SMART reported:
- **7 unsafe shutdowns** in 455 power-on hours
- **20 power cycles**

These occurred during initial server setup (BIOS config, OS install, k8s bootstrapping) — not during normal operation.

### 5. Found kernel-level EXT4 errors

```
sudo dmesg | grep -iE 'i/o error|ext4.*error'
```

Multiple Longhorn volume filesystems had EXT4 journal aborts:

```
EXT4-fs error (device sdc): ext4_journal_check_start:84: Detected aborted journal
EXT4-fs (sdc): error count since last fsck: 2
EXT4-fs (sde): error count since last fsck: 3
EXT4-fs (sdb): error count since last fsck: 1
```

**Initial (wrong) theory:** errors were from unsafe shutdowns during server setup in Jan/Feb. Epoch timestamps seemed to match, but converting them correctly revealed the errors occurred **Sat Apr 25, 22:26–23:22 AEST** — during the deploy we were running to fix the server_fn hash mismatch.

### 5b. Corrected timeline — CPU starvation from CI build runner

Deeper investigation of `journalctl` and `dmesg -T` revealed the true sequence:

1. **22:20** — Docker network churn (deploy-related container restarts)
2. **22:24** — All iSCSI connections start timing out: `ISCSI_ERR_NOP_TIMEDOUT: A NOP has timed out` on connections 1–15
3. **22:42:37** — `systemd-udevd` watchdog timeout (had consumed 2m15s CPU, killed with SIGABRT)
4. **22:42:40** — All Longhorn iSCSI connections report `conn error (1022)` simultaneously
5. **22:42:41** — I/O errors cascade across `sda`, `sdb`, `sdc`, `sde` — in-flight writes lost
6. **22:42:42** — `systemd-journald` watchdog timeout (killed with SIGABRT, journal file corrupted)
7. **22:42:42** — EXT4 journal aborts on `sdb`, `sdc`, `sde`

**Root cause:** The ARC build runner pod (`kyomi-build`) had `cpu: "14"` limits on a 16-core machine. During `cargo build --release` with cold sccache (triggered by the server_fn hash fix deploy), it pegged 14 cores for ~15 minutes. This left only 2 cores for the entire system — k8s, Longhorn engines, iSCSI daemons, systemd services, and all production workloads. The iSCSI NOP keepalives timed out because the iscsid couldn't get CPU time, causing all Longhorn volumes to lose their connections. In-flight writes were dropped, corrupting the EXT4 journals.

The `dind` sidecar (Docker-in-Docker) had NO resource limits at all, potentially consuming even more.

**Note:** The kernel errors on the local dev machine (`nuc`) showing `ata3` / `sda` UNC errors are a SEPARATE issue — that's the dev machine's own disk, not the prod node.

### 6. Assessed data recoverability

Tried reading individual partitions of the events table:

```sql
SELECT count() FROM site_c034dde57a4b3b46.events WHERE toYYYYMM(timestamp) = 202602
-- I/O error
SELECT count() FROM site_c034dde57a4b3b46.events WHERE toYYYYMM(timestamp) = 202604
-- I/O error
```

All 3 partitions (Feb, Mar, Apr 2026) were unreadable. The materialized view metadata files (`_sessions.sql`, `_visitors.sql`) were also corrupted. Even `ls` on the metadata directory returned I/O errors.

### 7. Attempted fsck

Scaled down ClickHouse, attempted to fsck the Longhorn block device (`/dev/sde`):

```
sudo e2fsck -y /dev/sde
# e2fsck: Input/output error while trying to open /dev/sde

sudo e2fsck -b 32768 -y /dev/sde
# Same error — alternate superblock also unreadable
```

The Longhorn engine stops when no pod mounts the volume, making the block device unreadable. Even `dd if=/dev/sde bs=512 count=1` returned I/O errors with the engine stopped. **fsck is not possible on Longhorn volumes without the engine running, and the engine only runs when the volume is mounted — chicken-and-egg with a corrupted filesystem.**

### 8. Recovery: reprovisioned the volume

```bash
# Delete corrupted PVC
kubectl delete pvc plausible-clickhouse-data-longhorn -n kyomi

# Recreate with same name
kubectl apply -f - <<EOF
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: plausible-clickhouse-data-longhorn
  namespace: kyomi
spec:
  accessModes: [ReadWriteOnce]
  storageClassName: longhorn
  resources:
    requests:
      storage: 50Gi
EOF

# Scale ClickHouse back up
kubectl scale deployment analytics-clickhouse -n kyomi --replicas=1
```

ClickHouse started clean. The `site_c034dde57a4b3b46` database will be auto-provisioned by `kyomi-auth::analytics_clickhouse` on the next analytics event.

## Root Cause

**CPU starvation from the CI build runner crashed Longhorn's iSCSI connections, causing in-flight write loss and EXT4 journal corruption on multiple volumes.**

The ARC runner pod (`kyomi-build`) was configured with `cpu: "14"` limits on a 16-core node. A `cargo build --release` with cold sccache pegged all 14 cores for ~15 minutes, leaving only 2 cores for the entire system. Longhorn's iSCSI NOP keepalives timed out, all volume connections dropped simultaneously, and in-flight writes were lost. The `dind` sidecar had no CPU limits at all.

## What Was NOT the Cause

- **NVMe hardware failure** — SMART is clean, 0 media errors, 0 data integrity errors
- **Unsafe shutdowns during setup** — initial theory was wrong; the epoch timestamps in EXT4 errors converted to Apr 25 2026, not Jan/Feb
- **Longhorn bug** — Longhorn correctly reported volumes as "healthy" because replicas were running; it has no mechanism to detect latent EXT4 corruption within a volume

## Remediation Applied

1. **Reduced ARC runner CPU limits** from 14 → 8 cores (runner) and added 2-core limit to the dind sidecar. Combined maximum is 10 cores, leaving 6+ cores for system services during builds. `maxRunners` reduced from 2 → 1 to prevent concurrent builds from saturating the node.
   - File: `infra/arc/runners/kyomi-build.yaml`
   - Applied via: `helm upgrade kyomi-build ...`
2. **Reprovisioned the corrupted analytics ClickHouse volume** (fresh PVC, empty database)
3. **Fixed Longhorn replica counts** — set all volumes to `replicas: 1` (single-node cluster)

## Unresolved Concerns

### 1. fsck is impossible on Longhorn volumes

When a pod releases a Longhorn volume, the engine stops and the block device becomes unreadable. This means standard filesystem repair tools (`e2fsck`) cannot be run on Longhorn volumes outside of a running pod. Longhorn's "maintenance mode" attach via VolumeAttachment resources was unreliable — the volume bounced between attaching/detaching states and the engine never started without a pod consumer.

**This needs investigation.** If another volume gets corrupted, we need a reliable way to run fsck. Options to explore:
- Longhorn UI maintenance mode (may work better than kubectl)
- Creating a dummy pod that mounts the raw block device (requires `volumeMode: Block` on the PVC, which our PVCs don't use)
- Running fsck from within a pod that mounts the volume normally (risky — ext4 may refuse to mount a corrupted filesystem)

### 2. Other volumes may have latent corruption

`sdb` and `sdc` also had EXT4 journal errors from the same event. These are:
- `sdb` (10G) → `pvc-516d4a3d` (unknown workload)
- `sdc` (400G) → `pvc-fa8c5a02` (unknown workload)

These should be proactively checked. If they're also analytics/non-critical, consider reprovisioning them preemptively. If they're Postgres or other stateful workloads, we need a monitoring strategy.

### 3. No monitoring for volume-level corruption

Longhorn has no built-in detection for latent filesystem corruption within volumes. The volume shows "healthy" because the replica process is running, even if the filesystem is damaged. Consider:
- Periodic `e2fsck -n` (read-only check) from within pods
- ClickHouse `CHECK TABLE` queries on a schedule
- Monitoring ClickHouse error logs for I/O error patterns

### 4. Single replica = no redundancy

All volumes on this single-node cluster run with 1 replica. There's no data redundancy — any corruption is permanent. This is accepted for the current setup but should be documented as a known risk. If the cluster ever gains a second node, all critical volumes should be bumped to 2+ replicas.

## Longhorn Replica Count Fixes (same session)

During investigation, found two volumes misconfigured with `replicas: 3` on a single-node cluster:
- `pvc-27406f0f` (minecraft) — set to 1, now healthy
- `pvc-7d0bc94b` (territoryguru shapefiles) — set to 1, now healthy
- `pvc-5fb0ee8f` (new analytics volume) — set to 1 (was provisioned with default 3)

Longhorn default-replica-count was already set to 1; the old PVCs predated that setting.

## PVC-to-Workload Mapping (for future reference)

| Device | Size | PVC | Workload | Status |
|--------|------|-----|----------|--------|
| sda | 20G | pvc-4ad97ced | Postgres (kyomi) | healthy |
| sdb | 10G | pvc-516d4a3d | **unknown — has EXT4 errors** | healthy (latent?) |
| sdc | 400G | pvc-fa8c5a02 | **unknown — has EXT4 errors** | healthy (latent?) |
| sdd | 10G | pvc-27406f0f | minecraft | healthy (was degraded, fixed) |
| sde | 50G | pvc-7115f254 | analytics ClickHouse (OLD, deleted) | **corrupted, reprovisioned** |
| sdf | 5G | pvc-538d5f49 | trial ClickHouse | healthy |
| sdg | 1G | pvc-599ea8be | Redis (kyomi) | healthy |
| sdh | 10G | pvc-327ece2b | territoryguru (1 of 2) | healthy |
| sdi | 4G | pvc-7d0bc94b | territoryguru shapefiles | healthy (was degraded, fixed) |
| sdk | 5G | pvc-eedbddc4 | **unknown** | healthy |

**TODO:** Identify workloads for `pvc-516d4a3d` (sdb), `pvc-fa8c5a02` (sdc), and `pvc-eedbddc4` (sdk). The first two have known EXT4 errors and need proactive assessment.
