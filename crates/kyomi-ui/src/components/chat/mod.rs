// SPDX-License-Identifier: AGPL-3.0-or-later

//! Chat components — WebSocket client, state machine, thinking events, message rendering, input, etc.

pub mod chat_state;
pub mod thinking;
pub mod websocket_client;

pub use chat_state::{ChatState, ChatStateMachine};
pub use thinking::{
    ThinkingEvent, ThinkingManager, ThinkingState, TokenUsage, process_thinking_event,
};
pub use websocket_client::{WebSocketContext, WebSocketProvider};
