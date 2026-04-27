# Incident: Analytics ClickHouse Volume Corruption

**Date:** 2026-04-27
**Severity:** Data loss (analytics only)
**Duration:** ~6 hours investigation + recovery
**Data lost:** ~3,428 analytics events (Feb–Apr 2026)

## Symptoms

Charts sourcing from the analytics ClickHouse instance failed with:

```
Code: 76. DB::Exception: Cannot open file /var/lib/clickhouse/store/9b5/...all_3119_3119_1/data.cmrk4:
errno: 5, strerror: Input/output error
```

The error surfaced when ClickHouse's background merge tried to compact across a damaged region.

## Investigation Timeline

### 1. Initial error analysis

The error was on the `site_c034dde57a4b3b46` database, specifically a materialized view inner table (`.inner_id.f7195be9-d80a-4a12-85ed-3bd3a65c4e27`). Attempted to `DROP PART` but that also failed with I/O errors — the corruption wasn't limited to one part.

### 2. Checked ClickHouse logs

Found ongoing I/O errors during background merges:

```
filesystem error: in create_directories: Input/output error
["/var/lib/clickhouse/store/9b5/.../tmp_merge_all_1_3119_1047"]
```

### 3. Checked Longhorn volume health

- Analytics volume (`pvc-7115f254`) reported **healthy** with **1 replica**
- Two other volumes (minecraft, territoryguru) reported **degraded** — these had `replicas: 3` on a single-node cluster, so 2 replicas could never schedule. This was a misconfiguration, not corruption.

### 4. Checked physical disk (NVMe)

NVMe is healthy: 0 media errors, 0 data integrity errors, 100% available spare, 3% worn. **Not a hardware issue.**

However, SMART reported:
- **7 unsafe shutdowns** in 455 power-on hours
- **20 power cycles**

These occurred during initial server setup — not relevant to the incident.

### 5. Found kernel-level EXT4 errors

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

### 6. Assessed data recoverability

All 3 partitions (Feb, Mar, Apr 2026) were unreadable. The materialized view metadata files were also corrupted. Even `ls` on the metadata directory returned I/O errors. The entire volume was unrecoverable — superblock and alternate superblocks all returned I/O errors from `e2fsck`.

### 7. Recovery

