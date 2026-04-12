//! Build script for `kyomi-server`.
//!
//! Ensures that `apps/mcp-chart-app/chart_app.html` (embedded into the MCP
//! chart resource via `include_str!` in `routes/mcp.rs`) exists at compile
//! time. The file is a build artifact produced by the Vite build in
//! `apps/mcp-chart-app` and is intentionally gitignored. Without this script,
//! a fresh clone or worktree fails to compile because the include target is
//! missing.
//!
//! Behaviour:
//!
//! 1. If `chart_app.html` already exists, do nothing (hot path — no work on
//!    every rebuild).
//! 2. If `dist/mcp-app.html` exists (the Vite build ran previously but the
//!    copy step was skipped), copy it into place.
//! 3. Otherwise, attempt to run `npm run build` in `apps/mcp-chart-app`.
//!    Requires `npm` in `PATH`. On failure, emit a clear cargo error
//!    explaining how to fix it manually.
//!
//! Rerun triggers: only when the script itself or the mcp-chart-app source
//! files change — rebuilds of unrelated Rust code do not re-invoke this.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let mcp_chart_app = manifest_dir
        .join("..")
        .join("..")
        .join("apps")
        .join("mcp-chart-app");
    let chart_app_html = mcp_chart_app.join("chart_app.html");
    let dist_html = mcp_chart_app.join("dist").join("mcp-app.html");

    // Rerun triggers — script re-runs when any of these change. Critically
    // we watch `chart_app.html` itself: per the Cargo reference, if a
    // `rerun-if-changed` target path does not exist, the script always
    // re-runs. That guarantees we regenerate the artifact if a developer
    // deletes it or a fresh checkout lacks it, instead of Cargo caching the
    // build-script output and handing the compiler a missing include path.
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("build.rs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        chart_app_html.display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        mcp_chart_app.join("src").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        mcp_chart_app.join("package.json").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        mcp_chart_app.join("vite.config.js").display()
    );

    if chart_app_html.exists() {
        return;
    }

    if dist_html.exists() {
        copy_or_fail(&dist_html, &chart_app_html);
        return;
    }

    if !try_npm_build(&mcp_chart_app) {
        fail_with_instructions(&mcp_chart_app);
    }

    // After the build, the post-build copy step in package.json should have
    // written chart_app.html. Verify before we declare success.
    if !chart_app_html.exists() {
        // `npm run build` succeeded but the post-build `cp` in package.json
        // was skipped (e.g. on non-POSIX shells without `cp`). Copy the
        // Vite output directly from Rust so we stay cross-platform.
        if dist_html.exists() {
            copy_or_fail(&dist_html, &chart_app_html);
        } else {
            fail_with_instructions(&mcp_chart_app);
        }
    }
}

fn try_npm_build(mcp_chart_app: &Path) -> bool {
    // Ensure dependencies are installed.
    let install = Command::new("npm")
        .args(["install", "--silent"])
        .current_dir(mcp_chart_app)
        .status();
    let Ok(install_status) = install else {
        return false;
    };
    if !install_status.success() {
        return false;
    }

    let build = Command::new("npm")
        .args(["run", "build"])
        .current_dir(mcp_chart_app)
        .status();
    matches!(build, Ok(status) if status.success())
}

fn copy_or_fail(src: &Path, dst: &Path) {
    if let Err(e) = std::fs::copy(src, dst) {
        panic!(
            "kyomi-server build.rs: failed to copy {} -> {}: {e}",
            src.display(),
            dst.display()
        );
    }
}

fn fail_with_instructions(mcp_chart_app: &Path) -> ! {
    panic!(
        "kyomi-server build.rs: `apps/mcp-chart-app/chart_app.html` is missing \
         and could not be generated automatically (npm not found or build \
         failed). Run the following manually, then retry:\n\n    \
         cd {} && npm install && npm run build\n",
        mcp_chart_app.display()
    );
}
