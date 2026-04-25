//! Build script for `kyomi-server`.
//!
//! Ensures that `apps/mcp-chart-app-wasm/chart_app.html` (embedded into the
//! MCP chart resource via `include_str!` in `routes/mcp.rs`) exists at compile
//! time. The file is a build artifact produced by `build.sh` in
//! `apps/mcp-chart-app-wasm` (Trunk + Python inliner) and is gitignored.
//!
//! Behaviour:
//!
//! 1. If `chart_app.html` already exists locally, do nothing.
//! 2. Otherwise, attempt to run `build.sh` in `apps/mcp-chart-app-wasm`.
//! 3. If the build fails (e.g. in a git worktree where trunk can't resolve
//!    paths), copy the artifact from the main worktree.
//! 4. If all of the above fail, emit a clear cargo error.
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
            if try_copy_from_main_worktree(&chart_app_html) {
                println!(
                    "cargo:warning=chart_app.html copied from main worktree. \
                     Run `cd apps/mcp-chart-app-wasm && bash build.sh` to rebuild locally."
                );
            } else {
                panic!(
                    "kyomi-server build.rs: `apps/mcp-chart-app-wasm/chart_app.html` is missing \
                     and could not be generated automatically or copied from the main worktree. \
                     Run the following manually, then retry:\n\n    \
                     cd {} && bash build.sh\n",
                    wasm_app.display()
                );
            }
        }
    }
}

fn try_copy_from_main_worktree(local_path: &PathBuf) -> bool {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => o,
        _ => return false,
    };

    let stdout = match std::str::from_utf8(&output.stdout) {
        Ok(s) => s,
        Err(_) => return false,
    };

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            let candidate = PathBuf::from(path)
                .join("apps")
                .join("mcp-chart-app-wasm")
                .join("chart_app.html");
            if candidate != *local_path && candidate.exists() {
                return std::fs::copy(&candidate, local_path).is_ok();
            }
        }
    }

    false
}
