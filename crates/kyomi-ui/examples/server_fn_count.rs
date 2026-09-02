// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-275: a standalone, database-free smoke check that `inventory`'s
//! server-fn self-registration (KYO-191) survives `[profile.release]`
//! (`lto = true`, `codegen-units = 1`, `strip = true`, `panic = "abort"` —
//! see the workspace `Cargo.toml`).
//!
//! `server_fn_registration_tests::server_fn_registry_is_populated_via_inventory`
//! (`crates/kyomi-ui/src/lib.rs`) already asserts the registry has at least
//! `kyomi_ui::SERVER_FN_REGISTRY_FLOOR` entries — but that test runs under
//! `cargo test`, which always builds with the dev/test profile, never with
//! LTO. If a future toolchain or linker change ever caused LTO to strip
//! `inventory`'s `#[used]` link-section statics, that unit test would keep
//! passing while every server function 404ed in the actual production
//! binary, with nothing anywhere catching it. This binary is the guard for
//! that specific gap: build and run it `--release` and it exercises the
//! exact registration mechanism under the exact profile that ships.
//!
//! ## Usage
//!
//! ```sh
//! cargo build --release --example server_fn_count -p kyomi-ui \
//!     --features ssr,slack --locked
//! ./target/release/examples/server_fn_count
//! ```
//!
//! Both features matter, not just `ssr`: apps/server/Cargo.toml sets
//! `default = ["slack"]`, and release.yml builds `-p kyomi-server` with no
//! `--features` override, so the shipping binary always has `slack`
//! compiled in. `server_fns/slack.rs` and `server_fns/workspace.rs` both
//! carry paired `#[cfg(feature = "slack")]` /
//! `#[cfg(not(feature = "slack"))]` server_fn implementations — building
//! this example with `ssr` alone would silently register the
//! `not(slack)` variants instead of what production actually ships.
//!
//! Exits 0 and prints the count on success; exits 1 with a diagnostic
//! message otherwise. `panic = "abort"` applies to this binary too (it's
//! part of the workspace's `[profile.release]`), so failure is reported via
//! an explicit `ExitCode`, not `Result`-returning `main`/`unwrap`/`panic!` —
//! this must produce a clean, well-formatted stderr message rather than an
//! abort trace.
//!
//! ## The "0 means nothing was ever linked" trap
//!
//! An example binary only pulls in the parts of a dependency's rlib the
//! linker can prove are reachable from `main`. This file never calls into
//! `kyomi_ui` for anything else, so without the line below, nothing forces
//! the linker to include the crate's `#[server]`-annotated functions (and
//! the `inventory::submit!` statics the `#[server]` macro attaches to each
//! one) in this binary at all — the registry would read back as `0`
//! entries, which looks exactly like "LTO stripped the registration
//! statics" (the real failure this file exists to catch) but actually means
//! "this example is broken, not kyomi-ui." That false negative cost real
//! investigation time during KYO-191. Referencing `kyomi_ui::App` forces
//! the whole `kyomi_ui` rlib to link, which is enough under this
//! workspace's `codegen-units = 1` release profile (the crate compiles as
//! one unit, so anything reachable pulls in everything).
//!
//! A related, lower-severity version of the same mistake: building with
//! `--features ssr` alone (omitting `slack`) does NOT read back as `0` —
//! it reads back as a real but *wrong* number, because
//! `#[cfg(not(feature = "slack"))]` server_fn variants register in
//! `slack`'s place. That's a silent false confidence, not a false
//! negative, which is exactly why `required-features = ["ssr", "slack"]`
//! is set on this example's `[[example]]` entry in Cargo.toml rather than
//! left to whoever invokes `cargo build` to remember.
use std::process::ExitCode;

fn main() -> ExitCode {
    // See the "0 means nothing was ever linked" section above — do not
    // remove this line, and do not replace it with a comment-only
    // explanation of what it *would* do.
    let _app: fn() -> _ = kyomi_ui::App;

    let count = leptos::server_fn::axum::server_fn_paths().count();

    if count == 0 {
        eprintln!(
            "server_fn_count: registry is EMPTY (0 entries).\n\n\
             This is far more likely to mean this example binary itself \
             failed to link the kyomi_ui rlib (see the module docs at the \
             top of examples/server_fn_count.rs — the `kyomi_ui::App` \
             reference trap that cost real time during KYO-191) than a \
             genuine registration failure. Before treating this as a \
             KYO-191-class regression, confirm the example still \
             references `kyomi_ui::App` and was built with `--features \
             ssr,slack`."
        );
        return ExitCode::FAILURE;
    }

    if count < kyomi_ui::SERVER_FN_REGISTRY_FLOOR {
        eprintln!(
            "server_fn_count: registry has only {count} entries, below the \
             floor of {} (kyomi_ui::SERVER_FN_REGISTRY_FLOOR).\n\n\
             Under this release profile (lto = true, codegen-units = 1, \
             strip = true — see [profile.release] in the workspace \
             Cargo.toml), a collapsed count like this most likely means \
             `inventory`'s #[used] link-section statics were stripped by \
             the linker or LTO, not that server functions were actually \
             removed. See KYO-191 for the original investigation into this \
             registration mechanism (`server_fn::axum`'s \
             `initialize_server_fn_map!`, populated via `inventory` at \
             link time) before re-deriving it from scratch.",
            kyomi_ui::SERVER_FN_REGISTRY_FLOOR
        );
        return ExitCode::FAILURE;
    }

    println!(
        "server_fn_count: {count} entries registered (floor {})",
        kyomi_ui::SERVER_FN_REGISTRY_FLOOR
    );
    ExitCode::SUCCESS
}
