// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared Leptos UI components — usable in both the main kyomi-ui crate
//! (SSR + hydrate) and the standalone mcp-chart-app-wasm (CSR).
//!
//! No server-side dependencies. All components compile to WASM without
//! feature flags.

pub mod components;

pub use components::*;
