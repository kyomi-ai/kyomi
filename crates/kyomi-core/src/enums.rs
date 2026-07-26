// SPDX-License-Identifier: AGPL-3.0-or-later

//! Type-safe enums for all enum-like database columns.
//!
//! PostgreSQL has one true enum type (`learning_scope`), which uses
//! `#[sqlx(type_name = "learning_scope")]`. All other "enum" columns are
//! VARCHAR/TEXT storing snake_case strings.
//!
//! VARCHAR enums use manual `sqlx::Type + Encode + Decode` implementations
//! (via `impl_sqlx_varchar_enum!`) that delegate to `String`, which is
//! compatible with both PostgreSQL `TEXT` and `VARCHAR` column types.
//!
//! Every enum also derives `Serialize`/`Deserialize` with `rename_all = "snake_case"`
//! for identical JSON wire format (no API changes).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Implements `sqlx::Type`, `sqlx::Encode`, and `sqlx::Decode` for a
/// VARCHAR enum by delegating to `String`, for both Postgres and SQLite.
///
/// This works with both `TEXT` and `VARCHAR` columns because
/// `String::compatible()` accepts both OIDs (Postgres) and SQLite stores
/// all enum values as TEXT natively.
///
/// Requires: `AsRef<str>` + `FromStr` on the enum.
macro_rules! impl_sqlx_varchar_enum {
    ($enum_type:ty) => {
        // ── Postgres ─────────────────────────────────────────────
        impl sqlx::Type<sqlx::Postgres> for $enum_type {
            fn type_info() -> sqlx::postgres::PgTypeInfo {
                <String as sqlx::Type<sqlx::Postgres>>::type_info()
            }
            fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Postgres>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Postgres> for $enum_type {
            fn encode_by_ref(
                &self,
                buf: &mut sqlx::postgres::PgArgumentBuffer,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>>
            {
                <&str as sqlx::Encode<'q, sqlx::Postgres>>::encode_by_ref(&self.as_ref(), buf)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Postgres> for $enum_type {
            fn decode(
                value: sqlx::postgres::PgValueRef<'r>,
            ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                let s = <&str as sqlx::Decode<'r, sqlx::Postgres>>::decode(value)?;
                s.parse::<Self>().map_err(|e| e.into())
            }
        }

        // ── SQLite ───────────────────────────────────────────────
        impl sqlx::Type<sqlx::Sqlite> for $enum_type {
            fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
                <String as sqlx::Type<sqlx::Sqlite>>::type_info()
            }
            fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
                <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
            }
        }

        impl<'q> sqlx::Encode<'q, sqlx::Sqlite> for $enum_type {
            fn encode_by_ref(
                &self,
                args: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>,
            ) -> Result<sqlx::encode::IsNull, Box<dyn std::error::Error + Send + Sync>>
            {
                let s = self.as_ref().to_owned();
                <String as sqlx::Encode<'q, sqlx::Sqlite>>::encode_by_ref(&s, args)
            }
        }

        impl<'r> sqlx::Decode<'r, sqlx::Sqlite> for $enum_type {
            fn decode(
                value: sqlx::sqlite::SqliteValueRef<'r>,
            ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
                let s = <&str as sqlx::Decode<'r, sqlx::Sqlite>>::decode(value)?;
                s.parse::<Self>().map_err(|e| e.into())
            }
        }
    };
}

// ─── Learning scope (true PG enum) ──────────────────────────────────────────

/// Visibility scope for agent learnings.
///
/// This is the **only** PostgreSQL enum type — stored as the custom type
/// `learning_scope` in the DB. All other enums in this file are VARCHAR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "learning_scope", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum LearningScope {
    Workspace,
    User,
}

impl fmt::Display for LearningScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace => write!(f, "workspace"),
            Self::User => write!(f, "user"),
        }
    }
}

impl AsRef<str> for LearningScope {
    fn as_ref(&self) -> &str {
        match self {
            Self::Workspace => "workspace",
            Self::User => "user",
        }
    }
}

// ─── Learning type (VARCHAR) ────────────────────────────────────────────────

/// Classification type for agent learnings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningType {
    Navigation,
    EventContext,
    Preference,
    Metric,
}

impl_sqlx_varchar_enum!(LearningType);

