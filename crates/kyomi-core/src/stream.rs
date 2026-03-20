// SPDX-License-Identifier: AGPL-3.0-or-later

//! Streaming types — re-exported from kyomi-connect-protocol.
pub use kyomi_connect_protocol::stream::{ColumnInfo, QueryStreamEvent, SimpleType};

/// QueryStream using kyomi-core's Error type (for monorepo compatibility).
///
/// The monorepo's `QueryStream` uses `kyomi_core::Error` in the `Stream::Item`,
/// while `kyomi_connect_protocol::QueryStream` uses the protocol crate's lighter
/// `Error`. We keep the monorepo version here so all existing provider code and
/// consumers continue to work unchanged.
pub type QueryStream =
    std::pin::Pin<Box<dyn futures_util::Stream<Item = crate::Result<QueryStreamEvent>> + Send>>;
