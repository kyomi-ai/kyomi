// SPDX-License-Identifier: AGPL-3.0-or-later

//! Theme synchronization — delegates to mcp_transport::apply_theme.
//!
//! Kept as a thin re-export for backwards compatibility with other modules.

pub use crate::mcp_transport::apply_theme;