impl fmt::Display for LearningType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for LearningType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Navigation => "navigation",
            Self::EventContext => "event_context",
            Self::Preference => "preference",
            Self::Metric => "metric",
        }
    }
}

impl FromStr for LearningType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "navigation" => Ok(Self::Navigation),
            "event_context" => Ok(Self::EventContext),
            "preference" => Ok(Self::Preference),
            "metric" => Ok(Self::Metric),
            _ => Err(format!("unknown LearningType: {s}")),
        }
    }
}

// ─── Subscription tier (VARCHAR) ────────────────────────────────────────────

/// Workspace subscription tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionTier {
    Free,
    Basic,
    Starter,
    Pro,
    Team,
    Enterprise,
    Cloud,
}

impl_sqlx_varchar_enum!(SubscriptionTier);

impl fmt::Display for SubscriptionTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for SubscriptionTier {
    fn as_ref(&self) -> &str {
        match self {
            Self::Free => "free",
            Self::Basic => "basic",
            Self::Starter => "starter",
            Self::Pro => "pro",
            Self::Team => "team",
            Self::Enterprise => "enterprise",
            Self::Cloud => "cloud",
        }
    }
}

impl FromStr for SubscriptionTier {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "free" => Ok(Self::Free),
            "basic" => Ok(Self::Basic),
            "starter" => Ok(Self::Starter),
            "pro" => Ok(Self::Pro),
            "team" => Ok(Self::Team),
            "enterprise" => Ok(Self::Enterprise),
            "cloud" => Ok(Self::Cloud),
            _ => Err(format!("unknown SubscriptionTier: {s}")),
        }
    }
}

// ─── Subscription status (VARCHAR) ──────────────────────────────────────────

/// Workspace subscription status (Stripe lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionStatus {
    Trialing,
    Active,
    PastDue,
    Cancelled,
}

impl_sqlx_varchar_enum!(SubscriptionStatus);

impl fmt::Display for SubscriptionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for SubscriptionStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Trialing => "trialing",
            Self::Active => "active",
            Self::PastDue => "past_due",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for SubscriptionStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trialing" => Ok(Self::Trialing),
            "active" => Ok(Self::Active),
            "past_due" => Ok(Self::PastDue),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown SubscriptionStatus: {s}")),
        }
    }
}

// ─── Workspace status (VARCHAR) ─────────────────────────────────────────────

/// Workspace lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStatus {
    Trial,
    Active,
    Suspended,
}

impl_sqlx_varchar_enum!(WorkspaceStatus);

impl fmt::Display for WorkspaceStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for WorkspaceStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Trial => "trial",
            Self::Active => "active",
            Self::Suspended => "suspended",
        }
    }
}

impl FromStr for WorkspaceStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trial" => Ok(Self::Trial),
            "active" => Ok(Self::Active),
            "suspended" => Ok(Self::Suspended),
            _ => Err(format!("unknown WorkspaceStatus: {s}")),
        }
    }
}

// ─── Workspace role (VARCHAR) ───────────────────────────────────────────────

/// User role within a workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRole {
    WorkspaceAdmin,
    WorkspaceUser,
}

impl_sqlx_varchar_enum!(WorkspaceRole);

impl fmt::Display for WorkspaceRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for WorkspaceRole {
    fn as_ref(&self) -> &str {
        let roles = &crate::constants::get().workspace.roles;
        match self {
            Self::WorkspaceAdmin => &roles.admin,
            Self::WorkspaceUser => &roles.user,
        }
    }
}

impl FromStr for WorkspaceRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let roles = &crate::constants::get().workspace.roles;
        if s == roles.admin {
            Ok(Self::WorkspaceAdmin)
        } else if s == roles.user {
            Ok(Self::WorkspaceUser)
        } else {
            Err(format!("unknown WorkspaceRole: {s}"))
        }
    }
}

// ─── Watch mode (VARCHAR) ───────────────────────────────────────────────────

/// Watch execution mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    Alert,
    Report,
}

impl_sqlx_varchar_enum!(WatchMode);

impl fmt::Display for WatchMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for WatchMode {
    fn as_ref(&self) -> &str {
        match self {
            Self::Alert => "alert",
            Self::Report => "report",
        }
    }
}

