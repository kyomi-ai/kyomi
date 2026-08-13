// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Build script for `kyomi-slack`.
//!
//! `src/routes.rs` and `src/alert.rs` each expand
//! `sqlx::migrate!("../../apps/server/migrations-sqlite")` at compile time.
//! That macro embeds the migration files that exist *when the macro
//! expands* by generating `include_bytes!` references to each one — so
//! editing an existing `.sql` file marks the crate dirty and triggers a
//! rebuild, but *adding* or *removing* a migration file touches nothing the
//! macro referenced, so Cargo sees no changed input and skips the rebuild.
//! Any test that reaches a DB through this crate then silently runs against
//! the previous migration chain and reports green (KYO-343; observed
//! concretely on KYO-293, where tests written for the post-`00033` schema
//! passed against a schema whose `_sqlx_migrations` table only went up to
//! version 32).
//!
//! This script re-runs the crate's build whenever the migrations directory's
//! contents change, closing that gap. It also fails the build loudly if the
//! directory `sqlx::migrate!` depends on does not exist, so a future
//! move/rename of the migrations directory breaks the build immediately
//! instead of degrading back into silent staleness.

use std::path::{Path, PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set"),
    );

    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("build.rs").display()
    );

    let migrations_sqlite = manifest_dir
        .join("..")
        .join("..")
        .join("apps")
        .join("server")
        .join("migrations-sqlite");
    rerun_if_migrations_dir_changed("kyomi-slack", &migrations_sqlite);
}

/// Emits `cargo:rerun-if-changed` for `dir`, panicking if it does not exist.
///
/// `crate_name`'s `sqlx::migrate!` call sites depend on `dir` existing at
/// this resolved path; if it doesn't, the crate's migration coverage is
/// silently missing, which is exactly the failure mode KYO-343 exists to
/// prevent.
fn rerun_if_migrations_dir_changed(crate_name: &str, dir: &Path) {
    if !dir.is_dir() {
        panic!(
            "{crate_name} build.rs: migrations directory {} does not exist, but a \
             `sqlx::migrate!` call site in this crate depends on it. If the migrations \
             directory was moved or renamed, update both the `sqlx::migrate!` path(s) and \
             this build script.",
            dir.display()
        );
    }
    println!("cargo:rerun-if-changed={}", dir.display());
}