1. Deleted corrupted PVC, Longhorn provisioned fresh 50Gi volume
2. Scaled ClickHouse back up — started with empty data directory
3. User re-created the analytics site from the Settings UI (new site_id: `0af68a4a0f6d95ec`)
4. Manually created `sessions` and `visitors` materialized views + public views (see "Collector TLS Bug" below for why the collector couldn't do this automatically)
5. Backfilled MV data from existing events
6. Updated analytics tracking snippet on all marketing pages (NAS HTML files + VitePress source in kyomi-private)

### 8. Mistakes made during recovery

**The agent operated on the local dev Postgres (`localhost:5433`) instead of the prod database inside k8s for the entire recovery session.** Every manual `INSERT`, `DELETE`, and `SELECT` against `datasource_configs` and `analytics_sites` was hitting the wrong database. This led to:

- Repeated confusion about why manually-created rows weren't visible to the app
- Multiple unnecessary `ALTER USER` commands on ClickHouse that broke working passwords the app had provisioned
- Hours of debugging a non-existent "shared_password encryption" issue
- The user having to delete and re-create the analytics site multiple times

**Lesson:** The `.env` file contains local dev credentials (`localhost:5433`). Prod Postgres is at `postgres:5432` inside k8s, accessible only from within the cluster. Always verify which database you're connected to before making changes. For prod database operations, use `kubectl exec` into a pod with database access.

## Root Cause

**CPU starvation from the CI build runner crashed Longhorn's iSCSI connections, causing in-flight write loss and EXT4 journal corruption on multiple volumes.**

The ARC runner pod (`kyomi-build`) was configured with `cpu: "14"` limits on a 16-core node. A `cargo build --release` with cold sccache pegged all 14 cores for ~15 minutes, leaving only 2 cores for the entire system. Longhorn's iSCSI NOP keepalives timed out, all volume connections dropped simultaneously, and in-flight writes were lost. The `dind` sidecar had no CPU limits at all.

The iSCSI NOP timeout was set to just 5 seconds (interval) + 5 seconds (timeout) = 10 seconds of CPU starvation to kill all volumes.

## What Was NOT the Cause

- **NVMe hardware failure** — SMART is clean, 0 media errors, 0 data integrity errors
- **Unsafe shutdowns during setup** — initial theory was wrong; the epoch timestamps in EXT4 errors converted to Apr 25 2026, not Jan/Feb
- **Longhorn bug** — Longhorn correctly reported volumes as "healthy" because replicas were running; it has no mechanism to detect latent EXT4 corruption within a volume

## Remediation Applied

1. **Reduced ARC runner CPU limits** from 14 → 8 cores (runner) and added 2-core limit to the dind sidecar. Combined maximum is 10 cores, leaving 6+ cores for system services during builds. `maxRunners` reduced from 2 → 1 to prevent concurrent builds from saturating the node.
   - File: `infra/arc/runners/kyomi-build.yaml`
   - Applied via: `helm upgrade kyomi-build ...`
2. **Increased iSCSI NOP timeouts** from 5s → 30s (both interval and timeout). Before: 10 seconds of CPU starvation killed all volumes. After: 60 seconds of complete starvation required. Combined with the CPU cap, this makes the failure mode practically impossible.
   - File: `/etc/iscsi/iscsid.conf` on prod node
   - `node.conn[0].timeo.noop_out_interval = 30` (was 5)
   - `node.conn[0].timeo.noop_out_timeout = 30` (was 5)
   - New settings apply to new iSCSI sessions; existing connections retain old timeouts until pod restart or node reboot
3. **Reprovisioned the corrupted analytics ClickHouse volume** (fresh PVC, empty database)
4. **Re-created analytics site** — user provisioned from UI, views created manually, marketing snippet updated
5. **Fixed Longhorn replica counts** — set all volumes to `replicas: 1` (single-node cluster)
6. **Fixed analytics collector TLS bug** — `ChHttpConfig::url()` was hardcoded to `http://`, preventing the transform engine from creating sessions/visitors MVs over TLS. Added `secure` field that reads `ANALYTICS_CLICKHOUSE_SECURE` env var. (commit `a00bcba` in kyomi-private)

## Collector TLS Bug (discovered during recovery)

The analytics collector's transform engine (`apps/analytics-collector/src/transform/engine.rs`) had `ChHttpConfig::url()` hardcoded to `http://` while the event ingestion path (`clickhouse.rs`) correctly read `ANALYTICS_CLICKHOUSE_SECURE`. On TLS-enabled clusters, the `ensure_database_schemas` call (which creates `_sessions`, `_visitors` MVs and their public views) silently failed on every new analytics site since internal TLS was enabled in March 2026.

The old site's views pre-dated the TLS migration and were never affected. New sites would never get views auto-created — a latent bug that only surfaced when we reprovisioned the volume and created a new site.

**Fix:** Added `secure: bool` to `ChHttpConfig`, reading from `ANALYTICS_CLICKHOUSE_SECURE`, and using `https://` when true. One-line logic change + struct field addition.

## TerritoryGuru Volume Corruption (same event)

Two TerritoryGuru volumes were also corrupted by the same iSCSI dropout:
- `sdb` (10G) → **MinIO** — I/O errors on file reads (1 PDF affected)
- `sdc` (400G) → **Postgres/PostGIS** (`gis` database) — completely dead, used by mapnik for PDF map rendering

TerritoryGuru's primary database is MongoDB (healthy, backed up daily to Backblaze B2 via restic). Recovery handled separately.

## PVC-to-Workload Mapping

| Device | Size | PVC | Workload | Status |
|--------|------|-----|----------|--------|
| sda | 20G | pvc-4ad97ced | Postgres (kyomi) | healthy |
| sdb | 10G | pvc-516d4a3d | TerritoryGuru MinIO | **corrupted** (EXT4 errors) |
| sdc | 400G | pvc-fa8c5a02 | TerritoryGuru Postgres/PostGIS | **corrupted** (EXT4 errors) |
| sdd | 10G | pvc-27406f0f | minecraft | healthy (replica count fixed) |
| sde | 50G | pvc-7115f254 | analytics ClickHouse (OLD, deleted) | **reprovisioned** |
| sdf | 5G | pvc-538d5f49 | trial ClickHouse | healthy |
| sdg | 1G | pvc-599ea8be | Redis (kyomi) | healthy |
| sdh | 10G | pvc-327ece2b | territoryguru (1 of 2) | healthy |
| sdi | 4G | pvc-7d0bc94b | territoryguru shapefiles | healthy (replica count fixed) |
| sdk | 5G | pvc-eedbddc4 | unknown | healthy |
