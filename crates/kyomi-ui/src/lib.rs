// SPDX-License-Identifier: AGPL-3.0-or-later
#![recursion_limit = "512"]

//! kyomi-ui — Leptos frontend for Kyomi.
//!
//! This crate contains the Leptos components and server functions that
//! progressively replace the React frontend. Both SSR (server) and
//! hydrate (WASM) targets are supported via feature flags.

pub mod app;
pub mod cache;
pub mod chartml_provider;
pub mod components;
pub mod pages;
pub mod parser;
pub mod query_cache;
pub mod server_fns;
pub mod types;
pub mod utils;

#[cfg(target_arch = "wasm32")]
mod wasm_math_shims;

#[cfg(target_arch = "wasm32")]
pub mod panic_overlay;

#[cfg(target_arch = "wasm32")]
pub mod arrow_fetch;

pub use app::App;

// KYO-191: server functions self-register with `server_fn`'s Axum registry
// via `inventory` — the `#[server]` macro emits an `inventory::submit!` for
// every function, and `server_fn::axum::server_fn_paths()` reads that
// registry lazily on first access (see `initialize_server_fn_map!` in
// `server_fn::axum`). Explicit `register_explicit::<T>()` calls (formerly
// 195 of them, in a `register_server_functions()` here) were deleted after
// measuring, under the real production build profile (`lto = true`,
// `codegen-units = 1`, `strip = true`, `panic = "abort"`), that they added
// zero functions beyond what `inventory` already registered on its own.
// `server_fn`'s own docs say explicit registration is only needed for a WASM
// server target or an environment `inventory` can't instrument — neither
// applies to this binary. See `server_fn_registry_is_populated_via_inventory`
// below for the regression guard that replaced it.
#[cfg(all(test, feature = "ssr"))]
mod server_fn_registration_tests {
    //! KYO-191 deleted `register_server_functions()` (195 explicit
    //! `register_explicit::<T>()` calls) after measuring that `inventory`'s
    //! link-section statics already register every server function on their
    //! own — explicit registration was a no-op under the production build
    //! profile. Deleting that belt-and-braces mechanism removes the one
    //! thing that used to guarantee (by construction) that a function
    //! existed in the registry. Without a replacement, a broken registry —
    //! e.g. from a future toolchain/linker change, or a `#[server]` fn
    //! defined somewhere `inventory`'s macro can't see — would silently
    //! manifest as every server function 404ing at runtime, with nothing at
    //! boot to explain why.
    //!
    //! This test is that replacement. It asserts the registry is populated
    //! with a plausible number of entries, and spot-checks several of the
    //! 12 functions the ticket found missing from the old explicit list
    //! (`logout`, `generate_ssh_key`, `get_catalog_refresh_status`) so a
    //! regression in exactly the class of function this ticket was worried
    //! about would fail loudly here instead of 404ing in production.

    /// Registered paths carry a numeric hash suffix appended directly after
    /// the function name with no separator (see
    /// `server_fn_macro::ServerFnCall::server_fn_url`, which concatenates
    /// prefix + fn name + hash). That means `/leptos-api/logout` is a
    /// textual prefix of `/leptos-api/logout_all_sessions`, so a naive
    /// `starts_with` check on `"/leptos-api/logout"` would pass even if only
    /// `logout_all_sessions` were ever registered and plain `logout` were
    /// missing. Require that the byte immediately following the function
    /// name is an ASCII digit (the start of the hash) rather than another
    /// identifier character, so the two can't be confused for each other.
    fn has_registered_path_for(paths: &[String], leptos_api_fn_name: &str) -> bool {
        let prefix = format!("/leptos-api/{leptos_api_fn_name}");
        paths.iter().any(|path| {
            path.strip_prefix(prefix.as_str())
                .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
        })
    }

    #[test]
    fn server_fn_registry_is_populated_via_inventory() {
        let paths: Vec<String> = leptos::server_fn::axum::server_fn_paths()
            .map(|(path, _method)| path.to_string())
            .collect();

        // A floor, not an exact expectation: KYO-191 measured 206 registered
        // functions under the production profile at the time this test was
        // written. Hardcoding 206 would fail this test the next time anyone
        // adds a server function; 150 is comfortably below that while still
        // catching "the registry came back empty or near-empty."
        assert!(
            paths.len() >= 150,
            "expected the inventory-populated server_fn registry to contain \
             at least 150 entries (a floor, not the exact count — KYO-191 \
             measured 206 under the production profile), got {}. A \
             collapsed registry means every server function 404s at \
             runtime with no signal at boot.",
            paths.len()
        );

        // Spot-check functions KYO-191 found missing from the old explicit
        // registration list, to prove `inventory` covers exactly the case
        // that list was added to guard against.
        for fn_name in ["logout", "generate_ssh_key", "get_catalog_refresh_status"] {
            assert!(
                has_registered_path_for(&paths, fn_name),
                "expected a registered `/leptos-api/{fn_name}...` path — this \
                 was one of the 12 functions KYO-191 found missing from the \
                 (now-deleted) explicit registration list"
            );
        }

        // `logout` and `logout_all_sessions` share a textual prefix; assert
        // the longer one is independently registered so the check above
        // can't have silently passed on `logout_all_sessions` alone.
        assert!(
            has_registered_path_for(&paths, "logout_all_sessions"),
            "expected `logout_all_sessions` to be registered independently \
             of `logout`"
        );
    }
}

