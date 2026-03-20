// SPDX-License-Identifier: AGPL-3.0-or-later

//! Unified WebSocket manager with Redis pub/sub for multi-replica delivery.
//!
//! - `manager.rs`: `WebSocketManager` — connection tracking, Redis pub/sub
//! - `helpers.rs`: convenience functions for sending typed messages

pub mod helpers;
pub mod manager;

pub use manager::WebSocketManager;
