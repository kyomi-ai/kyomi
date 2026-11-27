//! Build script for `kyomi-server`.
//!
//! `apps/mcp-chart-app-wasm/chart_app.html` is embedded into the MCP chart
//! resource via `include_str!` in `routes/mcp.rs`. It is a build artifact
//! produced by `build.sh` in `apps/mcp-chart-app-wasm` (Trunk + the
//! `inline_chart_app.py` inliner) and is gitignored, so every worktree has to
//! produce its own.
//!
//! Behaviour:
//!
//! 1. When Cargo runs this script and `chart_app.html` is missing, incomplete,
//!    or older than any chart app input, run `build.sh` to rebuild it.
//! 2. Otherwise keep the existing artifact.
//! 3. If `build.sh` fails, fail the server build with the fix. The artifact is
//!    never copied from another worktree: a file built from a different branch
//!    or dependency version makes local verification meaningless.
//!
//! Cargo reruns this script only when one of the tracked files below changes,
//! so an unchanged tree does no work. The timestamp check covers the other
//! case: the first build in a target directory where the artifact already
//! exists (CI runs `build.sh` before `cargo build`), which must not build the
//! chart app a second time.
//!
//! `chart_app.html` is also tracked, so deleting it or rebuilding it outside
//! Cargo still reruns this script. After a rebuild it makes Cargo run the
//! script once more on the next build; that run takes the "up to date" branch,
//! so it costs a script run and one recompile of `kyomi-server`, never a second
//! chart app build.

use std::process::Command;

// `is_stale`/`newest_modified`, shared with `tests/build_script_freshness.rs`.
include!("build_support.rs");

fn main() {
    let manifest_dir = std::path::PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
    );
    let repo_root = manifest_dir
        .ancestors()
        .nth(2)
        .expect("apps/server has a repository root")
        .to_path_buf();
    let chart_app_dir = repo_root.join("apps").join("mcp-chart-app-wasm");
    let chart_app_html = chart_app_dir.join("chart_app.html");

    // Everything `build.sh` reads from the chart app: see `chart_app_inputs`
    // for what is tracked and why. Directories are tracked recursively by
    // Cargo; the list stays explicit because the chart app directory also holds
    // build outputs (`chart_app.html`, `dist/`, `style/output.css` and
    // `target/`) that must not become rerun triggers. `style/` is represented
    // by its Tailwind entry point rather than the directory for the same
    // reason: tracking the directory would rerun this script — and recompile
    // the server — after every build that rewrites the generated `output.css`.
    let local_manifests = local_package_manifests(
        &chart_app_metadata(&chart_app_dir),
        &chart_app_dir.join("Cargo.toml"),
    )
    .unwrap_or_else(|error| {
        panic!("kyomi-server build.rs: could not list the chart app's local packages: {error}")
    });
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".cargo"))
        });
    let inputs = chart_app_inputs(
        &repo_root,
        &manifest_dir,
        cargo_home.as_deref(),
        &local_manifests,
    );
    println!("cargo:rerun-if-env-changed=CARGO_HOME");

    for input in &inputs {
        println!("cargo:rerun-if-changed={}", input.display());
    }
    println!("cargo:rerun-if-changed={}", chart_app_html.display());

    // Cargo reruns a script whose tracked path is missing on every invocation,
    // so a renamed or removed input would rebuild the chart app on every build
    // without ever failing. Fail once, naming the input, instead.
    if let Some(missing) = inputs.iter().find(|input| !input.exists()) {
        panic!(
            "kyomi-server build.rs: the chart app input list is out of date: {} does not exist. \
             Update the list in apps/server/build.rs to match what \
             apps/mcp-chart-app-wasm/build.sh reads.",
            missing.display()
        );
    }

    if !is_stale(&chart_app_html, &inputs) {
        eprintln!(
            "kyomi-server build.rs: chart_app.html is up to date; \
             not rebuilding the MCP chart app"
        );
        return;
    }

    eprintln!(
        "kyomi-server build.rs: rebuilding the MCP chart app \
         (chart_app.html is missing, incomplete or older than its inputs)"
    );

    match Command::new("bash")
        .arg("build.sh")
        .current_dir(&chart_app_dir)
        // A build script runs with the outer Cargo's lint settings, and the
        // nested `build.sh` would inherit them:
        //   - `CARGO_ENCODED_RUSTFLAGS` is set, and a nested `cargo` treats a
        //     set value as authoritative, so it ignores `.cargo/config.toml`'s
        //     wasm32 rustflags. The chart app was then built without
        //     `--cfg=leptos_debuginfo` and `--cfg=has_std`. On Linux the value
        //     also carries the host's mold link argument, which a wasm32 build
        //     must not get.
        //   - Under `cargo clippy`, `RUSTC_WORKSPACE_WRAPPER` points at
        //     clippy-driver and `CLIPPY_ARGS` carries the outer clippy flags
        //     (`-- -D warnings`), so the chart app would be linted as part of
        //     the server's clippy run and fail on its own warnings.
        // Removing all three makes the nested build the same `bash build.sh`
        // the workflows run directly.
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CLIPPY_ARGS")
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => fail(
            &chart_app_dir,
            &format!("`bash build.sh` exited with {status}"),
        ),
        Err(error) => fail(
            &chart_app_dir,
            &format!("could not run `bash build.sh`: {error}"),
        ),
    }

    // The counterpart of the incomplete-artifact rule in `is_stale`: an
    // artifact that build.sh reports success for but that is not a complete
    // page is stale forever, so without this check the next build would run
    // build.sh again and loop — a full chart app build per cargo invocation.
    // Fail here instead, where the reason is visible.
    if !is_complete_artifact(&chart_app_html) {
        fail(
            &chart_app_dir,
            "`bash build.sh` succeeded but did not write a complete chart_app.html",
        );
    }
}

