// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi Connect — provider and registry for routing queries through
//! customer-deployed Connect instances via WebSocket.
//!
//! - [`registry`] — Maps `datasource_config_id` to active WebSocket connections
//! - [`provider`] — `DatasourceProvider` implementation that routes through the registry

pub mod provider;
pub mod registry;
