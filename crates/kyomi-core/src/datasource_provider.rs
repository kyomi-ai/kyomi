// SPDX-License-Identifier: AGPL-3.0-or-later

//! Datasource provider trait — **DEPRECATED** in favour of `kyomi_datasource::DatasourceProvider`.
//!
//! The minimal trait defined here in Phase 5 has been superseded by the
//! expanded trait in the `kyomi-datasource` crate (Phase 6A), which adds
//! `execute_query`, `dry_run`, and richer result types.
//!
//! This module is kept for backward compatibility. New code should import
//! from `kyomi_datasource` directly.

// This module is intentionally empty. The expanded DatasourceProvider trait,
// result types, and all provider implementations live in the kyomi-datasource
// crate. Keeping this module (rather than removing it) avoids breaking any
// existing `use kyomi_core::datasource_provider` imports during the transition.
