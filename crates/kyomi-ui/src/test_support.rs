// SPDX-License-Identifier: AGPL-3.0-or-later

//! Crate-local test-only support for this crate's source-assertion tests.
//!
//! Leptos view trees can't be exercised as plain unit tests, so much of
//! this crate's wiring is pinned by tests that `include_str!` a file into a
//! `SRC` constant, slice a window out of it, and assert against what that
//! window contains — see
//! `docs/standards/testing/anchor-source-text-markers-on-code-not-copy.md`
//! for how to choose the markers that bound such a window.
//!
//! [`extract_between`] is the slicing primitive those tests share.
//!
//! ## Provenance (KYO-272)
//!
//! Ten private copies of this function accumulated across the crate before
//! this module existed — in `components/feedback_modal.rs`,
//! `pages/accept_invite.rs`, `pages/accept_ownership.rs`,
//! `pages/auth/login.rs`, `pages/onboarding/datasource_onboarding.rs`,
//! `pages/settings/analytics.rs`,
//! `pages/settings/datasources/tests/mod.rs`, `server_fns/analytics.rs`,
//! `server_fns/workspace.rs` and `utils/beta_access.rs`. The last of those
//! carried a comment predicting that "a fourth copy would be the trigger to
//! actually extract a shared test-support crate"; by then there were
//! already more than four, which is precisely the aggregate-invisibility
//! failure that
//! `docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md`
//! describes.
//!
//! The ten copies were **not** all identical. They fell into two groups
//! that returned *different slices* for the same arguments:
//!
//! - Five (`feedback_modal.rs`, `pages/settings/analytics.rs`,
//!   `pages/settings/datasources/tests/mod.rs`, `server_fns/analytics.rs`,
//!   `server_fns/workspace.rs`) returned `src[start_pos..end_pos]` — the
//!   slice **including** the `start` marker, with `end` searched from
//!   `start_pos`.
//! - Five (`accept_invite.rs`, `accept_ownership.rs`, `auth/login.rs`,
//!   `onboarding/datasource_onboarding.rs`, `utils/beta_access.rs`)
//!   returned the slice **excluding** the `start` marker, with `end`
//!   searched from the first byte after it.
//!
//! This module keeps the first (start-inclusive) behaviour, because that is
//! the contract the standards doc above already writes down as this
//! codebase's convention (`src.find(start)`, then `src[start_pos..]
//! .find(end)`), and because it backed roughly 159 of the 169 call sites.
//! Every one of the ten call sites belonging to the start-exclusive group
//! was checked individually before being moved onto this definition: at all
//! ten the `end` marker resolves to the same byte offset either way, so the
//! only difference is the `start` marker text now sitting at the front of
//! the window, and no asserted needle — positive or negative — occurs
//! inside any of those `start` markers. No assertion changed outcome.
//!
//! ## Gating
//!
//! Plain `#[cfg(test)]`, deliberately *weaker* than the
//! `#[cfg(all(test, feature = "ssr"))]` used by
//! `server_fns/test_support.rs`. The consumers are split: six test modules
//! are plain `#[cfg(test)]` and four are
//! `#[cfg(all(test, feature = "ssr"))]`. `cfg(test)` is implied by both, so
//! this gate satisfies every consumer; gating on `ssr` instead would break
//! `cargo test -p kyomi-ui --lib` (no `--features ssr`), where the six
//! plain-`cfg(test)` modules still compile and still need this function.

/// Returns the slice of `src` running from the first occurrence of `start`
/// up to (but not including) the first occurrence of `end` at or after that
/// point.
///
/// # The returned slice includes the `start` marker text itself
///
/// This is the subtlety that matters most at the call site, and it is not
/// obvious from the name. The window begins *at* `start`, not after it, so
/// the `start` marker's own characters are part of what you then assert
/// against.
///
/// Two consequences:
///
/// - A **positive** assertion (`window.contains(needle)`) passes trivially
///   if `needle` also appears inside `start`. The test then proves nothing
///   about the code the window was meant to cover, and cannot fail. This is
///   the dangerous direction: it fails silently, by succeeding.
/// - A **negative** assertion (`!window.contains(needle)`) fails
///   spuriously if `needle` appears inside `start`. This one is loud, so it
///   gets caught.
///
/// Either way, check your needle against your marker. KYO-260 shipped a
/// defect of this family from the mirror-image direction — an assertion
/// that could never *pass*, because the marker's own letters were inside
/// the window it was scanning.
///
/// When you need the text strictly *after* the marker, make `start` the
/// full text you want excluded and assert accordingly; do not assume this
/// function trims it.
///
/// # Matching is leftmost, not unique
///
/// `start` resolves to its *first* occurrence in `src`, and `end` to the
/// first occurrence at or after `start`'s beginning — not the first after
/// the marker ends, so an `end` that also occurs inside `start` yields an
/// empty window. Neither marker is required to be unique; correctness rests
/// on the real definition coming first in the file. See
/// `docs/standards/testing/anchor-source-text-markers-on-code-not-copy.md`.
///
/// # Panics
///
/// Panics if either marker is absent. That is a `panic!`, not an assertion
/// failure: a missing marker means the code it anchored was renamed,
/// reformatted or removed, so the test never reaches its assertion. The
/// function is `#[track_caller]`, so the panic is reported at the calling
/// test's line rather than here — which is what the ten private copies were
/// hard-coding their own file names into the message to achieve.
#[track_caller]
pub(crate) fn extract_between<'a>(src: &'a str, start: &str, end: &str) -> &'a str {
    let Some(start_pos) = src.find(start) else {
        panic!("start marker not found: {start:?}");
    };
    let Some(offset) = src[start_pos..].find(end) else {
        panic!("end marker not found after {start:?}: {end:?}");
    };
    &src[start_pos..start_pos + offset]
}

mod tests {
    use super::extract_between;

    /// The property the doc comment leads with, and the one KYO-272 had to
    /// verify at ten call sites before unifying them: the window starts
    /// *at* the marker, not after it.
    #[test]
    fn returned_slice_includes_the_start_marker() {
        assert_eq!(extract_between("aSTARTbENDc", "START", "END"), "STARTb");
    }

    /// `end` is searched from `start`'s beginning, so an `end` that occurs
    /// inside `start` yields an empty window rather than skipping past it.
    #[test]
    fn end_marker_is_searched_from_the_start_of_the_marker() {
        assert_eq!(extract_between("xxABCyy", "ABC", "B"), "A");
    }

    /// Leftmost match, not unique match — the first occurrence wins even
    /// when the marker appears repeatedly.
    #[test]
    fn markers_resolve_to_their_leftmost_occurrence() {
        assert_eq!(extract_between("M1|M2|E|E", "M", "E"), "M1|M2|");
    }

    #[test]
    #[should_panic(expected = "start marker not found")]
    fn missing_start_marker_panics() {
        extract_between("abc", "nope", "c");
    }

    #[test]
    #[should_panic(expected = "end marker not found")]
    fn missing_end_marker_panics() {
        extract_between("abc", "a", "nope");
    }
}
