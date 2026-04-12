// SPDX-License-Identifier: AGPL-3.0-or-later

//! Agent execution service — main entry point for running agent chat.
//!
//! [`execute_agent_chat`] orchestrates the full lifecycle of an AI message:
//! 1. Build or use a provided system prompt
//! 2. Create the Anthropic client and agent
//! 3. Create the adapter with persistence
//! 4. Wire the thinking tracker
//! 5. Run the agent loop
//! 6. Handle errors and cancellation
//! 7. Return the result
//!
//! [`deliver_response`] streams the response via WebSocket.
//! [`generate_session_title`] fires a background task to title new sessions.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use kyomi_auth::chat_service;
use kyomi_auth::websocket::helpers as ws_helpers;
use kyomi_auth::websocket::WebSocketManager;
use kyomi_core::{DbPool, KVPool};
use kyomi_embed::LazyEmbedding;

use crate::adapter::ChatAgentAdapter;

/// SQL used to persist a row to `api_usage_log` after an agent turn.
///
/// Semantics of the two cost columns (see [`kyomi_core::models::ApiUsageLog`]):
/// * `cost_estimate` is what `BillingService::calculate_credits_info` sums —
///   it is the amount debited from Kyomi bundle credits. For BYOK rows it is
///   always `0.0` so BYOK traffic does not touch Kyomi billing.
/// * `provider_cost_usd` is BYOK-only observability — the real upstream
///   provider cost in USD. `NULL` for Kyomi rows where `cost_estimate` is
///   already the real cost.
///
/// Extracted as a const so it can be unit-tested without a database.
pub(crate) const API_USAGE_LOG_INSERT_SQL: &str = "INSERT INTO api_usage_log \
     (user_id, workspace_id, session_id, timestamp, provider, model, \
      input_tokens, output_tokens, total_tokens, \
      cache_creation_input_tokens, cache_read_input_tokens, \
      cost_estimate, component, provider_cost_usd) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 0, 0, $10, $11, $12)";
use crate::agent::{AgentConfig, CustomAgent};
use crate::anthropic::{AUDIT_MODEL, DEFAULT_MODEL};
use crate::prompt;
use crate::provider::{
    create_provider, create_provider_from_workspace, resolve_provider_config, ProviderKind,
};
use crate::thinking::AgentThinkingTracker;
use crate::tools::{create_default_registry, ToolContext, ToolFilter, TRIAL_CHAT_TOOLS};

// ---------------------------------------------------------------------------
// Configuration and result types
// ---------------------------------------------------------------------------

/// Configuration for a single agent execution.
#[derive(Debug, Clone)]
pub struct AgentExecutionConfig {
    pub session_id: String,
    pub user_id: String,
    pub workspace_id: String,
    pub message: String,
    pub model_name: Option<String>,
    pub temperature: f32,
    pub is_shared_conversation: bool,
    pub context_type: String,
    pub workspace_user_ids: Option<Vec<String>>,
    pub cancel_token: CancellationToken,
    pub current_time_user_tz: Option<String>,
    pub message_source: Option<String>,
    pub system_prompt: Option<String>,
    pub tools_subset: Option<Vec<String>>,
    pub max_iterations: u32,
    pub component: String,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    /// Optional conversation history for multi-turn conversations.
    /// Each entry is a (role, content) pair where role is "user" or "assistant".
    /// Used by trial chat to pass prior messages (max 10).
    pub conversation_history: Option<Vec<(String, String)>>,
    /// Display name for event attribution (e.g., WebSocket dashboard updates).
    /// Populated by the caller from the authenticated user record.
    pub user_display_name: String,
}

impl Default for AgentExecutionConfig {
    fn default() -> Self {
        Self {
            session_id: String::new(),
            user_id: String::new(),
            workspace_id: String::new(),
            message: String::new(),
            model_name: None,
            temperature: 0.7,
            is_shared_conversation: false,
            context_type: "chat".into(),
            workspace_user_ids: None,
            cancel_token: CancellationToken::new(),
            current_time_user_tz: None,
            message_source: None,
            system_prompt: None,
            tools_subset: None,
            max_iterations: 25,
            component: "custom_agent".into(),
            user_message_id: None,
            assistant_message_id: None,
            conversation_history: None,
            user_display_name: "Unknown".to_string(),
        }
    }
}

/// Result of an agent execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentExecutionResult {
    pub response_text: String,
    pub assistant_message_id: String,
    pub thinking_events: Vec<serde_json::Value>,
    pub token_usage: Option<serde_json::Value>,
    pub status: String,
    pub model: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Main execution
// ---------------------------------------------------------------------------

