// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure freshness logic shared between `apps/server/build.rs` and its test.
//
// `build.rs` is never a `cargo test` target — Cargo compiles and runs it only
// as the build-script binary, so a `#[cfg(test)] mod` written directly inside
// `build.rs` is silently never executed by `cargo test` (the same reason
// `crates/kyomi-embed/build_support.rs` exists, KYO-510). This file is pulled
// into `build.rs` via `include!`, and into `apps/server/tests/
// build_script_freshness.rs` via `#[path]`, so `cargo test` exercises the
// exact source the build script runs.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// True when `chart_app.html` is missing, incomplete, or older than any input.
///
/// An input that cannot be read is skipped: the build script rejects a missing
/// input before it asks, because treating it as stale would rebuild the chart
/// app on every build instead of failing once. Timestamps are the only
/// freshness signal available, because `build.sh` writes no manifest of its
/// inputs. Equality counts as stale so that a same-tick write never hides a
/// change.
pub fn is_stale(chart_app_html: &Path, inputs: &[PathBuf]) -> bool {
    let artifact_modified =
        match fs::metadata(chart_app_html).and_then(|metadata| metadata.modified()) {
            Ok(modified) => modified,
            Err(_) => return true,
        };
    if !is_complete_artifact(chart_app_html) {
        return true;
    }

    inputs
        .iter()
        .any(|input| newest_modified(input).is_some_and(|modified| modified >= artifact_modified))
}

/// Newest modification time at or below `path`; `None` when it cannot be read.
pub fn newest_modified(path: &Path) -> Option<SystemTime> {
    let metadata = fs::metadata(path).ok()?;
    let mut newest = metadata.modified().ok()?;
    if metadata.is_dir()
        && let Ok(entries) = fs::read_dir(path)
    {
        for entry in entries.flatten() {
            if let Some(modified) = newest_modified(&entry.path()) {
                newest = newest.max(modified);
            }
        }
    }
    Some(newest)
}

/// Whether `path` is a complete single-file chart app.
///
/// The inliner always writes a page that starts with the doctype and ends with
/// `</html>`. A build that failed or was interrupted while writing leaves a
/// truncated file with a fresh timestamp, and `include_str!` compiles any
/// valid UTF-8 silently, so an incomplete artifact must count as stale rather
/// than be embedded. The build script rejects one it just produced for the same
/// reason: treating it as stale forever would rebuild the chart app on every
/// build instead of failing once.
fn is_complete_artifact(path: &Path) -> bool {
    let Ok(html) = fs::read_to_string(path) else {
        return false;
    };
    html.starts_with("<!DOCTYPE html>") && html.trim_end().ends_with("</html>")
}

/// Every input `build.sh` reads for the chart app, as paths for `build.rs` to
/// track and for [`is_stale`] to compare against the artifact.
///
/// `cargo_home` is where Cargo keeps the user's own configuration
/// (`$CARGO_HOME`, else `~/.cargo`). `local_manifests` are the `Cargo.toml` of
/// every package the chart app builds from a local path (see
/// [`local_package_manifests`]).
///
/// Why each one is here, and what was left out on purpose:
///
/// - The chart app's own sources, manifest, lock, scripts and Tailwind entry
///   point, and the two in-repo crates it compiles in.
/// - The root `Cargo.toml`: the in-repo crates inherit their dependency
///   versions and features from `[workspace.dependencies]`, so a change there
///   changes what the chart app compiles without touching any file above.
/// - Every Cargo configuration file that applies to the build (see
///   [`cargo_config_files`]): the repo's `.cargo/config.toml` carries the wasm32
///   `rustflags` (`--cfg=has_std`, `--cfg=leptos_debuginfo`) that shape the
///   app, and a `[patch.crates-io]` override, which only the user's own
///   configuration may hold, changes which chartml sources are compiled.
/// - The sources of every local-path package, which is how a patched chartml
///   checkout is rebuilt when its code changes.
/// - `rust-toolchain.toml`: the compiler is an input to the WASM it emits.
///
/// The root `Cargo.lock` is not an input: the chart app resolves from its own
/// `Cargo.lock` (`build.sh` fails when that is stale), which is tracked.
pub fn chart_app_inputs(
    repo_root: &Path,
    manifest_dir: &Path,
    cargo_home: Option<&Path>,
    local_manifests: &[PathBuf],
) -> Vec<PathBuf> {
    let chart_app_dir = repo_root.join("apps").join("mcp-chart-app-wasm");
    let mut inputs = vec![
        chart_app_dir.join("src"),
        chart_app_dir.join("Cargo.toml"),
        chart_app_dir.join("Cargo.lock"),
        chart_app_dir.join("build.sh"),
        chart_app_dir.join("inline_chart_app.py"),
        chart_app_dir.join("index.html"),
        chart_app_dir.join("Trunk.toml"),
        chart_app_dir.join("style").join("main.css"),
        // `kyomi-ui-components` is compiled into the chart app and scanned by
        // its Tailwind `@source` directives; `kyomi-chart-theme` is compiled
        // into it. A change to either makes the embedded app stale.
        repo_root.join("crates").join("kyomi-ui-components"),
        repo_root.join("crates").join("kyomi-chart-theme"),
        repo_root.join("Cargo.toml"),
        repo_root.join(".cargo").join("config.toml"),
        repo_root.join("rust-toolchain.toml"),
        // This script's own files: an artifact made by an older build recipe
        // is rebuilt once after the recipe changes.
        manifest_dir.join("build.rs"),
        manifest_dir.join("build_support.rs"),
    ];
    inputs.extend(cargo_config_files(&chart_app_dir, cargo_home));
    for manifest in local_manifests {
        inputs.extend(local_package_inputs(manifest));
    }
    inputs.sort();
    inputs.dedup();
    inputs
}

