// SPDX-License-Identifier: AGPL-3.0-or-later

//! Re-export ConnectRegistry from kyomi-datasource.
//!
//! The canonical implementation lives in `kyomi_datasource_server::connect::registry`.
//! This module re-exports it for backwards compatibility within kyomi-api.

pub use kyomi_datasource_server::connect::registry::*;
