//! KYO-184 compile-time sanity checks. This file is a Leptos view tree —
//! its reactive branching can't be exercised as a plain unit test — so,
//! following the precedent in `pages/settings/profile.rs`
//! (`tests_part3`), these assert against the source text itself. Each
//! assertion locks in one specific piece of wiring; if it fails, that
//! wiring has regressed.
//!
//! KYO-455 split this module out of `datasources.rs` into one file per
//! test topic (this file holds only the shared helpers/markers) so two
//! PRs adding tests on different topics touch different files instead of
//! both appending to the same tail — see
//! `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`.
//! Add a new test to the topic file it belongs with; add a new topic
//! file (and declare it below) only when no existing one fits.

/// Returns the source slice from the first occurrence of `start` up to
/// (but not including) the first occurrence of `end` that follows it.
/// Panics with a clear message if either marker is missing — a missing
/// marker means the code it was anchoring has been renamed or removed.
fn extract_between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
    let start_pos = src
        .find(start)
        .unwrap_or_else(|| panic!("marker not found in datasources.rs: {start:?}"));
    let end_pos = src[start_pos..]
        .find(end)
        .map(|i| start_pos + i)
        .unwrap_or_else(|| panic!("end marker not found after {start:?} in datasources.rs: {end:?}"));
    &src[start_pos..end_pos]
}

/// True when `needle` appears somewhere in the `window` characters
/// immediately preceding the first occurrence of `anchor` in `haystack`.
/// Whitespace/indentation-agnostic, unlike a plain substring match on
/// the two markers concatenated verbatim.
fn appears_shortly_before(haystack: &str, needle: &str, anchor: &str, window: usize) -> bool {
    let Some(anchor_pos) = haystack.find(anchor) else {
        return false;
    };
    let start = anchor_pos.saturating_sub(window);
    haystack[start..anchor_pos].contains(needle)
}

/// The full source of `datasources.rs` — production code only. Before
/// KYO-455 this module lived inline at the bottom of `datasources.rs`
/// itself, so this same `include_str!` also pulled in the tests' own
/// source text, and several assertions had to scope themselves around
/// that (slicing at `MOD_TESTS_MARKER`, or bounding to a `<Show>` block
/// known to sit above the old inline module) to avoid matching their own
/// literals. Now that the tests live in sibling files under `tests/`,
/// this constant contains production code exclusively — no such scoping
/// is needed for a whole-file search, though `MOD_TESTS_MARKER` below is
/// still useful as an explicit "end of production code" boundary for
/// `extract_between`.
const SRC: &str = include_str!("../../datasources.rs");

/// The `#[cfg(...)]`/`mod tests;` declaration that ends `datasources.rs`.
/// Useful as an explicit right-hand boundary for `extract_between` calls
/// that need "from some marker to the end of production code" (e.g. "the
/// last function in the file") without relying on `SRC`'s own EOF, which
/// would silently widen its extraction if a new item were ever appended
/// after that function.
const MOD_TESTS_MARKER: &str = "#[cfg(all(test, feature = \"ssr\"))]\nmod tests;";

mod save_actions;
mod auth_mode_sections;
mod oauth;
mod credential_state_reset;
mod create_mode;
mod catalog;
mod list_view;
mod connection_test_badge;
mod synapse_connection_config;
mod synapse_tenant_id_credentials;
mod list_connect_gate;