/// The Cargo configuration files that exist and apply to a build run from
/// `chart_app_dir`: `.cargo/config.toml` (or the older `.cargo/config`) in that
/// directory and each of its ancestors, then the same files in `cargo_home`.
///
/// Only files that exist are returned, because Cargo reruns a build script on
/// every invocation when a path it was told to track is missing. A file created
/// later is picked up by the next run that something else triggers; the
/// override it holds cannot change the output unnoticed in the meantime,
/// because a changed resolution fails `build.sh`'s `cargo metadata --locked`
/// until the lock is regenerated, and the lock is tracked.
pub fn cargo_config_files(chart_app_dir: &Path, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    for dir in chart_app_dir.ancestors() {
        let cargo_dir = dir.join(".cargo");
        candidates.push(cargo_dir.join("config.toml"));
        candidates.push(cargo_dir.join("config"));
    }
    if let Some(home) = cargo_home {
        candidates.push(home.join("config.toml"));
        candidates.push(home.join("config"));
    }
    candidates.retain(|candidate| candidate.is_file());
    candidates.sort();
    candidates.dedup();
    candidates
}

/// The `Cargo.toml` of every package built from a local path, read from the
/// output of `cargo metadata --format-version 1` for the chart app.
///
/// Those are the packages whose sources live outside Cargo's registry cache:
/// the in-repo crates, and any crate a local `[patch.crates-io]` override points
/// at a checkout. `exclude` (the chart app's own manifest) is left out, because
/// the chart app's own inputs are tracked individually.
///
/// A metadata document that is not the expected shape is an error rather than
/// an empty list: an empty list would silently stop tracking a patched checkout.
pub fn local_package_manifests(metadata_json: &str, exclude: &Path) -> Result<Vec<PathBuf>, String> {
    let metadata: serde_json::Value = serde_json::from_str(metadata_json)
        .map_err(|error| format!("cargo metadata output is not JSON: {error}"))?;
    let packages = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
        .ok_or("cargo metadata output has no `packages` array")?;

    let mut manifests = Vec::new();
    for package in packages {
        // `source` is null for a package built from a local path.
        if !package.get("source").is_some_and(serde_json::Value::is_null) {
            continue;
        }
        let manifest = package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            .ok_or("a local package in cargo metadata output has no `manifest_path`")?;
        let manifest = PathBuf::from(manifest);
        if manifest != exclude {
            manifests.push(manifest);
        }
    }
    Ok(manifests)
}

/// What to track for one local package: its manifest, its `build.rs` and its
/// `src` directory, when they exist. The rest of its directory is left out so a
/// checkout outside this repository, which holds its own `target/` and `.git/`,
/// does not make every build of the server look stale.
fn local_package_inputs(manifest: &Path) -> Vec<PathBuf> {
    let Some(dir) = manifest.parent() else {
        return vec![manifest.to_path_buf()];
    };
    let mut inputs = vec![manifest.to_path_buf()];
    for optional in ["build.rs", "src"] {
        let path = dir.join(optional);
        if path.exists() {
            inputs.push(path);
        }
    }
    inputs
}