impl FromStr for WatchMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "alert" => Ok(Self::Alert),
            "report" => Ok(Self::Report),
            _ => Err(format!("unknown WatchMode: {s}")),
        }
    }
}

// ─── Watch execution status (VARCHAR) ───────────────────────────────────────

/// Status of a watch execution run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchExecutionStatus {
    Running,
    Success,
    Error,
    NoAlert,
}

impl_sqlx_varchar_enum!(WatchExecutionStatus);

impl fmt::Display for WatchExecutionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for WatchExecutionStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Error => "error",
            Self::NoAlert => "no_alert",
        }
    }
}

impl FromStr for WatchExecutionStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "running" => Ok(Self::Running),
            "success" => Ok(Self::Success),
            "error" => Ok(Self::Error),
            "no_alert" => Ok(Self::NoAlert),
            _ => Err(format!("unknown WatchExecutionStatus: {s}")),
        }
    }
}

// ─── Session type (VARCHAR) ─────────────────────────────────────────────────

/// Chat session type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionType {
    Chat,
    DashboardCopilot,
}

impl_sqlx_varchar_enum!(SessionType);

impl fmt::Display for SessionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for SessionType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Chat => "chat",
            Self::DashboardCopilot => "dashboard_copilot",
        }
    }
}

impl FromStr for SessionType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "chat" => Ok(Self::Chat),
            "dashboard_copilot" => Ok(Self::DashboardCopilot),
            _ => Err(format!("unknown SessionType: {s}")),
        }
    }
}

// ─── Datasource type (VARCHAR) ──────────────────────────────────────────────

/// Supported datasource provider types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DatasourceType {
    Postgres,
    Bigquery,
    Clickhouse,
    Databricks,
    Mysql,
    Redshift,
    Snowflake,
    Sqlserver,
    Synapse,
    Flaredb,
}

impl_sqlx_varchar_enum!(DatasourceType);

impl fmt::Display for DatasourceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for DatasourceType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Postgres => "postgres",
            Self::Bigquery => "bigquery",
            Self::Clickhouse => "clickhouse",
            Self::Databricks => "databricks",
            Self::Mysql => "mysql",
            Self::Redshift => "redshift",
            Self::Snowflake => "snowflake",
            Self::Sqlserver => "sqlserver",
            Self::Synapse => "synapse",
            Self::Flaredb => "flaredb",
        }
    }
}

impl FromStr for DatasourceType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "postgres" => Ok(Self::Postgres),
            "bigquery" => Ok(Self::Bigquery),
            "clickhouse" => Ok(Self::Clickhouse),
            "databricks" => Ok(Self::Databricks),
            "mysql" => Ok(Self::Mysql),
            "redshift" => Ok(Self::Redshift),
            "snowflake" => Ok(Self::Snowflake),
            "sqlserver" => Ok(Self::Sqlserver),
            "synapse" => Ok(Self::Synapse),
            "flaredb" => Ok(Self::Flaredb),
            _ => Err(format!("unknown DatasourceType: {s}")),
        }
    }
}

// ─── DatasourceType ↔ datasource_registry::DatasourceType conversion ───────

impl From<DatasourceType> for crate::datasource_registry::DatasourceType {
    fn from(dt: DatasourceType) -> Self {
        match dt {
            DatasourceType::Postgres => Self::Postgres,
            DatasourceType::Bigquery => Self::BigQuery,
            DatasourceType::Clickhouse => Self::ClickHouse,
            DatasourceType::Databricks => Self::Databricks,
            DatasourceType::Mysql => Self::MySQL,
            DatasourceType::Redshift => Self::Redshift,
            DatasourceType::Snowflake => Self::Snowflake,
            DatasourceType::Sqlserver => Self::SqlServer,
            DatasourceType::Synapse => Self::Synapse,
            DatasourceType::Flaredb => Self::FlareDb,
        }
    }
}