/// `cargo metadata` for the chart app, which lists every package it builds and
/// where each one comes from. Run with `--locked`, as `build.sh` checks the
/// lock, so it never rewrites `Cargo.lock`. A failure stops the server build
/// here with Cargo's own message; `build.sh` would stop on the same cause.
fn chart_app_metadata(chart_app_dir: &std::path::Path) -> String {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(chart_app_dir)
        .output()
        .unwrap_or_else(|error| {
            panic!("kyomi-server build.rs: could not run `cargo metadata` for the chart app: {error}")
        });
    if !output.status.success() {
        panic!(
            "kyomi-server build.rs: `cargo metadata --locked` failed for the chart app ({}):\n{}\n\
             A stale apps/mcp-chart-app-wasm/Cargo.lock, or a local [patch.crates-io] override \
             whose lock has not been regenerated, is the usual cause: see CLAUDE.md, \
             \"External Crate Dependencies\".",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8(output.stdout)
        .unwrap_or_else(|error| panic!("kyomi-server build.rs: cargo metadata output is not UTF-8: {error}"))
}

/// Fail the server build with an actionable message.
fn fail(chart_app_dir: &std::path::Path, reason: &str) -> ! {
    panic!(
        "kyomi-server build.rs: building the MCP chart app failed: {reason}.\n\n\
         kyomi-server embeds apps/mcp-chart-app-wasm/chart_app.html with include_str!, and that \
         artifact must be built from this worktree's chart app sources. It is never copied from \
         another worktree, because an artifact from a different branch or dependency version \
         makes local verification meaningless.\n\n\
         Fix the failure reported above, then rebuild. To build the chart app directly, run:\n\n    \
         cd {} && bash build.sh\n\n\
         The chart app build needs `trunk`, `tailwindcss` and `python3`, and the \
         `wasm32-unknown-unknown` target; `.github/workflows/ci.yml` installs the first two in \
         \"Install trunk\" and \"Install tailwindcss\", the runner image provides python3, and the \
         target comes from rust-toolchain.toml via `setup-rust`. A stale \
         apps/mcp-chart-app-wasm/Cargo.lock also stops the build: update the lock in the same \
         change as the manifest.",
        chart_app_dir.display()
    );
}
