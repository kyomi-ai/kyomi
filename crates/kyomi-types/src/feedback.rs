// SPDX-License-Identifier: AGPL-3.0-or-later

//! Canonical feedback-type values.
//!
//! Shared between the UI's Type selector
//! (`kyomi_ui::components::feedback_modal`, which additionally carries
//! per-type display metadata — label and icon — that has no server-side
//! meaning and stays UI-side) and the server's validation allowlist
//! (`kyomi_auth::feedback_service::submit_feedback`), so the two lists
//! cannot silently drift apart. Both crates already depend on
//! `kyomi-types` unconditionally (`kyomi-ui` is not `ssr`-gated on it),
//! so sharing this list costs no new cross-crate dependency.

/// All feedback types the server accepts in a feedback submission.
///
/// `access_request` (KYO-417) is validated identically to the other
/// three — the UI restricting when it appears in the Type selector is a
/// UX nicety, not a security boundary, so the server does not gate on
/// context. See `kyomi_ui::components::feedback_modal` for the
/// visibility rule.
pub const FEEDBACK_TYPE_VALUES: &[&str] = &["bug", "feature", "question", "access_request"];