impl From<crate::datasource_registry::DatasourceType> for DatasourceType {
    fn from(dt: crate::datasource_registry::DatasourceType) -> Self {
        match dt {
            crate::datasource_registry::DatasourceType::Postgres => Self::Postgres,
            crate::datasource_registry::DatasourceType::BigQuery => Self::Bigquery,
            crate::datasource_registry::DatasourceType::ClickHouse => Self::Clickhouse,
            crate::datasource_registry::DatasourceType::Databricks => Self::Databricks,
            crate::datasource_registry::DatasourceType::MySQL => Self::Mysql,
            crate::datasource_registry::DatasourceType::Redshift => Self::Redshift,
            crate::datasource_registry::DatasourceType::Snowflake => Self::Snowflake,
            crate::datasource_registry::DatasourceType::SqlServer => Self::Sqlserver,
            crate::datasource_registry::DatasourceType::Synapse => Self::Synapse,
            crate::datasource_registry::DatasourceType::FlareDb => Self::Flaredb,
        }
    }
}

// ─── Feedback type (VARCHAR) ────────────────────────────────────────────────

/// Type of user feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackType {
    Bug,
    Feature,
    Question,
}

impl_sqlx_varchar_enum!(FeedbackType);

impl fmt::Display for FeedbackType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for FeedbackType {
    fn as_ref(&self) -> &str {
        match self {
            Self::Bug => "bug",
            Self::Feature => "feature",
            Self::Question => "question",
        }
    }
}

impl FromStr for FeedbackType {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "bug" => Ok(Self::Bug),
            "feature" => Ok(Self::Feature),
            "question" => Ok(Self::Question),
            _ => Err(format!("unknown FeedbackType: {s}")),
        }
    }
}

// ─── Feedback status (VARCHAR) ──────────────────────────────────────────────

/// Feedback lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackStatus {
    New,
    Reviewed,
    Resolved,
    Closed,
}

impl_sqlx_varchar_enum!(FeedbackStatus);

impl fmt::Display for FeedbackStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for FeedbackStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::New => "new",
            Self::Reviewed => "reviewed",
            Self::Resolved => "resolved",
            Self::Closed => "closed",
        }
    }
}

impl FromStr for FeedbackStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "new" => Ok(Self::New),
            "reviewed" => Ok(Self::Reviewed),
            "resolved" => Ok(Self::Resolved),
            "closed" => Ok(Self::Closed),
            _ => Err(format!("unknown FeedbackStatus: {s}")),
        }
    }
}

// ─── Invitation status (VARCHAR) ────────────────────────────────────────────

/// Workspace invitation lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvitationStatus {
    Pending,
    Accepted,
    Expired,
    Cancelled,
}

impl_sqlx_varchar_enum!(InvitationStatus);

impl fmt::Display for InvitationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for InvitationStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for InvitationStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown InvitationStatus: {s}")),
        }
    }
}

// ─── Transfer status (VARCHAR) ──────────────────────────────────────────────

/// Ownership transfer lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferStatus {
    Pending,
    Accepted,
    Declined,
    Cancelled,
}

impl_sqlx_varchar_enum!(TransferStatus);

impl fmt::Display for TransferStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for TransferStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Pending => "pending",
            Self::Accepted => "accepted",
            Self::Declined => "declined",
            Self::Cancelled => "cancelled",
        }
    }
}

impl FromStr for TransferStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "accepted" => Ok(Self::Accepted),
            "declined" => Ok(Self::Declined),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(format!("unknown TransferStatus: {s}")),
        }
    }
}

// ─── Catalog refresh status (VARCHAR) ───────────────────────────────────────

/// Status of a catalog schema refresh operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogRefreshStatus {
    Idle,
    Running,
    Failed,
}

impl_sqlx_varchar_enum!(CatalogRefreshStatus);

impl fmt::Display for CatalogRefreshStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for CatalogRefreshStatus {
    fn as_ref(&self) -> &str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Failed => "failed",
        }
    }
}

impl FromStr for CatalogRefreshStatus {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "idle" => Ok(Self::Idle),
            "running" => Ok(Self::Running),
            "failed" => Ok(Self::Failed),
            _ => Err(format!("unknown CatalogRefreshStatus: {s}")),
        }
    }
}

// ─── Chat message role (VARCHAR) ────────────────────────────────────────────

/// Role of a chat message sender.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatMessageRole {
    User,
    Assistant,
    System,
    Tool,
}

impl_sqlx_varchar_enum!(ChatMessageRole);

impl fmt::Display for ChatMessageRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_ref())
    }
}

impl AsRef<str> for ChatMessageRole {
    fn as_ref(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
            Self::Tool => "tool",
        }
    }
}

