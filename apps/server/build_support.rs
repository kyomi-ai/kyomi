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
/// An input whose own metadata cannot be read counts as stale: freshness
/// cannot be proven, and `build.sh` reports the missing file itself. Timestamps
/// are the only freshness signal available, because `build.sh` writes no
/// manifest of its inputs. Equality counts as stale so that a same-tick write
/// never hides a change.
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
        .any(|input| newest_modified(input).is_none_or(|modified| modified >= artifact_modified))
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
