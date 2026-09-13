// SPDX-License-Identifier: AGPL-3.0-or-later

//! Tests for `config.rs`, one file per topic.
//!
//! `config.rs` gains a field on most weeks, so a single `mod tests { … }` at
//! its tail would serialise every concurrently-open PR that adds a test on the
//! same closing brace — see `docs/standards/code-organization/`'s
//! one-test-topic-per-file rule. Shared fixtures and the `include_str!`
//! constant live here; each topic gets its own file.

mod smtp_settings;

/// `config.rs`'s own source, for the guard tests that pin which call sites
/// route through the shared predicate rather than re-spelling it.
///
/// Production code only: this test module lives in its own file, so the
/// `include_str!` does not pull in the tests' own literals and no marker-based
/// compensation is needed.
const SRC: &str = include_str!("../../config.rs");