impl FromStr for ChatMessageRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "user" => Ok(Self::User),
            "assistant" => Ok(Self::Assistant),
            "system" => Ok(Self::System),
            "tool" => Ok(Self::Tool),
            _ => Err(format!("unknown ChatMessageRole: {s}")),
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_learning_scope_serde_roundtrip() {
        let scope = LearningScope::Workspace;
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, "\"workspace\"");
        let back: LearningScope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, scope);

        let scope = LearningScope::User;
        let json = serde_json::to_string(&scope).unwrap();
        assert_eq!(json, "\"user\"");
        let back: LearningScope = serde_json::from_str(&json).unwrap();
        assert_eq!(back, scope);
    }

    #[test]
    fn test_learning_type_serde_roundtrip() {
        let lt = LearningType::EventContext;
        let json = serde_json::to_string(&lt).unwrap();
        assert_eq!(json, "\"event_context\"");
        let back: LearningType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lt);
    }

    #[test]
    fn test_subscription_tier_display() {
        assert_eq!(SubscriptionTier::Free.to_string(), "free");
        assert_eq!(SubscriptionTier::Enterprise.to_string(), "enterprise");
    }

    #[test]
    fn test_subscription_status_serde() {
        let s = SubscriptionStatus::PastDue;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"past_due\"");
        let back: SubscriptionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_workspace_role_serde() {
        let r = WorkspaceRole::WorkspaceAdmin;
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"workspace_admin\"");
        let back: WorkspaceRole = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn test_workspace_role_from_str_rejects_removed_viewer_variant() {
        // KYO-183 removed the never-assignable `WorkspaceViewer` variant.
        // A migration backfills any lingering `workspace_viewer` DB rows to
        // `workspace_user` (see apps/server/migrations/20260726010000_...
        // and migrations-sqlite/00030_...) precisely because this parse is
        // now a hard `Err`, not a silent fallback -- this test documents
        // that the break is intentional and proves the migration is
        // load-bearing rather than decorative.
        let _ = crate::constants::load_with_fallback();
        assert!("workspace_viewer".parse::<WorkspaceRole>().is_err());
    }

    #[test]
    fn test_watch_execution_status_serde() {
        let s = WatchExecutionStatus::NoAlert;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"no_alert\"");
        let back: WatchExecutionStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_datasource_type_all_variants() {
        let types = [
            (DatasourceType::Postgres, "postgres"),
            (DatasourceType::Bigquery, "bigquery"),
            (DatasourceType::Clickhouse, "clickhouse"),
            (DatasourceType::Databricks, "databricks"),
            (DatasourceType::Mysql, "mysql"),
            (DatasourceType::Redshift, "redshift"),
            (DatasourceType::Snowflake, "snowflake"),
            (DatasourceType::Sqlserver, "sqlserver"),
            (DatasourceType::Synapse, "synapse"),
            (DatasourceType::Flaredb, "flaredb"),
        ];
        for (variant, expected) in types {
            assert_eq!(variant.as_ref(), expected);
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, format!("\"{expected}\""));
        }
    }

    #[test]
    fn test_chat_message_role_serde() {
        let r = ChatMessageRole::Assistant;
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, "\"assistant\"");
    }

    #[test]
    fn test_session_type_serde() {
        let s = SessionType::DashboardCopilot;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"dashboard_copilot\"");
        let back: SessionType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn test_catalog_refresh_status_serde() {
        let s = CatalogRefreshStatus::Idle;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"idle\"");
    }

    #[test]
    fn test_transfer_status_serde() {
        let s = TransferStatus::Declined;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"declined\"");
    }

    #[test]
    fn test_subscription_tier_cloud_roundtrip() {
        // Serde round-trip
        let tier = SubscriptionTier::Cloud;
        let json = serde_json::to_string(&tier).unwrap();
        assert_eq!(json, "\"cloud\"");
        let back: SubscriptionTier = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tier);

        // Display
        assert_eq!(SubscriptionTier::Cloud.to_string(), "cloud");

        // FromStr
        let parsed: SubscriptionTier = "cloud".parse().unwrap();
        assert_eq!(parsed, SubscriptionTier::Cloud);

        // AsRef<str>
        assert_eq!(SubscriptionTier::Cloud.as_ref(), "cloud");
    }
}
