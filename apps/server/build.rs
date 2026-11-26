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

    // Everything `build.sh` reads from the chart app. Directories are tracked
    // recursively by Cargo; the list stays explicit because the chart app
    // directory also holds build outputs (`chart_app.html`, `dist/`,
    // `style/output.css` and `target/`) that must not become rerun triggers.
    // `style/` is represented by its Tailwind entry point rather than the
    // directory for the same reason: tracking the directory would rerun this
    // script — and relink the server — after every build that rewrites the
    // generated `output.css`.
    let inputs = [
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
        // This script's own files: an artifact made by an older build recipe
        // is rebuilt once after the recipe changes.
        manifest_dir.join("build.rs"),
        manifest_dir.join("build_support.rs"),
    ];

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