/// Execute the agent chat loop for a single user message.
///
/// This is the main entry point that coordinates all the pieces:
/// system prompt, agent creation, adapter wiring, and execution.
pub async fn execute_agent_chat(
    config: AgentExecutionConfig,
    db: &DbPool,
    kv: &KVPool,
    encryption_key: &Arc<[u8; 32]>,
    embedding: &LazyEmbedding,
    ws_manager: &WebSocketManager,
    app_config: &Arc<kyomi_core::Config>,
    connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    platforms: Arc<kyomi_core::platform::PlatformRegistry>,
) -> kyomi_core::Result<AgentExecutionResult> {
    // 1. Build system prompt (or use provided custom one).
    let mut system_prompt = if let Some(ref custom) = config.system_prompt {
        custom.clone()
    } else {
        prompt::build_system_prompt(
            db,
            &config.workspace_id,
            &config.user_id,
            config.is_shared_conversation,
        )
        .await?
    };

    // 2. Append user-scoped learnings to system prompt.
    let user_learnings = prompt::get_learnings_for_system_prompt(
        db,
        kv,
        embedding.wait_ready().await?,
        &config.user_id,
        &config.workspace_id,
        Some(&config.session_id),
    )
    .await?;

    if let Some(learnings_section) = user_learnings {
        system_prompt.push_str(&learnings_section);
    }

    // 3. Create LLM provider via factory.
    //
    // Trial chat is unauthenticated and uses a synthetic `"trial-workspace"`
    // ID that does not exist in the DB — it must stay on the legacy global
    // path that reads server-side Kyomi keys from env vars. Every real
    // workspace goes through the BYOK-aware factory.
    let is_trial_context = config.context_type == "trial_chat";
    let (client, provider_kind, is_byok) = if is_trial_context {
        let mut provider_config = resolve_provider_config(app_config)?;
        if let Some(ref model) = config.model_name {
            provider_config.model = Some(model.clone());
        } else if provider_config.model.is_none()
            && provider_config.provider == ProviderKind::Anthropic
        {
            provider_config.model = Some(DEFAULT_MODEL.to_string());
        }
        let provider_kind = provider_config.provider;
        let client = create_provider(provider_config)?;
        (client, provider_kind, false)
    } else {
        let mut ws_config =
            kyomi_auth::workspace_ai_config::load(db, &config.workspace_id)
                .await
                .map_err(|e| {
                    kyomi_core::Error::Internal(format!(
                        "failed to load workspace AI config for {}: {e}",
                        config.workspace_id
                    ))
                })?;
            // Per-request model override (from AgentExecutionConfig) always
            // wins over the workspace default.
        if let Some(ref model) = config.model_name {
            ws_config.model = Some(model.clone());
        } else if ws_config.model.is_none()
            && ws_config.provider == kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi
        {
            // Preserve legacy behaviour for Anthropic Kyomi workspaces that
            // had no explicit default model configured.
            ws_config.model = Some(DEFAULT_MODEL.to_string());
        }
        let is_byok = ws_config.is_byok();
        let client = create_provider_from_workspace(&ws_config, app_config)?;
        let provider_kind = match ws_config.provider {
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi
            | kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Anthropic => {
                ProviderKind::Anthropic
            }
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::OpenAI => ProviderKind::OpenAI,
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Gemini => ProviderKind::Gemini,
        };
        // For Kyomi-mode workspaces, the effective provider kind reflects
        // whatever the server env keys resolve to (which may be OpenAI or
        // Gemini for self-hosted tenants). Re-read it from the resolved
        // fallback in that case so usage logs attribute to the right vendor.
        let provider_kind = if matches!(
            ws_config.provider,
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi
        ) {
            resolve_provider_config(app_config)
                .map(|c| c.provider)
                .unwrap_or(provider_kind)
        } else {
            provider_kind
        };
        (client, provider_kind, is_byok)
    };
    // Read the authoritative model name from the provider (not from config defaults).
    let model_name = client.model().to_string();

    // 4. Create agent config with context-appropriate tool filter.
    //
    // Tool filtering mirrors the Python backend:
    //   - chat / slack:     exclude copilot-only + MCP-only (default)
    //   - copilot variants: exclude MCP-only, include copilot tools
    //   - trial_chat:       restricted subset via include_only
    //   - kyomi_watch:      data-query-only subset via include_only
    //
    // If tools_subset is explicitly provided it takes precedence.
    let tool_filter = if let Some(ref subset) = config.tools_subset {
        // Explicit tools_subset overrides context-based defaults.
        ToolFilter {
            exclude_copilot_only: false,
            exclude_mcp_only: false,
            include_only: Some(subset.clone()),
        }
    } else {
        match config.context_type.as_str() {
            // Copilot contexts: copilot tools available, MCP-only excluded.
            "copilot" | "dashboard_copilot" | "chart_builder_copilot" | "watch_copilot" => {
                ToolFilter {
                    exclude_copilot_only: false,
                    exclude_mcp_only: true,
                    include_only: None,
                }
            }
            // Trial chat: restricted tool subset.
            "trial_chat" => ToolFilter {
                exclude_copilot_only: false,
                exclude_mcp_only: false,
                include_only: Some(TRIAL_CHAT_TOOLS.iter().map(|s| s.to_string()).collect()),
            },
            // Chat, Slack, and everything else: exclude copilot + MCP tools.
            _ => ToolFilter {
                exclude_copilot_only: true,
                exclude_mcp_only: true,
                include_only: None,
            },
        }
    };

    let agent_config = AgentConfig {
        max_iterations: config.max_iterations,
        temperature: Some(config.temperature),
        tool_filter,
        ..Default::default()
    };

    // 5. Create tool registry (filtered by tools_subset if provided).
    let registry = Arc::new(create_default_registry());

    // 6. Create tool context.
    let is_trial = config.context_type == "trial_chat";

    let tool_context = ToolContext {
        db: db.clone(),
        kv: kv.clone(),
        user_id: config.user_id.clone(),
        workspace_id: config.workspace_id.clone(),
        encryption_key: encryption_key.clone(),
        embedding: embedding.clone(),
        ws_manager: ws_manager.clone(),
        config: app_config.clone(),
        is_trial,
        session_id: Some(config.session_id.clone()),
        supports_mcp_apps: false, // Chat/watch execution — not MCP
        workspace_roles: vec![], // Chat/watch — no admin checks needed
        connect_registry: connect_registry.clone(),
        platforms,
        user_display_name: config.user_display_name.clone(),
    };

    // 7. Create agent and inject system prompt as the first message.
    let user_names: HashMap<String, String> = HashMap::new();
    let mut agent = CustomAgent::new(client, agent_config, registry, tool_context, user_names);

    // The system prompt must be the first message in the agent's state.
    // Each LLM provider implementation extracts it and maps it to
    // the appropriate vendor-specific API field.
    agent
        .state_mut()
        .messages
        .push(crate::types::Message::system(&system_prompt));

    // 7b. Inject conversation history (for trial chat multi-turn support).
    // History messages go between the system prompt and the new user message.
    if let Some(ref history) = config.conversation_history {
        for (role, content) in history.iter().take(10) {
            let msg = match role.as_str() {
                "user" => crate::types::Message::user(content),
                "assistant" => crate::types::Message::assistant(content),
                _ => continue,
            };
            agent.state_mut().messages.push(msg);
        }
    }

    // 8. Create adapter.
    //    Trial chat passes None for session_id to skip PostgreSQL persistence
    //    (trial sessions are Redis-only, no chat_sessions/chat_messages rows).
    let adapter_session_id = if is_trial {
        None
    } else {
        Some(config.session_id.clone())
    };
    let mut adapter = ChatAgentAdapter::new(
        agent,
        config.user_id.clone(),
        config.workspace_id.clone(),
        adapter_session_id,
        config.component.clone(),
        db.clone(),
        encryption_key.clone(),
    );

    // 9. Generate assistant message ID (no DB placeholder).
    //    The adapter's persist_after_chat() saves the actual assistant response
    //    with the correct content. This avoids empty placeholder rows.
    let assistant_message_id = config
        .assistant_message_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    // 10. Create thinking tracker.
    let tracker = AgentThinkingTracker::new(
        config.session_id.clone(),
        config.user_id.clone(),
        assistant_message_id.clone(),
        ws_manager.clone(),
        config.workspace_user_ids.clone(),
        Some(config.context_type.clone()),
    );
    let tracker = Arc::new(tokio::sync::Mutex::new(tracker));

    // Signal agent start.
    {
        let mut t = tracker.lock().await;
        t.agent_started("Analyzing your question", "Starting analysis...").await;
    }

    // 11. Wire tracker to adapter.
    adapter.set_thinking_tracker(tracker.clone());

    // 12. Run the agent loop.
    let result = adapter
        .chat(
            &config.message,
            config.cancel_token.clone(),
            config.current_time_user_tz.as_deref(),
            config.message_source.as_deref(),
            Some(&config.user_id),
            config.user_message_id.as_deref(),
            Some(&assistant_message_id),
        )
        .await;

    // 13. Handle result.
    let (response_text, status, error_msg) = match result {
        Ok(text) => {
            // Signal completion.
            {
                let mut t = tracker.lock().await;
                t.agent_completed("success").await;
            }
            (text, "completed".to_string(), None)
        }
        Err(e) => {
            let error_str = e.to_string();
            let is_cancelled = error_str.contains("cancelled");
            let status = if is_cancelled { "cancelled" } else { "error" };

            if !is_cancelled {
                error!(error = %error_str, "Agent execution failed");
            }

            (
                if is_cancelled {
                    "Request was cancelled.".to_string()
                } else {
                    format!("I encountered an error while processing your request: {error_str}")
                },
                status.to_string(),
                Some(error_str),
            )
        }
    };

    // 14. Get thinking events and token usage from tracker.
    let (thinking_events, token_usage, input_tokens, output_tokens, total_cost) = {
        let t = tracker.lock().await;
        let events = t.get_events_for_storage();
        let inp = t.total_input_tokens();
        let out = t.total_output_tokens();
        let cost = t.total_cost();
        let usage = serde_json::json!({
            "input_tokens": inp,
            "output_tokens": out,
            "total_tokens": inp + out,
            "cost": cost,
        });
        (events, Some(usage), inp, out, cost)
    };

    // 14b. Log usage to api_usage_log for billing (skip trial users).
    // Apply 10% markup to cover payment processing fees.
    //
    // BYOK (workspace-owned keys): the workspace pays the provider directly,
    // so we record the row with `cost_estimate = 0.0` — token counts and
    // model names are preserved for diagnostics, but `check_ai_usage_allowed`
    // sums `cost_estimate` from this table and must not charge BYOK usage
    // against `ai_credits_used_usd`. Writing 0 keeps the single source of
    // truth for billing consistent without a parallel BYOK-only code path.
    const AI_COST_MULTIPLIER: f64 = 1.1;
    if !config.user_id.starts_with("trial_") && (input_tokens > 0 || output_tokens > 0) {
        let total_tokens = (input_tokens + output_tokens) as i32;
        // BYOK: `cost_estimate = 0.0` (not billed against Kyomi credits) and
        //       `provider_cost_usd = total_cost` so we keep observability into
        //       the real upstream provider spend. `total_cost` already comes
        //       from the tracker, which aggregates per-call costs computed
        //       from token counts × the provider's pricing table.
        // Kyomi: `cost_estimate = total_cost * markup` (billed) and
        //       `provider_cost_usd = NULL` — `cost_estimate` already reflects
        //       the real cost, so the extra column would be redundant.
        let (billed_cost, provider_cost_usd): (f64, Option<f64>) = if is_byok {
            (0.0, Some(total_cost))
        } else {
            (total_cost * AI_COST_MULTIPLIER, None)
        };
        let now = chrono::Utc::now();
        let provider_str = provider_kind.to_string();
        if let Err(e) = kyomi_core::db_execute!(
            db,
            API_USAGE_LOG_INSERT_SQL,
            &config.user_id,
            &config.workspace_id,
            &config.session_id as &str,
            now,
            &provider_str,
            &model_name,
            input_tokens as i32,
            output_tokens as i32,
            total_tokens,
            billed_cost,
            &config.component,
            provider_cost_usd
        ) {
            warn!(error = %e, "Failed to log API usage to database");
        }
    }

    // 15. Attach metadata to the assistant message (model, thinking events, token usage).
    //     The message content was already saved by persist_after_chat();
    //     we only update the extra_metadata column here.
    //     Trial chat does not persist to PostgreSQL — skip the DB update.
    if !is_trial {
        let metadata = serde_json::json!({
            "model": model_name,
            "thinking_events": thinking_events,
            "token_usage": token_usage,
            "component": config.component,
        });

        // Non-fatal: content is already saved by persist_after_chat, this only attaches metadata.
        if let Err(e) = chat_service::update_message(
            db,
            encryption_key,
            &assistant_message_id,
            None, // content already saved by persist_after_chat
            Some(&metadata),
        )
        .await
        {
            tracing::warn!(error = %e, "Failed to attach metadata to assistant message (non-fatal)");
        }
    }

    // 16. Record conversation in PostgreSQL (fire-and-forget).
    //     Creates rows in `conversation_discussed` for injected entities.
    //     Skipped for trial chat (no knowledge context).
    if !is_trial {
        let session_id_clone = config.session_id.clone();
        let user_id_clone = config.user_id.clone();
        let workspace_id_clone = config.workspace_id.clone();
        let kv_clone = kv.clone();
        let db_clone = db.clone();

        tokio::spawn(async move {
            if let Err(e) = record_conversation_background(
                &db_clone,
                &kv_clone,
                &session_id_clone,
                &user_id_clone,
                &workspace_id_clone,
            )
            .await
            {
                warn!(
                    session_id = %session_id_clone,
                    error = %e,
                    "Failed to record conversation (non-fatal)"
                );
            }
        });
    }

    info!(
        session_id = %config.session_id,
        assistant_message_id = %assistant_message_id,
        status = %status,
        "Agent execution completed"
    );

    Ok(AgentExecutionResult {
        response_text,
        assistant_message_id,
        thinking_events,
        token_usage,
        status,
        model: Some(model_name),
        error: error_msg,
    })
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Number of characters per WebSocket streaming chunk.
const STREAM_CHUNK_SIZE: usize = 50;

/// Delay between WebSocket streaming chunks (milliseconds).
const STREAM_CHUNK_DELAY_MS: u64 = 20;

// ---------------------------------------------------------------------------
// Response delivery
// ---------------------------------------------------------------------------

/// Stream the agent response via WebSocket.
///
/// Sends the response in 50-character chunks via `chat_stream` messages,
/// then sends a `chat_complete` message with the full response.
///
/// If `trial_session_id` is provided, publishes to the `ws:trial:{session_id}`
/// Redis channel instead of `ws:user:{user_id}` (trial users are anonymous).
///
/// If the session is shared, broadcasts to all workspace members.
#[allow(clippy::too_many_arguments)]
pub async fn deliver_response(
    ws_manager: &WebSocketManager,
    user_id: &str,
    session_id: &str,
    message_id: &str,
    response: &str,
    model: &str,
    usage: Option<serde_json::Value>,
    context_type: &str,
    workspace_id: Option<&str>,
    workspace_user_ids: Option<&[String]>,
    trial_session_id: Option<&str>,
) {
    // Stream response in chunks.
    let chars: Vec<char> = response.chars().collect();
    let mut offset = 0;

    while offset < chars.len() {
        let end = (offset + STREAM_CHUNK_SIZE).min(chars.len());
        let chunk: String = chars[offset..end].iter().collect();

        if let Some(trial_sid) = trial_session_id {
            // Trial mode: publish to ws:trial:{session_id} channel
            ws_helpers::send_trial_chat_stream(
                ws_manager,
                trial_sid,
                message_id,
                &chunk,
                Some(context_type),
            )
            .await;
        } else {
            ws_helpers::send_chat_stream(
                ws_manager,
                user_id,
                session_id,
                message_id,
                &chunk,
                Some(context_type),
            )
            .await;

            // Broadcast to shared conversation members if applicable.
            if let Some(ws_user_ids) = workspace_user_ids {
                for uid in ws_user_ids {
                    if uid != user_id {
                        ws_helpers::send_chat_stream(
                            ws_manager,
                            uid,
                            session_id,
                            message_id,
                            &chunk,
                            Some(context_type),
                        )
                        .await;
                    }
                }
            }
        }

        offset = end;
        tokio::time::sleep(tokio::time::Duration::from_millis(STREAM_CHUNK_DELAY_MS)).await;
    }

    // Send complete message.
    if let Some(trial_sid) = trial_session_id {
        ws_helpers::send_trial_chat_complete(
            ws_manager,
            trial_sid,
            message_id,
            response,
            model,
            usage,
            Some(context_type),
        )
        .await;
    } else {
        ws_helpers::send_chat_complete(
            ws_manager,
            user_id,
            session_id,
            message_id,
            response,
            model,
            usage.clone(),
            Some(context_type),
        )
        .await;

        // Broadcast completion to shared conversation members.
        if let (Some(wid), Some(_ws_user_ids)) = (workspace_id, workspace_user_ids) {
            ws_helpers::broadcast_chat_complete(
                ws_manager,
                wid,
                session_id,
                message_id,
                response,
                model,
                usage,
                Some(user_id),
            )
            .await;
        }
    }
}

// ---------------------------------------------------------------------------
// Title generation
// ---------------------------------------------------------------------------

/// Generate a session title in the background (fire-and-forget).
///
/// Calls Claude Haiku with a short prompt to generate a 3-6 word title
/// from the first user message, then updates the DB and broadcasts via
/// WebSocket.
pub fn generate_session_title(
    db: DbPool,
    ws_manager: WebSocketManager,
    session_id: String,
    user_id: String,
    workspace_id: String,
    first_message: String,
    app_config: Arc<kyomi_core::Config>,
) {
    tokio::spawn(async move {
        let result = generate_title_inner(
            &db,
            &ws_manager,
            &session_id,
            &user_id,
            &workspace_id,
            &first_message,
            &app_config,
        )
        .await;

        if let Err(e) = result {
            warn!(
                session_id = %session_id,
                error = %e,
                "Failed to generate session title"
            );
        }
    });
}

/// Internal title generation logic.
async fn generate_title_inner(
    db: &DbPool,
    ws_manager: &WebSocketManager,
    session_id: &str,
    user_id: &str,
    workspace_id: &str,
    first_message: &str,
    app_config: &kyomi_core::Config,
) -> kyomi_core::Result<()> {
    // Verify session still exists before making the API call.
    let session = chat_service::get_session(db, session_id).await?;
    if session.is_none() {
        info!(session_id = %session_id, "Session deleted before title generation, skipping");
        return Ok(());
    }

    let system_prompt = "Generate a concise 3-6 word title for this conversation based on the \
                         user's first message. Return ONLY the title text, nothing else. \
                         No quotes, no hashes, no prefixes.";

    // Use a cheap/fast model for title generation, routed through the
    // workspace's configured provider (Kyomi-managed or BYOK).
    //
    // Design decision: in Kyomi mode we override Anthropic to AUDIT_MODEL
    // (Haiku) to minimise server-side cost. In BYOK mode we honour whatever
    // model the workspace configured — overriding a BYOK customer's chosen
    // model (which might be a specific GPT variant, a Gemini tier, or an
    // Anthropic tier they explicitly chose) would be a surprising silent
    // downgrade. If the workspace has no model set, `create_provider_from_workspace`
    // falls back to the provider's built-in default, and for Anthropic BYOK
    // with no model we still prefer AUDIT_MODEL to keep title-gen cheap.
    // TODO: Add per-provider cheapest-model selection for OpenAI/Gemini
    // (currently uses their default model, which may be more expensive than needed).
    let mut ws_config = kyomi_auth::workspace_ai_config::load(db, workspace_id)
        .await
        .map_err(|e| {
            kyomi_core::Error::Internal(format!(
                "failed to load workspace AI config for title generation ({workspace_id}): {e}"
            ))
        })?;
    let provider_is_anthropic = matches!(
        ws_config.provider,
        kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi
            | kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Anthropic
    );
    if provider_is_anthropic
        && matches!(
            ws_config.provider,
            kyomi_auth::workspace_ai_config::WorkspaceAiProvider::Kyomi
        )
    {
        // Kyomi-managed: always force Haiku for title generation.
        ws_config.model = Some(AUDIT_MODEL.to_string());
    } else if provider_is_anthropic && ws_config.model.is_none() {
        // BYOK Anthropic with no explicit model: use Haiku to keep cost low.
        ws_config.model = Some(AUDIT_MODEL.to_string());
    }
    let client = create_provider_from_workspace(&ws_config, app_config)?;

    let messages = vec![
        crate::types::Message::system(system_prompt),
        crate::types::Message::user(first_message),
    ];
    let tools = vec![];
    let user_names = HashMap::new();

    // Use a generous token budget to accommodate models that use chain-of-thought
    // thinking (e.g. Qwen3) — the thinking tokens are stripped in parse_response.
    let response = client.complete(&messages, &tools, Some(0.3), 512, &user_names).await?;

    // Clean up the title: remove quotes, hashes, and trim.
    let title = response
        .content
        .trim()
        .trim_matches('"')
        .trim_matches('#')
        .trim()
        .to_string();

    if title.is_empty() {
        return Ok(());
    }

    // Update DB.
    chat_service::update_session_title(db, session_id, &title).await?;

    // Broadcast via WebSocket.
    ws_helpers::send_title_update(ws_manager, user_id, session_id, &title).await;

    info!(
        session_id = %session_id,
        title = %title,
        "Generated session title"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Episodic layer -- post-conversation recording
// ---------------------------------------------------------------------------

/// Load conversation context from Redis and record the conversation in PostgreSQL.
///
/// Called as a fire-and-forget background task after `execute_agent_chat` completes.
async fn record_conversation_background(
    db: &DbPool,
    kv: &KVPool,
    session_id: &str,
    user_id: &str,
    workspace_id: &str,
) -> anyhow::Result<()> {
    // Load the conversation context that was accumulated during the chat.
    let context = kyomi_knowledge::context::load_context(kv, session_id).await?;

    if context.is_empty() {
        return Ok(());
    }

    // Record the conversation in PostgreSQL (conversation_discussed table).
    kyomi_knowledge::episodic::record_conversation(db, session_id, user_id, workspace_id, &context)
        .await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Contract: api_usage_log INSERT includes provider_cost_usd -----------
    //
    // Guards the BYOK observability column: `provider_cost_usd` must appear
    // in both the column list and the VALUES clause, and the parameter count
    // must match. If someone accidentally drops the column or reorders the
    // INSERT, this test catches it at `cargo test` time rather than in prod.

    #[test]
    fn api_usage_log_insert_has_provider_cost_usd_column() {
        let sql = API_USAGE_LOG_INSERT_SQL;
        assert!(
            sql.contains("provider_cost_usd"),
            "INSERT must reference provider_cost_usd: {sql}"
        );
        // Must be the 12th bind parameter (after the 11 existing ones).
        assert!(
            sql.contains("$12"),
            "INSERT must bind $12 for provider_cost_usd: {sql}"
        );
        // Sanity: the 12 placeholders line up with 12 named columns (two of
        // the columns — cache_creation_input_tokens / cache_read_input_tokens
        // — are hard-coded to 0 in VALUES, so 14 columns total, 12 binds).
        let placeholder_count = (1..=12).filter(|n| sql.contains(&format!("${n}"))).count();
        assert_eq!(
            placeholder_count, 12,
            "INSERT should have exactly 12 bind placeholders: {sql}"
        );
    }

    #[test]
    fn api_usage_log_insert_targets_correct_table() {
        assert!(API_USAGE_LOG_INSERT_SQL.contains("INSERT INTO api_usage_log"));
    }

    #[test]
    fn agent_execution_config_defaults() {
        let config = AgentExecutionConfig::default();
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_iterations, 25);
        assert_eq!(config.component, "custom_agent");
        assert_eq!(config.context_type, "chat");
        assert!(config.model_name.is_none());
        assert!(config.system_prompt.is_none());
        assert!(config.tools_subset.is_none());
        assert!(!config.is_shared_conversation);
    }

    #[test]
    fn agent_execution_result_serialization() {
        let result = AgentExecutionResult {
            response_text: "Here are the results.".into(),
            assistant_message_id: "msg-123".into(),
            thinking_events: vec![serde_json::json!({"event_type": "agent_start"})],
            token_usage: Some(serde_json::json!({"input_tokens": 100, "output_tokens": 50})),
            status: "completed".into(),
            model: Some("claude-sonnet-4-5-20250929".into()),
            error: None,
        };

        let json = serde_json::to_value(&result).expect("should serialize");
        assert_eq!(json["response_text"], "Here are the results.");
        assert_eq!(json["status"], "completed");
        assert_eq!(json["assistant_message_id"], "msg-123");
        assert!(json["error"].is_null());
    }

    #[test]
    fn agent_execution_result_with_error() {
        let result = AgentExecutionResult {
            response_text: "Error occurred".into(),
            assistant_message_id: "msg-456".into(),
            thinking_events: vec![],
            token_usage: None,
            status: "error".into(),
            model: None,
            error: Some("API rate limit exceeded".into()),
        };

        let json = serde_json::to_value(&result).expect("should serialize");
        assert_eq!(json["status"], "error");
        assert_eq!(json["error"], "API rate limit exceeded");
    }

    // -- Contract: AgentExecutionConfig comprehensive defaults ---------------

    #[test]
    fn agent_execution_config_defaults_comprehensive() {
        let config = AgentExecutionConfig::default();
        assert_eq!(config.session_id, "");
        assert_eq!(config.user_id, "");
        assert_eq!(config.workspace_id, "");
        assert_eq!(config.message, "");
        assert!(config.model_name.is_none());
        assert!((config.temperature - 0.7).abs() < f32::EPSILON);
        assert!(!config.is_shared_conversation);
        assert_eq!(config.context_type, "chat");
        assert!(config.workspace_user_ids.is_none());
        assert!(config.current_time_user_tz.is_none());
        assert!(config.message_source.is_none());
        assert!(config.system_prompt.is_none());
        assert!(config.tools_subset.is_none());
        assert_eq!(config.max_iterations, 25);
        assert_eq!(config.component, "custom_agent");
        assert!(config.assistant_message_id.is_none());
    }

    // -- Contract: AgentExecutionConfig with all fields set -----------------

    #[test]
    fn agent_execution_config_with_all_fields() {
        let config = AgentExecutionConfig {
            session_id: "sess-123".into(),
            user_id: "user-456".into(),
            workspace_id: "ws-789".into(),
            message: "Show me revenue".into(),
            model_name: Some("claude-haiku-4-5-20251001".into()),
            temperature: 0.3,
            is_shared_conversation: true,
            context_type: "copilot".into(),
            workspace_user_ids: Some(vec!["user-456".into(), "user-abc".into()]),
            cancel_token: CancellationToken::new(),
            current_time_user_tz: Some("2025-01-15T10:30:00+11:00".into()),
            message_source: Some("web".into()),
            system_prompt: Some("Custom prompt.".into()),
            tools_subset: Some(vec!["search_knowledge".into(), "query_datasource".into()]),
            max_iterations: 10,
            component: "watch_agent".into(),
            assistant_message_id: Some("msg-pre-created".into()),
            user_message_id: Some("msg-user-123".into()),
            conversation_history: Some(vec![
                ("user".into(), "What's our MRR?".into()),
                ("assistant".into(), "Let me look that up.".into()),
            ]),
            user_display_name: "Test User".to_string(),
        };

        assert_eq!(config.session_id, "sess-123");
        assert_eq!(config.model_name.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert!(config.is_shared_conversation);
        assert_eq!(config.context_type, "copilot");
        assert_eq!(config.workspace_user_ids.as_ref().unwrap().len(), 2);
        assert_eq!(config.tools_subset.as_ref().unwrap().len(), 2);
        assert_eq!(config.max_iterations, 10);
        assert_eq!(config.conversation_history.as_ref().unwrap().len(), 2);
    }

    // -- Contract: AgentExecutionResult serialization roundtrip -------------

    #[test]
    fn agent_execution_result_roundtrip() {
        let result = AgentExecutionResult {
            response_text: "The revenue was $1.2M in Q4.".into(),
            assistant_message_id: "msg-abc-123".into(),
            thinking_events: vec![
                serde_json::json!({"event_type": "agent_start", "title": "Starting"}),
                serde_json::json!({"event_type": "tool_execution_start", "title": "Querying"}),
                serde_json::json!({"event_type": "agent_complete", "title": "Done"}),
            ],
            token_usage: Some(serde_json::json!({
                "input_tokens": 5000,
                "output_tokens": 800,
                "total_tokens": 5800,
                "cost": 0.027,
            })),
            status: "completed".into(),
            model: Some("claude-sonnet-4-5-20250929".into()),
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let restored: AgentExecutionResult = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.response_text, "The revenue was $1.2M in Q4.");
        assert_eq!(restored.assistant_message_id, "msg-abc-123");
        assert_eq!(restored.thinking_events.len(), 3);
        assert_eq!(restored.status, "completed");
        assert_eq!(restored.model.as_deref(), Some("claude-sonnet-4-5-20250929"));
        assert!(restored.error.is_none());

        let usage = restored.token_usage.unwrap();
        assert_eq!(usage["input_tokens"], 5000);
        assert_eq!(usage["output_tokens"], 800);
    }

    // -- Contract: AgentExecutionResult with cancelled status ----------------

    #[test]
    fn agent_execution_result_cancelled() {
        let result = AgentExecutionResult {
            response_text: "Request was cancelled.".into(),
            assistant_message_id: "msg-cancel".into(),
            thinking_events: vec![],
            token_usage: None,
            status: "cancelled".into(),
            model: Some("claude-sonnet-4-5-20250929".into()),
            error: Some("Request cancelled".into()),
        };

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["status"], "cancelled");
        assert_eq!(json["error"], "Request cancelled");
        assert_eq!(json["response_text"], "Request was cancelled.");
    }

    // -- Contract: AgentExecutionResult empty thinking_events ----------------

    #[test]
    fn agent_execution_result_empty_events() {
        let result = AgentExecutionResult {
            response_text: "Simple response.".into(),
            assistant_message_id: "msg-simple".into(),
            thinking_events: vec![],
            token_usage: None,
            status: "completed".into(),
            model: None,
            error: None,
        };

        let json = serde_json::to_value(&result).unwrap();
        assert!(json["thinking_events"].as_array().unwrap().is_empty());
        assert!(json["token_usage"].is_null());
        assert!(json["model"].is_null());
    }

    // -- Contract: Streaming constants are reasonable -----------------------

    #[test]
    fn streaming_chunk_size_is_reasonable() {
        assert_eq!(STREAM_CHUNK_SIZE, 50);
    }

    #[test]
    fn streaming_chunk_delay_is_reasonable() {
        assert_eq!(STREAM_CHUNK_DELAY_MS, 20);
    }

    // -- Contract: CancellationToken can be created from config defaults ----

    #[test]
    fn cancel_token_default_is_not_cancelled() {
        let config = AgentExecutionConfig::default();
        assert!(!config.cancel_token.is_cancelled());
    }

    #[test]
    fn cancel_token_can_be_cancelled() {
        let config = AgentExecutionConfig::default();
        config.cancel_token.cancel();
        assert!(config.cancel_token.is_cancelled());
    }
}
