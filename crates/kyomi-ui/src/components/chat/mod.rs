// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat components — WebSocket client, state machine, thinking events, message rendering, input, etc.

pub mod agent_thinking;
pub mod chat_engine;
pub mod chat_input;
pub mod chat_state;
pub mod copilot_chat;
pub mod inline_editable_title;
pub mod thinking;
pub mod tool_schema_renderer;
pub mod websocket_client;
pub mod websocket_debug_panel;

pub use agent_thinking::AgentThinking;
pub use chat_engine::{ChatEngine, ChatEngineConfig, SessionMode};
pub use chat_input::ChatInput;
pub use chat_state::{ChatState, ChatStateMachine};
pub use copilot_chat::CopilotChat;
pub use inline_editable_title::InlineEditableTitle;
pub use thinking::{
    ThinkingEvent, ThinkingManager, ThinkingState, TokenUsage, process_thinking_event,
};
pub use websocket_client::{CloseRecord, WebSocketContext, WebSocketProvider, WsDiagnostics};
pub use websocket_debug_panel::WebSocketDebugPanel;
