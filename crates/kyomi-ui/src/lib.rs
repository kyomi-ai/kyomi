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
pub mod wasm_logging;

/// Crate-local test-only support shared by this crate's source-assertion
/// tests — see the module doc comment in `test_support.rs` (KYO-272).
///
/// Gated on plain `#[cfg(test)]` rather than
/// `#[cfg(all(test, feature = "ssr"))]` because its consumers are split
/// across both gates; `cfg(test)` is the only one implied by all of them.
#[cfg(test)]
pub(crate) mod test_support;

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

// KYO-281: `Button` (crates/kyomi-ui-components/src/components/button.rs)
// deliberately defaults to `type="button"` so buttons don't accidentally
// submit a form. That default has a sharp edge: a `<Button>` used as the
// *intended* submit control of a `<form on:submit=...>` silently never
// submits either — the `on:submit` handler never fires, no request is
// sent, and the page looks completely normal; the only way to notice is
// capturing network traffic. Three instances of exactly this bug shipped
// (`passkey_signup_complete.rs`, `account_recovery_complete.rs`,
// `connect_setup_page.rs`) before it was caught. This test walks every
// page file at runtime so a fourth instance can't ship silently.
//
// This module deliberately lives here rather than under `src/pages`
// (e.g. `pages/mod.rs`) even though it only inspects that directory: the
// KYO-281 explanatory comments this fix added near the two Button call
// sites quote the literal string `button_type="submit"` in prose, and an
// earlier draft of this test lived inside `pages/mod.rs` — its own
// doc comments quoting `<form on:submit=...>` were then walked as if
// they were real markup, tripping the "no matching `</form>`" panic on
// itself. Living outside `src/pages` sidesteps that self-reference risk
// entirely rather than special-casing it.
#[cfg(all(test, feature = "ssr"))]
mod submit_button_guard_tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    /// Recursively collect every `.rs` file under `dir`. This walks the
    /// filesystem at runtime (rather than `include_str!` on a hardcoded
    /// file list) so pages added after this test was written are covered
    /// automatically instead of silently skipped.
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("failed to read directory {}: {e}", dir.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|e| {
                panic!("failed to read a directory entry under {}: {e}", dir.display())
            });
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Byte offset just past the `>` that closes the `<form ...>` opening
    /// tag starting at `form_start`. None of the form tags in this
    /// codebase put a literal `>` inside an attribute value, so a plain
    /// search for the next `>` is sufficient to find the tag boundary.
    fn form_open_tag_end(content: &str, form_start: usize) -> usize {
        let rel = content[form_start..].find('>').unwrap_or_else(|| {
            panic!("found `<form` at byte {form_start} with no closing `>` on its opening tag")
        });
        form_start + rel + 1
    }

    /// Strip `//` line comments, but only outside string literals. A naive
    /// "cut at the first `//`" strip would truncate lines that contain a
    /// URL string (e.g. `href="https://kyomi.ai/terms"`, which appears
    /// inside two of the forms this test checks). This matters because the
    /// KYO-281 fix's own explanatory comments quote the literal substring
    /// `button_type="submit"` in prose — without stripping comments first,
    /// a form whose Button lost its `button_type="submit"` attribute but
    /// kept the nearby comment would still read as "has a submit trigger"
    /// to a plain substring search, which defeats the point of this test.
    /// This does not handle raw strings (`r"..."`, `r#"..."#`) — none of
    /// the page markup this test inspects uses them.
    fn strip_line_comments(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for line in text.lines() {
            let bytes = line.as_bytes();
            let mut in_string = false;
            let mut i = 0;
            let mut cut = line.len();
            while i < bytes.len() {
                match bytes[i] {
                    b'\\' if in_string => {
                        i += 2; // skip the escaped character
                        continue;
                    }
                    b'"' => in_string = !in_string,
                    b'/' if !in_string && i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                        cut = i;
                        break;
                    }
                    _ => {}
                }
                i += 1;
            }
            out.push_str(&line[..cut]);
            out.push('\n');
        }
        out
    }

    #[test]
    fn every_form_with_on_submit_has_a_submit_trigger() {
        // Cargo runs test binaries with cwd set to the package's manifest
        // directory (crates/kyomi-ui), so this relative path is stable
        // regardless of where `cargo test` is invoked from.
        let pages_dir = Path::new("src/pages");
        assert!(
            pages_dir.is_dir(),
            "expected `{}` to exist and be a directory relative to the crate root — if this \
             fails, cargo's test-binary cwd assumption this test relies on has changed",
            pages_dir.display()
        );

        let mut files = Vec::new();
        collect_rs_files(pages_dir, &mut files);
        assert!(
            !files.is_empty(),
            "found zero `.rs` files under `{}` — the directory walk is broken",
            pages_dir.display()
        );

        let mut checked_forms = 0usize;

        for file in &files {
            let content = fs::read_to_string(file)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", file.display()));
            let content_code_only = strip_line_comments(&content);

            let mut search_from = 0;
            while let Some(rel) = content[search_from..].find("<form") {
                let form_start = search_from + rel;
                let open_tag_end = form_open_tag_end(&content, form_start);
                let open_tag = &content[form_start..open_tag_end];

                if !open_tag.contains("on:submit=") {
                    // A <form> with no on:submit handler has nothing for
                    // this test to check — skip past it.
                    search_from = open_tag_end;
                    continue;
                }

                // Find the matching close tag. This is a substring
                // scanner, not a parser: if another `<form` opens before
                // this one's `</form>`, it's a nested form this scanner
                // can't reason about correctly — fail loudly rather than
                // silently checking the wrong span.
                let next_open_rel = content[open_tag_end..].find("<form");
                let next_close_rel = content[open_tag_end..].find("</form>");

                let close_rel = match (next_open_rel, next_close_rel) {
                    (_, None) => panic!(
                        "{}: `<form on:submit=` at byte {form_start} has no matching \
                         `</form>` — cannot verify it has a submit trigger",
                        file.display()
                    ),
                    (Some(open_rel), Some(close_rel)) if open_rel < close_rel => panic!(
                        "{}: a `<form` opens at byte {} before the `</form>` matching the \
                         `<form on:submit=` at byte {form_start} — this KYO-281 guard test \
                         cannot check nested forms correctly; restructure the markup or \
                         extend the test",
                        file.display(),
                        open_tag_end + open_rel
                    ),
                    (_, Some(close_rel)) => close_rel,
                };

                let form_end = open_tag_end + close_rel + "</form>".len();
                let slice_code_only = strip_line_comments(&content[form_start..form_end]);
                checked_forms += 1;

                // Every real submit-trigger spelling in this codebase
                // contains the substring `type="submit"`:
                //   <Button ... button_type="submit">   (shared component)
                //   <button ... type="submit">          (native button)
                //   <button ... attr:r#type="submit">   (native, prop-set)
                let has_submit_trigger = slice_code_only.contains(r#"type="submit""#);

                if !has_submit_trigger {
                    // Documented allowance: a form's submit control can
                    // legitimately live *outside* the <form> and trigger
                    // it via `HtmlFormElement::request_submit()` from an
                    // `on:click` handler elsewhere in the component — see
                    // `pages/dashboards/collections_sidebar.rs`, where the
                    // Modal footer's button looks up the form by id and
                    // calls `form.request_submit()`. The allowance is
                    // keyed on that call existing (as real code, not just
                    // a comment) anywhere in the same file, not on the
                    // filename: any future form using this pattern is
                    // automatically covered, and a form in this same file
                    // that *doesn't* wire up `request_submit()` still
                    // fails loudly below.
                    assert!(
                        content_code_only.contains("request_submit()"),
                        "{}: `<form on:submit=` at byte {form_start} has no submit trigger \
                         inside it — no `<Button button_type=\"submit\">` and no native \
                         `<button type=\"submit\">` / `attr:r#type=\"submit\"` — and the \
                         file has no `request_submit()` call either. `Button` \
                         (crates/kyomi-ui-components/src/components/button.rs) defaults to \
                         `type=\"button\"` so this form will silently never submit: the \
                         on:submit handler never fires, no request is sent, and nothing in \
                         the UI shows a problem (KYO-281).",
                        file.display()
                    );
                }

                search_from = form_end;
            }
        }

        // A floor, not an exact count: 11 `<form on:submit=` blocks existed
        // under src/pages when this test was written. Hardcoding 11 would
        // fail the next time anyone adds a form; 10 stays comfortably
        // below that while still catching "the walk found nothing."
        assert!(
            checked_forms >= 10,
            "expected to find at least 10 `<form on:submit=` blocks under src/pages, found \
             {checked_forms} — the directory walk or the `<form` scanner may be broken"
        );
    }
}

