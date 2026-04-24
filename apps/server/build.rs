//! Build script for `kyomi-server`.
//!
//! Ensures that `apps/mcp-chart-app-wasm/chart_app.html` (embedded into the
//! MCP chart resource via `include_str!` in `routes/mcp.rs`) exists at compile
//! time. The file is a build artifact produced by `build.sh` in
//! `apps/mcp-chart-app-wasm` (Trunk + Python inliner) and is gitignored.
//!
//! Behaviour:
//!
//! 1. If `chart_app.html` already exists, do nothing.
//! 2. Otherwise, attempt to run `build.sh` in `apps/mcp-chart-app-wasm`.
//!    Requires `trunk` and `python3` in `PATH`. On failure, emit a clear
//!    cargo error explaining how to fix it manually.
//!
//! Rerun triggers: only when the script itself or the WASM app source files
//! change.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
    );
    let wasm_app = manifest_dir
        .join("..")
        .join("..")
        .join("apps")
        .join("mcp-chart-app-wasm");
    let chart_app_html = wasm_app.join("chart_app.html");

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
        wasm_app.join("src").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        wasm_app.join("Cargo.toml").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        wasm_app.join("build.sh").display()
    );

    if chart_app_html.exists() {
        return;
    }

    let build = Command::new("bash")
        .args(["build.sh"])
        .current_dir(&wasm_app)
        .status();

    match build {
        Ok(status) if status.success() && chart_app_html.exists() => {}
        _ => {
            panic!(
                "kyomi-server build.rs: `apps/mcp-chart-app-wasm/chart_app.html` is missing \
                 and could not be generated automatically. Run the following manually, \
                 then retry:\n\n    \
                 cd {} && bash build.sh\n",
                wasm_app.display()
            );
        }
    }
}
