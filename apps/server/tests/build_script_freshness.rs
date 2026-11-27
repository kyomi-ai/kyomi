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
fn missing_input_does_not_make_the_artifact_stale() {
    let dir = scratch_dir("missing-input");
    let artifact = dir.join("chart_app.html");
    write_artifact_at(&artifact, future_base());

    // The build script rejects a missing input before asking; staleness must
    // not turn it into a rebuild on every build.
    assert!(!build_support::is_stale(&artifact, &[dir.join("absent")]));

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

// ---------------------------------------------------------------------------
// The input list (KYO-1001)
// ---------------------------------------------------------------------------

/// A repository layout holding every file `chart_app_inputs` names, so the
/// list can be compared against real paths.
fn fake_repo(test: &str) -> (PathBuf, PathBuf) {
    let repo = scratch_dir(test);
    let manifest_dir = repo.join("apps").join("server");
    for file in [
        "Cargo.toml",
        "rust-toolchain.toml",
        ".cargo/config.toml",
        "apps/mcp-chart-app-wasm/Cargo.toml",
        "apps/server/build.rs",
        "apps/server/build_support.rs",
    ] {
        write_input_at(&repo.join(file), 1);
    }
    (repo, manifest_dir)
}

#[test]
fn inputs_track_the_root_manifest_the_cargo_config_and_the_toolchain_pin() {
    let (repo, manifest_dir) = fake_repo("inputs-root");

    let inputs = build_support::chart_app_inputs(&repo, &manifest_dir, None, &[]);

    for tracked in ["Cargo.toml", ".cargo/config.toml", "rust-toolchain.toml"] {
        assert!(
            inputs.contains(&repo.join(tracked)),
            "{tracked} changes what the chart app compiles, so a change to it must rebuild the app; \
             inputs were {inputs:?}"
        );
    }

    fs::remove_dir_all(&repo).expect("remove scratch dir");
}

#[test]
fn a_change_to_the_root_manifest_or_cargo_config_makes_the_artifact_stale() {
    for changed in ["Cargo.toml", ".cargo/config.toml"] {
        let (repo, manifest_dir) = fake_repo("inputs-stale");
        let artifact = repo.join("apps/mcp-chart-app-wasm/chart_app.html");
        let base = future_base();
        write_artifact_at(&artifact, base);
        let inputs = build_support::chart_app_inputs(&repo, &manifest_dir, None, &[]);
        assert!(
            !build_support::is_stale(&artifact, &inputs),
            "precondition: an artifact newer than every input is fresh"
        );

        write_input_at(&repo.join(changed), base + 10);

        assert!(
            build_support::is_stale(&artifact, &inputs),
            "editing {changed} after the artifact was built must make it stale"
        );
        fs::remove_dir_all(&repo).expect("remove scratch dir");
    }
}

#[test]
fn cargo_configuration_in_ancestor_directories_and_cargo_home_is_tracked() {
    let dir = scratch_dir("config-files");
    let chart_app = dir.join("work").join("repo").join("apps").join("chart");
    fs::create_dir_all(&chart_app).expect("create chart app dir");
    let repo_config = dir.join("work/repo/.cargo/config.toml");
    let outer_config = dir.join("work/.cargo/config");
    let home = dir.join("home/.cargo");
    let home_config = home.join("config.toml");
    for file in [&repo_config, &outer_config, &home_config] {
        write_input_at(file, 1);
    }

    let files = build_support::cargo_config_files(&chart_app, Some(&home));

    for expected in [&repo_config, &outer_config, &home_config] {
        assert!(files.contains(expected), "{expected:?} applies to the build; got {files:?}");
    }
    assert!(
        !files.contains(&dir.join("work/repo/apps/.cargo/config.toml")),
        "a configuration file that does not exist must not be tracked, or Cargo \
         reruns the build script on every build"
    );

    fs::remove_dir_all(&dir).expect("remove scratch dir");
}

#[test]
fn a_user_level_patch_override_is_tracked_through_cargo_home() {
    let (repo, manifest_dir) = fake_repo("inputs-patch");
    let home = repo.join("home").join(".cargo");
    let user_config = home.join("config.toml");
    write_at(
        &user_config,
        1,
        b"[patch.crates-io]\nchartml-core = { path = \"/somewhere/chartml-core\" }\n",
    );

    let inputs = build_support::chart_app_inputs(&repo, &manifest_dir, Some(&home), &[]);

    assert!(
        inputs.contains(&user_config),
        "the file that holds a local [patch.crates-io] override must be an input; got {inputs:?}"
    );

    fs::remove_dir_all(&repo).expect("remove scratch dir");
}

const METADATA: &str = r#"{
  "packages": [
    {"name": "mcp-chart-app-wasm", "source": null, "manifest_path": "/repo/apps/mcp-chart-app-wasm/Cargo.toml"},
    {"name": "kyomi-chart-theme", "source": null, "manifest_path": "/repo/crates/kyomi-chart-theme/Cargo.toml"},
    {"name": "chartml-core", "source": null, "manifest_path": "/work/chartml/crates/chartml-core/Cargo.toml"},
    {"name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index", "manifest_path": "/cache/serde/Cargo.toml"}
  ]
}"#;

#[test]
fn local_packages_are_the_ones_without_a_registry_source() {
    let exclude = Path::new("/repo/apps/mcp-chart-app-wasm/Cargo.toml");

    let manifests = build_support::local_package_manifests(METADATA, exclude)
        .expect("well-formed metadata is read");

    assert_eq!(
        manifests,
        vec![
            PathBuf::from("/repo/crates/kyomi-chart-theme/Cargo.toml"),
            PathBuf::from("/work/chartml/crates/chartml-core/Cargo.toml"),
        ],
        "the chart app itself and registry packages are not local packages"
    );
}

#[test]
fn a_patched_checkouts_sources_make_the_artifact_stale_when_they_change() {
    let (repo, manifest_dir) = fake_repo("inputs-patched-source");
    let checkout = repo.join("chartml").join("crates").join("chartml-core");
    let manifest = checkout.join("Cargo.toml");
    write_input_at(&manifest, 1);
    write_input_at(&checkout.join("src").join("lib.rs"), 1);
    // The checkout's own build output must not count as an input.
    write_input_at(&checkout.join("target").join("debug").join("noise"), 1);
    let artifact = repo.join("apps/mcp-chart-app-wasm/chart_app.html");
    let base = future_base();
    write_artifact_at(&artifact, base);
    let inputs =
        build_support::chart_app_inputs(&repo, &manifest_dir, None, std::slice::from_ref(&manifest));
    assert!(!build_support::is_stale(&artifact, &inputs));

    write_input_at(&checkout.join("target").join("debug").join("noise"), base + 10);
    assert!(
        !build_support::is_stale(&artifact, &inputs),
        "a patched checkout's build output is not an input"
    );

    write_input_at(&checkout.join("src").join("lib.rs"), base + 20);
    assert!(
        build_support::is_stale(&artifact, &inputs),
        "editing a patched checkout's source after the artifact was built must make it stale"
    );

    fs::remove_dir_all(&repo).expect("remove scratch dir");
}

#[test]
fn metadata_that_is_not_the_expected_shape_is_an_error_not_an_empty_list() {
    let exclude = Path::new("/repo/apps/mcp-chart-app-wasm/Cargo.toml");

    for bad in ["not json", "{}", r#"{"packages": [{"source": null}]}"#] {
        assert!(
            build_support::local_package_manifests(bad, exclude).is_err(),
            "{bad:?} must not read as 'no local packages', which would stop tracking a patched checkout"
        );
    }
}
