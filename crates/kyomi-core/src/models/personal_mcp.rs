// SPDX-License-Identifier: AGPL-3.0-or-later

//! Personal MCP rows contain no plaintext or encrypted credentials.
#[derive(Clone, Debug, sqlx::FromRow)]
pub struct UserMcpConnection {
    pub connection_id: String,
    pub owner_user_id: String,
    pub name: String,
    pub endpoint: String,
    pub auth_kind: String,
    pub status: String,
    pub generation: i64,
}
