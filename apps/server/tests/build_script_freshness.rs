// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for the freshness decision in `apps/server/build.rs` (KYO-993).
//!
//! `build.rs` is never a `cargo test` target, so the logic it shares with the
//! server build lives in `build_support.rs` and is compiled here exactly as
//! the build script compiles it.

#[path = "../build_support.rs"]
mod build_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The shape every complete chart app has: the inliner's doctype and closing
/// tag. `is_stale` treats anything else as a failed or interrupted write.
const COMPLETE_ARTIFACT: &[u8] = b"<!DOCTYPE html>\n<html>\n<body></body>\n</html>\n";

fn scratch_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "kyomi-chart-app-freshness-{}-{test}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs()
}

/// A base mtime clear of the real "now" timestamps that creating files leaves
/// on their parent directories, so only the files `write_at` touches decide
/// the answer.
fn future_base() -> u64 {
    now_seconds() + 10_000
}

/// Create `path` with `contents` and set its modification time to `seconds`
/// after the epoch.
fn write_at(path: &Path, seconds: u64, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directory");
    }
    fs::write(path, contents).expect("write file");
    fs::File::options()
        .write(true)
        .open(path)
        .expect("open file to set its mtime")
        .set_modified(UNIX_EPOCH + Duration::from_secs(seconds))
        .expect("set modification time");
}

fn write_input_at(path: &Path, seconds: u64) {
    write_at(path, seconds, b"x");
}

fn write_artifact_at(path: &Path, seconds: u64) {
    write_at(path, seconds, COMPLETE_ARTIFACT);
}

#[test]
fn missing_artifact_is_stale() {
    let dir = scratch_dir("missing");
    let artifact = dir.join("chart_app.html");
    let input = dir.join("src").join("main.rs");
    write_input_at(&input, future_base() - 10);

    assert!(build_support::is_stale(&artifact, &[input]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn complete_artifact_newer_than_every_input_is_fresh() {
    let dir = scratch_dir("fresh");
    let artifact = dir.join("chart_app.html");
    let input = dir.join("Cargo.lock");
    let base = future_base();
    write_input_at(&input, base - 10);
    write_artifact_at(&artifact, base);

    assert!(!build_support::is_stale(&artifact, &[input]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn input_with_the_same_mtime_as_the_artifact_is_stale() {
    let dir = scratch_dir("same-mtime");
    let artifact = dir.join("chart_app.html");
    let input = dir.join("Cargo.lock");
    let base = future_base();
    write_input_at(&input, base);
    write_artifact_at(&artifact, base);

    assert!(build_support::is_stale(&artifact, &[input]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn nested_input_newer_than_the_artifact_is_stale() {
    let dir = scratch_dir("nested-newer");
    let artifact = dir.join("chart_app.html");
    let input = dir.join("src").join("app.rs");
    let base = future_base();
    write_artifact_at(&artifact, base);
    write_input_at(&input, base + 10);

    assert!(build_support::is_stale(&artifact, &[dir.join("src")]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn missing_input_makes_the_artifact_stale() {
    let dir = scratch_dir("missing-input");
    let artifact = dir.join("chart_app.html");
    write_artifact_at(&artifact, future_base());

    assert!(build_support::is_stale(&artifact, &[dir.join("absent")]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn truncated_artifact_newer_than_its_inputs_is_stale() {
    let dir = scratch_dir("truncated");
    let artifact = dir.join("chart_app.html");
    let input = dir.join("src").join("main.rs");
    let base = future_base();
    write_input_at(&input, base - 10);
    // A failed or interrupted write leaves a prefix of the page with a fresh
    // mtime; embedding it compiles and draws nothing.
    write_at(&artifact, base, b"<!DOCTYPE html>\n<html>\n<head><meta char");

    assert!(build_support::is_stale(&artifact, &[input]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn empty_artifact_newer_than_its_inputs_is_stale() {
    let dir = scratch_dir("empty");
    let artifact = dir.join("chart_app.html");
    let input = dir.join("src").join("main.rs");
    let base = future_base();
    write_input_at(&input, base - 10);
    write_at(&artifact, base, b"");

    assert!(build_support::is_stale(&artifact, &[input]));

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn newest_modified_walks_a_directory_tree() {
    let dir = scratch_dir("newest");
    let base = future_base();
    let newest = dir.join("src").join("nested").join("deep.rs");
    write_input_at(&dir.join("src").join("main.rs"), base - 10);
    write_input_at(&newest, base + 10);

    assert_eq!(
        build_support::newest_modified(&dir.join("src")),
        Some(UNIX_EPOCH + Duration::from_secs(base + 10))
    );
    assert_eq!(build_support::newest_modified(&dir.join("absent")), None);

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}
