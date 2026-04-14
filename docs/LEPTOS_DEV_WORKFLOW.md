# Leptos Frontend Dev Workflow

How to iterate on Leptos frontend changes and test on dev.kyomi.ai.

## Quick Reference

```bash
# Frontend-only change? Just run:
cd crates/kyomi-ui && trunk build
# Then hard-refresh dev.kyomi.ai
```

No server rebuild needed — the server reads assets from disk.

## How It Works

The server binary is built with the `dev-server` Cargo profile, which sets `debug-assertions = true`. This activates `rust-embed`'s conditional code path that reads files from `crates/kyomi-ui/dist/` **at runtime from disk** instead of using the copy embedded at compile time.

```
Browser → nginx (NAS) → server :3000 → reads dist/ from disk
                                         ↑
                              trunk build writes here
```

**Important:** `rust-embed` without the `debug-embed` feature generates both an embedded path (`#[cfg(not(debug_assertions))]`) and a disk-reading path (`#[cfg(debug_assertions)]`). The `debug-assertions = true` in the profile activates the disk-reading path. Do NOT add the `debug-embed` feature — that forces embedding only.

## When to Rebuild What

| What changed | Command | Time |
|---|---|---|
| Leptos UI code (`kyomi-ui/src/`) | `cd crates/kyomi-ui && trunk build` | ~30s |
| Server functions (in `kyomi-ui/src/server_fns/`) | `cargo build --profile dev-server -p kyomi-server` + restart | ~7 min |
| Server routes (`apps/server/src/`) | `cargo build --profile dev-server -p kyomi-server` + restart | ~7 min |

## Starting the Server

If the server on :3000 isn't running:

```bash
cd /home/jason/repos/kyomi
set -a; source .env; set +a
PORT=3000 FRONTEND_URL=https://dev.kyomi.ai \
  nohup target/dev-server/kyomi > /tmp/kyomi-leptos.log 2>&1 &
```

Check it started:
```bash
grep "listening" /tmp/kyomi-leptos.log
```

## Rebuilding the Server Binary

Only needed when server-side Rust code changes (not for frontend-only changes):

```bash
cargo build --profile dev-server -p kyomi-server
# Then restart:
kill $(lsof -ti:3000); sleep 1
# Start again (see above)
```

## Infrastructure

- **nginx** on NAS (192.168.1.100) proxies `dev.kyomi.ai` → `192.168.1.200:3000`
- **Server** on :3000 serves both API and frontend (reads `dist/` from disk in dev)
- **Debug WASM** is ~250MB — first load takes 15s+, that's expected

## What NOT to Do

- **Don't use `scripts/dev/rebuild-leptos.sh`** for frontend-only changes — it does a full release trunk build (LTO + wasm-opt) and rebuilds the server binary.
- **Don't use `trunk build --release`** — adds LTO, wasm-opt, size optimization. Debug builds are fine for dev.
- **Don't add `features = ["debug-embed"]` to rust-embed** — that forces embedding and breaks disk reading.

## Profile Configuration

In `Cargo.toml`:

```toml
[profile.dev-server]
inherits = "release"
lto = false
codegen-units = 16
debug-assertions = true  # Activates rust-embed disk reading at runtime
```
