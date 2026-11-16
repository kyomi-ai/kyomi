// SPDX-License-Identifier: AGPL-3.0-or-later

//! Per-workspace web search / fetch backend override (KYO-969).
//!
//! Stored under `workspaces.settings.custom_settings.web_search` and
//! `.web_fetch`, each an object:
//!
//! ```json
//! { "provider": "tavily", "api_key_encrypted": "<workspace_secrets ciphertext>",
//!   "base_url": "https://...", "options": { "...": "opaque pass-through" } }
//! ```
//!
//! A workspace entry replaces the instance setting for that tool entirely (it
//! is how a workspace enables a backend the instance has none for, or brings its
//! own key and quota). The API key is encrypted at rest with the same master key
//! as the BYOK LLM key ([`crate::workspace_secrets`]); plaintext is never
//! stored. This is configuration only: there is no settings form (owner
//! decision), but [`store`] is the write path one would call.

use kyomi_core::DbPool;
use kyomi_core::web_tools::{WebBackendSettings, WebToolKind};

use crate::workspace_secrets;

/// Settings key under `custom_settings` for a tool.
fn settings_key(tool: WebToolKind) -> &'static str {
    match tool {
        WebToolKind::Search => "web_search",
        WebToolKind::Fetch => "web_fetch",
    }
}

/// Decrypted per-workspace overrides; `None` = no override for that tool.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceWebOverride {
    pub search: Option<WebBackendSettings>,
    pub fetch: Option<WebBackendSettings>,
}

#[derive(Debug, thiserror::Error)]
pub enum WorkspaceWebConfigError {
    #[error("workspace {0} not found")]
    WorkspaceNotFound(String),
    #[error("workspace secrets encryption is unavailable (WORKSPACE_SECRETS_KEY is not configured)")]
    EncryptionUnavailable,
    #[error(transparent)]
    Secret(#[from] workspace_secrets::WorkspaceSecretError),
    #[error("invalid stored web tool setting: {0}")]
    Invalid(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(sqlx::FromRow)]
struct SettingsRow {
    settings: Option<serde_json::Value>,
}

/// Parse one tool's entry out of the settings JSON, decrypting the key.
fn read_entry(
    settings: &Option<serde_json::Value>,
    tool: WebToolKind,
) -> Result<Option<WebBackendSettings>, WorkspaceWebConfigError> {
    let Some(entry) = settings
        .as_ref()
        .and_then(|s| s.get("custom_settings"))
        .and_then(|c| c.get(settings_key(tool)))
        .filter(|e| !e.is_null())
    else {
        return Ok(None);
    };
    let text = |k: &str| entry.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|v| !v.is_empty());
    let api_key = match text("api_key_encrypted") {
        Some(ct) => {
            if !workspace_secrets::is_available() {
                return Err(WorkspaceWebConfigError::EncryptionUnavailable);
            }
            Some(workspace_secrets::decrypt_secret(ct)?)
        }
        None => None,
    };
    let options = match entry.get("options") {
        None | Some(serde_json::Value::Null) => None,
        Some(v @ serde_json::Value::Object(_)) => Some(v.to_string()),
        Some(_) => return Err(WorkspaceWebConfigError::Invalid(format!("{}.options must be an object", settings_key(tool)))),
    };
    Ok(Some(WebBackendSettings {
        provider: text("provider").map(str::to_string),
        api_key,
        base_url: text("base_url").map(str::to_string),
        options,
    }))
}

/// Load a workspace's web tool overrides.
pub async fn load(db: &DbPool, workspace_id: &str) -> Result<WorkspaceWebOverride, WorkspaceWebConfigError> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        SettingsRow,
        "SELECT settings FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .ok_or_else(|| WorkspaceWebConfigError::WorkspaceNotFound(workspace_id.to_string()))?;
    Ok(WorkspaceWebOverride {
        search: read_entry(&row.settings, WebToolKind::Search)?,
        fetch: read_entry(&row.settings, WebToolKind::Fetch)?,
    })
}

