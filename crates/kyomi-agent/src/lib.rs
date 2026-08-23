// SPDX-License-Identifier: AGPL-3.0-or-later

#![recursion_limit = "256"]
//! kyomi-agent -- AI agent system for the Kyomi backend.
//!
//! Provides: Anthropic LLM client, agent loop, tool framework,
//! thinking tracker, chat adapter, execution service, and prompt building.
//!
//! # Architecture
//!
//! The agent system is layered:
//!
//! 1. **Types** ([`types`]) -- Core LLM types shared across the system.
//! 2. **Provider Abstraction** ([`provider`]) -- [`LLMProvider`] trait + factory.
//!    Concrete backends: [`anthropic`], [`openai`], [`gemini`].
//! 3. **Tool Framework** ([`tools`]) -- `AgentTool` trait and `ToolRegistry`.
//! 4. **Agent Loop** ([`agent`]) -- `CustomAgent` with iteration-based tool execution.
//! 5. **Prompt** ([`prompt`]) -- System prompt building and learning injection.
//! 6. **Adapter** ([`adapter`]) -- `ChatAgentAdapter` with DB persistence and context loading.
//! 7. **Execution** ([`execution`]) -- `AgentExecutionService` entry point for chat.
//! 8. **Watch Execution** ([`watch_execution`]) -- Watch-specific agent execution engine.
//! 9. **Alert** ([`alert`]) -- Watch alert delivery (WebSocket, Slack, email).
//! 10. **Scheduler** ([`scheduler`]) -- Background watch poller with CAS locking.
//! 11. **Catalog Scheduler** ([`catalog_scheduler`]) -- Background catalog refresh + token cleanup.

pub mod adapter;
pub mod agent;
pub mod alert;
pub mod anthropic;
pub mod catalog;
pub mod catalog_scheduler;
pub mod copilot;
pub mod chartml_factory;
pub mod chartml_utils;
pub mod compaction;
pub mod d3_format;
pub mod execution;
pub mod forecast;
pub mod gemini;
pub mod openai;
pub mod markdown_to_typst;
pub mod pdf_export;
pub mod pdf_typst;
pub mod prompt;
pub mod provider;
pub mod scheduler;
// Slack message processor moved to enterprise/kyomi-slack crate (Phase 12).
pub mod thinking;
pub mod tools;
pub mod types;
pub mod watch_execution;
pub mod web_push;

pub use adapter::{ChatAgentAdapter, UserMessagePersistence};
pub use agent::{AgentCallbacks, AgentConfig, AgentState, CustomAgent};
pub use anthropic::{AnthropicClient, AUDIT_MODEL, DEFAULT_MODEL};
pub use execution::{
    deliver_response, execute_agent_chat, generate_dashboard_summary,
    generate_session_title, AgentExecutionConfig, DashboardSummaryParams,
    AgentExecutionEnv, AgentExecutionResult,
};
pub use thinking::{AgentThinkingEvent, AgentThinkingTracker, ThinkingEventType};
pub use tools::{
    create_default_registry, AgentTool, ToolContext, ToolFilter, ToolRegistry, WATCH_TOOLS,
};
pub use provider::{
    create_provider, create_provider_from_workspace, resolve_provider_config, LLMProvider,
    LLMProviderConfig, ProviderKind,
};
pub use types::*;
pub use catalog_scheduler::CatalogRefreshScheduler;
pub use scheduler::{WatchScheduler, WatchSchedulerDeps};
pub use watch_execution::execute_watch;
