// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database models (sqlx `FromRow` structs) matching the Python SQLAlchemy schema.
//!
//! These are read/write structs for the core auth tables, workspace tables,
//! and supporting models (invitations, transfers, API tokens).
//! Field names and types match the PostgreSQL columns exactly.

pub mod agent_learning;
pub mod api_token;
pub mod api_usage_log;
pub mod auth_method;
pub mod chart;
pub mod chat_message;
pub mod collection;
pub mod chat_session;
pub mod conversation_read_status;
pub mod dashboard;
pub mod datasource;
pub mod email_subscriber;
pub mod feedback;
pub mod oauth_client;
pub mod ownership_transfer;
pub mod push_subscription;
pub mod query_cache;
pub mod query_search_embedding;
pub mod refresh_token;
pub mod search_embedding;
pub mod sql_query_history;
pub mod table_cache;
pub mod user;
pub mod verification_token;
pub mod watch;
pub mod workspace;
pub mod workspace_invitation;

pub use agent_learning::AgentLearning;
pub use api_token::ApiToken;
pub use api_usage_log::ApiUsageLog;
pub use auth_method::UserAuthMethod;
pub use chart::Chart;
pub use collection::{Collection, CollectionDashboard};
pub use chat_message::ChatMessage;
pub use chat_session::ChatSession;
pub use conversation_read_status::ConversationReadStatus;
pub use dashboard::{Dashboard, DashboardVersion, DashboardView};
pub use datasource::{DatasourceConfig, UserDatasourceCredential, UserDatasourcePreference};
pub use email_subscriber::EmailSubscriber;
pub use feedback::Feedback;
pub use oauth_client::OAuthClient;
pub use ownership_transfer::OwnershipTransfer;
pub use push_subscription::PushSubscription;
pub use query_cache::QueryCache;
pub use refresh_token::RefreshToken;
pub use sql_query_history::SqlQueryHistory;
pub use user::User;
pub use verification_token::VerificationToken;
pub use watch::{Watch, WatchExecution};
pub use workspace::{Workspace, WorkspaceUser};
pub use workspace_invitation::WorkspaceInvitation;