/// Store (or, with `None`, remove) one tool's override, encrypting the key.
/// Other `settings` keys are preserved.
pub async fn store(
    db: &DbPool,
    workspace_id: &str,
    tool: WebToolKind,
    value: Option<&WebBackendSettings>,
) -> Result<(), WorkspaceWebConfigError> {
    let row = kyomi_core::db_fetch_optional!(
        db,
        SettingsRow,
        "SELECT settings FROM workspaces WHERE workspace_id = $1",
        workspace_id
    )?
    .ok_or_else(|| WorkspaceWebConfigError::WorkspaceNotFound(workspace_id.to_string()))?;
    let mut settings = row.settings.unwrap_or_else(|| serde_json::json!({}));
    let custom = settings
        .as_object_mut()
        .ok_or_else(|| WorkspaceWebConfigError::Invalid("settings is not an object".into()))?
        .entry("custom_settings")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| WorkspaceWebConfigError::Invalid("custom_settings is not an object".into()))?;
    match value {
        None => {
            custom.remove(settings_key(tool));
        }
        Some(v) => {
            let mut entry = serde_json::Map::new();
            if let Some(p) = &v.provider {
                entry.insert("provider".into(), p.clone().into());
            }
            if let Some(key) = &v.api_key {
                if !workspace_secrets::is_available() {
                    return Err(WorkspaceWebConfigError::EncryptionUnavailable);
                }
                entry.insert("api_key_encrypted".into(), workspace_secrets::encrypt_secret(key)?.into());
            }
            if let Some(u) = &v.base_url {
                entry.insert("base_url".into(), u.clone().into());
            }
            if let Some(o) = &v.options {
                let parsed: serde_json::Value = serde_json::from_str(o)
                    .map_err(|_| WorkspaceWebConfigError::Invalid("options must be a JSON object".into()))?;
                if !parsed.is_object() {
                    return Err(WorkspaceWebConfigError::Invalid("options must be a JSON object".into()));
                }
                entry.insert("options".into(), parsed);
            }
            custom.insert(settings_key(tool).into(), serde_json::Value::Object(entry));
        }
    }
    let text = serde_json::to_string(&settings).map_err(|e| WorkspaceWebConfigError::Invalid(e.to_string()))?;
    match db {
        kyomi_core::db::DbPool::Postgres(pg) => {
            sqlx::query("UPDATE workspaces SET settings = $1::json WHERE workspace_id = $2")
                .bind(&text)
                .bind(workspace_id)
                .execute(pg)
                .await?;
        }
        kyomi_core::db::DbPool::Sqlite(sq) => {
            sqlx::query("UPDATE workspaces SET settings = $1 WHERE workspace_id = $2")
                .bind(&text)
                .bind(workspace_id)
                .execute(sq)
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_or_null_entries_mean_no_override() {
        assert_eq!(read_entry(&None, WebToolKind::Search).unwrap(), None);
        let s = Some(json!({"custom_settings": {"web_search": null}}));
        assert_eq!(read_entry(&s, WebToolKind::Search).unwrap(), None);
        assert_eq!(read_entry(&s, WebToolKind::Fetch).unwrap(), None);
    }

    #[test]
    fn entry_without_key_reads_provider_url_and_opaque_options() {
        let s = Some(json!({"custom_settings": {"web_fetch": {
            "provider": "builtin", "base_url": " ", "options": {"x": 1}}}}));
        let got = read_entry(&s, WebToolKind::Fetch).unwrap().unwrap();
        assert_eq!(got.provider.as_deref(), Some("builtin"));
        assert_eq!(got.base_url, None, "blank values are ignored");
        assert_eq!(got.api_key, None);
        assert_eq!(got.options.as_deref(), Some(r#"{"x":1}"#));
    }

    #[test]
    fn non_object_options_are_rejected() {
        let s = Some(json!({"custom_settings": {"web_search": {"provider": "tavily", "options": [1]}}}));
        assert!(matches!(read_entry(&s, WebToolKind::Search), Err(WorkspaceWebConfigError::Invalid(_))));
    }

    async fn db_with_workspace(settings: Option<&str>) -> DbPool {
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        let sq = kyomi_test_db::sqlite_pool(&db);
        kyomi_test_db::seed_user(sq, "u1", "u1@test.local").await;
        kyomi_test_db::seed_workspace_with_name(sq, "w1", "u1", "W").await;
        if let Some(s) = settings {
            sqlx::query("UPDATE workspaces SET settings = $1 WHERE workspace_id = 'w1'").bind(s).execute(sq).await.unwrap();
        }
        db
    }

    #[tokio::test]
    async fn store_then_load_round_trips_and_preserves_other_settings() {
        let db = db_with_workspace(Some(r#"{"custom_settings":{"default_model":"m1"},"other":1}"#)).await;
        let value = WebBackendSettings {
            provider: Some("builtin".into()),
            api_key: None,
            base_url: None,
            options: Some(r#"{"a":1}"#.into()),
        };
        store(&db, "w1", WebToolKind::Fetch, Some(&value)).await.unwrap();
        let loaded = load(&db, "w1").await.unwrap();
        assert_eq!(loaded.fetch.as_ref().unwrap().provider.as_deref(), Some("builtin"));
        assert_eq!(loaded.fetch.as_ref().unwrap().options.as_deref(), Some(r#"{"a":1}"#));
        assert_eq!(loaded.search, None);
        let row = kyomi_core::db_fetch_optional!(&db, SettingsRow, "SELECT settings FROM workspaces WHERE workspace_id = $1", "w1")
            .unwrap()
            .unwrap();
        let settings = row.settings.unwrap();
        assert_eq!(settings["custom_settings"]["default_model"], "m1");
        assert_eq!(settings["other"], 1);

        store(&db, "w1", WebToolKind::Fetch, None).await.unwrap();
        assert_eq!(load(&db, "w1").await.unwrap(), WorkspaceWebOverride::default());
    }

    #[tokio::test]
    async fn unknown_workspace_is_an_error_and_bad_options_are_not_stored() {
        let db = db_with_workspace(None).await;
        assert!(matches!(load(&db, "nope").await, Err(WorkspaceWebConfigError::WorkspaceNotFound(_))));
        let bad = WebBackendSettings { provider: Some("tavily".into()), options: Some("[1]".into()), ..Default::default() };
        assert!(store(&db, "w1", WebToolKind::Search, Some(&bad)).await.is_err());
        assert_eq!(load(&db, "w1").await.unwrap(), WorkspaceWebOverride::default());
    }
}
