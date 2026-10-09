// SPDX-License-Identifier: AGPL-3.0-or-later

//! Layout-owned chat runs. Views attach to a session; the single WebSocket
//! listener keeps receiving its frames even when no chat view is mounted.

use std::collections::{HashMap, HashSet, VecDeque};

use leptos::prelude::*;

use super::chat_state::{ChatState, ChatStateData};
use super::thinking::ThinkingData;
use crate::server_fns::chat::ChatMessageItem;

#[derive(Clone)]
pub(crate) struct ChatRun {
    pub messages: ArcRwSignal<Vec<ChatMessageItem>>,
    pub chat_state: ChatStateData,
    pub thinking: ThinkingData,
    pub context_type: Option<String>,
}

impl ChatRun {
    pub(crate) fn new(context_type: Option<String>) -> Self {
        Self {
            messages: ArcRwSignal::new(Vec::new()),
            chat_state: ChatStateData::new(),
            thinking: ThinkingData::new(),
            context_type,
        }
    }

    fn is_live(&self) -> bool {
        matches!(
            self.chat_state.state().get_untracked(),
            ChatState::Sending | ChatState::Streaming | ChatState::Cancelling
        ) || self.messages.with_untracked(|messages| {
            messages
                .last()
                .is_some_and(|message| message.status == "in_progress")
        })
    }
}

struct ViewRegistration {
    session_id: Option<String>,
    events: Vec<String>,
    callback: Option<Callback<(String, serde_json::Value)>>,
}

// Remember terminal message identities without retaining their content. This
// prevents late frames from recreating an evicted run, while a new assistant
// message in the same conversation can still start a fresh run.
const MAX_TERMINAL_MESSAGES: usize = 256;

struct StoreInner {
    runs: HashMap<String, ChatRun>,
    terminal_messages: HashSet<(String, String)>,
    terminal_order: VecDeque<(String, String)>,
    views: HashMap<u64, ViewRegistration>,
    next_view: u64,
    #[cfg(target_arch = "wasm32")]
    ws: Option<super::WebSocketContext>,
    #[cfg(target_arch = "wasm32")]
    subscriptions: HashMap<String, send_wrapper::SendWrapper<Box<dyn FnOnce()>>>,
}

/// Retains only mounted sessions and live runs. Completed runs have no idle
/// cache: the last view release or terminal event drops them immediately.
/// Live runs are deliberately exempt from an arbitrary count limit; dropping
/// one under memory pressure would reintroduce navigation gaps.
#[derive(Clone, Copy)]
pub struct ChatRunStore {
    inner: StoredValue<StoreInner>,
    pub(crate) generation: RwSignal<u64>,
}

impl Default for ChatRunStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatRunStore {
    pub fn new() -> Self {
        Self {
            inner: StoredValue::new(StoreInner {
                runs: HashMap::new(),
                terminal_messages: HashSet::new(),
                terminal_order: VecDeque::new(),
                views: HashMap::new(),
                next_view: 0,
                #[cfg(target_arch = "wasm32")]
                ws: None,
                #[cfg(target_arch = "wasm32")]
                subscriptions: HashMap::new(),
            }),
            generation: RwSignal::new(0),
        }
    }

    pub(crate) fn register_view(
        self,
        events: Vec<String>,
        callback: Option<Callback<(String, serde_json::Value)>>,
    ) -> u64 {
        #[cfg(target_arch = "wasm32")]
        for event in &events {
            self.subscribe(event);
        }
        let mut id = 0;
        self.inner.update_value(|inner| {
            inner.next_view += 1;
            id = inner.next_view;
            inner.views.insert(
                id,
                ViewRegistration {
                    session_id: None,
                    events,
                    callback,
                },
            );
        });
        id
    }

    pub(crate) fn attach(self, view: u64, session_id: Option<&str>, draft: ChatRun) -> ChatRun {
        let mut selected = draft.clone();
        self.inner.update_value(|inner| {
            if let Some(registration) = inner.views.get_mut(&view) {
                registration.session_id = session_id.map(str::to_string);
            }
            if let Some(sid) = session_id {
                selected = inner.runs.entry(sid.to_string()).or_insert(draft).clone();
            }
            inner.evict();
        });
        selected
    }

    pub(crate) fn release_view(self, view: u64) {
        self.inner.try_update_value(|inner| {
            inner.views.remove(&view);
            inner.evict();
        });
    }

    pub(crate) fn forget(self, session_id: &str) {
        self.inner.try_update_value(|inner| {
            inner.runs.remove(session_id);
        });
    }

    pub(crate) fn sweep(self) {
        self.inner.try_update_value(StoreInner::evict);
    }

    fn clear(self) {
        self.inner.try_update_value(|inner| {
            for run in inner.runs.values() {
                run.messages.set(Vec::new());
                run.thinking.clear_all();
                run.chat_state.reset();
            }
            inner.runs.clear();
            inner.terminal_messages.clear();
            inner.terminal_order.clear();
            for view in inner.views.values_mut() {
                view.session_id = None;
            }
        });
        self.generation.try_update(|generation| *generation += 1);
    }

    pub fn receive(self, event: &str, msg: super::websocket_client::WebSocketMessage) {
        use super::chat_engine::{
            context_type_matches, error_event_context_type, handle_run_event,
        };
        let Some(sid) = msg.session_id.as_deref().filter(|sid| !sid.is_empty()) else {
            return;
        };
        let context_type = msg
            .data
            .as_ref()
            .and_then(|data| {
                if event == "agent_thinking" {
                    data.get("event")
                } else {
                    Some(data)
                }
            })
            .and_then(|data| data.get("context_type"))
            .and_then(|value| value.as_str());
        let context_type = if event == "error" {
            error_event_context_type(msg.data.as_ref())
        } else {
            context_type
        };
        let mut run = None;
        let mut callbacks = Vec::new();
        self.inner.try_update_value(|inner| {
            if matches!(event, "agent_thinking" | "chat_stream")
                && msg.message_id.as_ref().is_some_and(|message_id| {
                    inner
                        .terminal_messages
                        .contains(&(sid.to_string(), message_id.clone()))
                })
            {
                return;
            }
            // Ephemeral sessions only exist while their view is active. Late
            // frames after deleting one must not recreate it in the store.
            // Main-chat producers explicitly send "chat"; these runs may
            // first arrive before a view mounts and route by session alone.
            if !inner.runs.contains_key(sid)
                && context_type.is_some_and(|context| context != "chat")
            {
                return;
            }
            if !inner.runs.contains_key(sid)
                && !matches!(
                    event,
                    "agent_thinking"
                        | "chat_stream"
                        | "chat_complete"
                        | "error"
                        | "request_cancelled"
                )
            {
                return;
            }
            let entry = inner
                .runs
                .entry(sid.to_string())
                .or_insert_with(|| ChatRun::new(None));
            if event != "token_usage_update"
                && !context_type_matches(entry.context_type.as_deref(), context_type)
            {
                return;
            }
            run = Some(entry.clone());
            for view in inner.views.values() {
                if view.session_id.as_deref() == Some(sid)
                    && view.events.iter().any(|name| name == event)
                    && let Some(callback) = view.callback
                {
                    callbacks.push(callback);
                }
            }
        });
        let Some(run) = run else { return };
        handle_run_event(&run, event, &msg);
        if let Some(message_id) = &msg.message_id {
            let terminal = run.messages.with_untracked(|messages| {
                messages.iter().any(|message| {
                    message.message_id == *message_id
                        && matches!(
                            message.status.as_str(),
                            "complete" | "error" | "cancelled" | "interrupted"
                        )
                })
            }) || matches!(event, "error" | "request_cancelled")
                && matches!(
                    run.chat_state.state().get_untracked(),
                    ChatState::Error | ChatState::Cancelled
                );
            if terminal {
                self.inner
                    .try_update_value(|inner| inner.remember_terminal(sid, message_id));
            }
        }
        if let Some(data) = msg.data {
            for callback in callbacks {
                callback.try_run((event.to_string(), data.clone()));
            }
        }
        self.sweep();
    }

    #[cfg(target_arch = "wasm32")]
    fn subscribe(self, event: &str) {
        let ws = self.inner.with_value(|inner| {
            if inner.subscriptions.contains_key(event) {
                None
            } else {
                inner.ws.clone()
            }
        });
        let Some(ws) = ws else { return };
        let name = event.to_string();
        let unsubscribe = ws.subscribe(event, move |msg| self.receive(&name, msg));
        self.inner.update_value(|inner| {
            inner.subscriptions.insert(
                event.to_string(),
                send_wrapper::SendWrapper::new(unsubscribe),
            );
        });
    }
}

impl StoreInner {
    fn remember_terminal(&mut self, session_id: &str, message_id: &str) {
        let identity = (session_id.to_string(), message_id.to_string());
        if self.terminal_messages.insert(identity.clone()) {
            self.terminal_order.push_back(identity);
        }
        while self.terminal_order.len() > MAX_TERMINAL_MESSAGES {
            if let Some(oldest) = self.terminal_order.pop_front() {
                self.terminal_messages.remove(&oldest);
            }
        }
    }

    fn evict(&mut self) {
        let mounted = |sid: &str| {
            self.views
                .values()
                .any(|view| view.session_id.as_deref() == Some(sid))
        };
        let eligible: Vec<_> = self
            .runs
            .iter()
            .filter(|(sid, run)| !mounted(sid) && !run.is_live())
            .map(|(sid, _)| sid.clone())
            .collect();
        for sid in eligible {
            if let Some(run) = self.runs.remove(&sid)
                && let Some(last) = run.messages.with_untracked(|messages| {
                    messages
                        .last()
                        .filter(|message| message.message_type == "assistant")
                        .map(|message| message.message_id.clone())
                })
            {
                self.remember_terminal(&sid, &last);
            }
            tracing::debug!(session_id = %sid, reason = "terminal_without_view", "Evicted chat run");
        }
    }
}

/// Installed once under Layout's WebSocketProvider. Only this component owns
/// chat subscriptions; route cleanup merely releases a view registration.
#[component]
pub fn ChatRunStoreProvider(
    workspace_id: Signal<Option<String>>,
    children: Children,
) -> impl IntoView {
    let store = ChatRunStore::new();
    provide_context(store);
    Effect::new(move |previous: Option<Option<String>>| {
        let workspace = workspace_id.get();
        if previous.is_some_and(|previous| previous != workspace) {
            store.clear();
        }
        workspace
    });
    #[cfg(target_arch = "wasm32")]
    {
        let ws = expect_context::<super::WebSocketContext>();
        store.inner.update_value(|inner| inner.ws = Some(ws));
        for event in [
            "agent_thinking",
            "chat_stream",
            "chat_complete",
            "token_usage_update",
            "error",
            "request_cancelled",
            "shared_chat_message",
        ] {
            store.subscribe(event);
        }
        on_cleanup(move || {
            if let Some(subscriptions) = store
                .inner
                .try_update_value(|inner| std::mem::take(&mut inner.subscriptions))
            {
                for (_, unsubscribe) in subscriptions {
                    unsubscribe.take()();
                }
            }
        });
    }
    children()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(
        event: &str,
        session: Option<&str>,
        content: &str,
        offset: usize,
    ) -> super::super::websocket_client::WebSocketMessage {
        serde_json::from_value(json!({
            "type": event, "session_id": session, "message_id": "answer",
            "timestamp": "2026-09-27T01:00:00Z",
            "data": { "content": content, "content_offset": offset, "full_content": content,
                "context_type": "chat" }
        }))
        .expect("valid WebSocket fixture")
    }

    fn attach(store: ChatRunStore, sid: &str) -> (u64, ChatRun) {
        let view = store.register_view(vec![], None);
        (view, store.attach(view, Some(sid), ChatRun::new(None)))
    }

    #[test]
    fn thinking_usage_errors_and_cancellation_follow_the_retained_session() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            let (view, run) = attach(store, "a");
            let mut thought = frame("agent_thinking", Some("a"), "", 0);
            thought.data = Some(json!({ "event": {
                "event_id": "step-1", "event_type": "agent_thought",
                "timestamp": "now", "title": "Working while away"
            }}));
            store.receive("agent_thinking", thought);
            store.release_view(view);
            let mut usage = frame("token_usage_update", Some("a"), "", 0);
            usage.data = Some(json!({"token_usage": {"input_tokens": 12, "output_tokens": 8}}));
            store.receive("token_usage_update", usage);
            let state = run.thinking.get_for_message("answer");
            assert_eq!(state.events[0].title, "Working while away");
            assert_eq!(
                state
                    .token_usage
                    .expect("usage retained while unmounted")
                    .output_tokens,
                8
            );
            store.receive(
                "request_cancelled",
                frame("request_cancelled", Some("a"), "", 0),
            );
            assert!(run.thinking.get_for_message("answer").cancelled);
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
            store.receive("chat_stream", frame("chat_stream", Some("b"), "partial", 0));
            let mut error = frame("error", Some("b"), "", 0);
            error.data = Some(json!({"error": "test error"}));
            store.receive("error", error);
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
        });
    }

    #[test]
    fn navigation_retains_all_frames_and_terminal_unmounted_run_is_evicted() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            let (view, run) = attach(store, "a");
            store.receive("chat_stream", frame("chat_stream", Some("a"), "before ", 0));
            store.release_view(view);
            store.receive("chat_stream", frame("chat_stream", Some("a"), "away ", 7));
            let (returning, restored) = attach(store, "a");
            assert_eq!(restored.messages.get_untracked()[0].content, "before away ");
            assert_eq!(
                restored.chat_state.state().get_untracked(),
                ChatState::Streaming
            );
            store.receive("chat_stream", frame("chat_stream", Some("a"), "back", 12));
            assert_eq!(run.messages.get_untracked()[0].content, "before away back");
            store.release_view(returning);
            assert_eq!(store.inner.with_value(|inner| inner.runs.len()), 1);
            store.receive(
                "chat_complete",
                frame("chat_complete", Some("a"), "before away back", 0),
            );
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
        });
    }

    #[test]
    fn background_sessions_are_separate_and_missing_identity_is_rejected() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            store.receive("chat_stream", frame("chat_stream", Some("a"), "Alpha", 0));
            store.receive("chat_stream", frame("chat_stream", Some("b"), "Beta", 0));
            store.receive("chat_stream", frame("chat_stream", None, "foreign", 0));
            let (_, a) = attach(store, "a");
            let (_, b) = attach(store, "b");
            assert_eq!(a.messages.get_untracked()[0].content, "Alpha");
            assert_eq!(b.messages.get_untracked()[0].content, "Beta");
            assert_eq!(store.inner.with_value(|inner| inner.runs.len()), 2);
        });
    }

    #[test]
    fn unseen_main_chat_thinking_accepts_contextless_terminal_events() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            let mut thought = frame("agent_thinking", Some("a"), "", 0);
            thought.data = Some(json!({ "event": {
                "event_id": "background", "event_type": "agent_thought",
                "timestamp": "now", "title": "Background reasoning", "context_type": "chat"
            }}));
            store.receive("agent_thinking", thought);
            assert_eq!(store.inner.with_value(|inner| inner.runs.len()), 1);
            let mut cancelled = frame("request_cancelled", Some("a"), "", 0);
            cancelled.data = Some(json!({}));
            store.receive("request_cancelled", cancelled);
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
        });
    }

    #[test]
    fn completed_runs_do_not_accumulate_while_all_live_runs_remain_retained() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            for index in 0..34 {
                let sid = format!("live-{index}");
                store.receive("chat_stream", frame("chat_stream", Some(&sid), "live", 0));
            }
            for index in 0..200 {
                let sid = format!("done-{index}");
                let (view, _) = attach(store, &sid);
                store.receive(
                    "chat_complete",
                    frame("chat_complete", Some(&sid), "done", 0),
                );
                store.release_view(view);
            }
            assert_eq!(store.inner.with_value(|inner| inner.runs.len()), 34);
            for index in 0..34 {
                let sid = format!("live-{index}");
                store.receive(
                    "chat_complete",
                    frame("chat_complete", Some(&sid), "live done", 0),
                );
            }
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
        });
    }

    #[test]
    fn copilot_release_drops_custom_callbacks_and_late_frames_cannot_recreate_session() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            let calls = ArcRwSignal::new(0);
            let captured = calls.clone();
            let view = store.register_view(
                vec!["chart_update".into()],
                Some(Callback::new(move |_| captured.update(|n| *n += 1))),
            );
            let run = store.attach(
                view,
                Some("copilot"),
                ChatRun::new(Some("chart_copilot".into())),
            );
            let mut event = frame("chart_update", Some("copilot"), "", 0);
            event.data = Some(json!({"context_type":"chart_copilot"}));
            store.receive("chart_update", event.clone());
            assert_eq!(calls.get_untracked(), 1);
            run.chat_state.start_sending("copilot");
            store.release_view(view);
            store.forget("copilot");
            store.receive("chart_update", event);
            let mut late = frame("chat_stream", Some("copilot"), "late", 0);
            late.data.as_mut().expect("fixture data")["context_type"] = json!("chart_copilot");
            store.receive("chat_stream", late);
            assert_eq!(calls.get_untracked(), 1);
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
        });
    }

    #[test]
    fn delayed_frames_do_not_restart_or_retain_a_terminal_run() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            for terminal in ["complete", "error", "cancelled", "interrupted"] {
                let (view, run) = attach(store, terminal);
                store.receive(
                    "chat_stream",
                    frame("chat_stream", Some(terminal), "answer", 0),
                );
                store.receive(
                    "chat_complete",
                    frame("chat_complete", Some(terminal), "answer", 0),
                );
                run.messages
                    .update(|messages| messages[0].status = terminal.into());
                let mut thought = frame("agent_thinking", Some(terminal), "", 0);
                thought.data = Some(json!({"event": {
                    "event_id": "late", "event_type": "agent_thought",
                    "timestamp": "now", "title": "Late reasoning"
                }}));
                store.receive("agent_thinking", thought);
                store.receive(
                    "chat_stream",
                    frame("chat_stream", Some(terminal), "late", 6),
                );
                assert_eq!(run.chat_state.state().get_untracked(), ChatState::Idle);
                assert_eq!(run.messages.get_untracked()[0].content, "answer");
                assert!(!run.thinking.get_for_message("answer").is_active);
                store.release_view(view);
                assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
            }
        });
    }

    #[test]
    fn terminal_identity_cache_is_bounded_and_does_not_block_the_next_turn() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            for index in 0..MAX_TERMINAL_MESSAGES + 20 {
                let sid = format!("done-{index}");
                store.receive(
                    "chat_complete",
                    frame("chat_complete", Some(&sid), "done", 0),
                );
                store.receive("chat_stream", frame("chat_stream", Some(&sid), "late", 0));
                assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
            }
            assert_eq!(
                store
                    .inner
                    .with_value(|inner| inner.terminal_messages.len()),
                MAX_TERMINAL_MESSAGES
            );
            assert_eq!(
                store.inner.with_value(|inner| inner.terminal_order.len()),
                MAX_TERMINAL_MESSAGES
            );
            let sid = format!("done-{}", MAX_TERMINAL_MESSAGES + 19);
            let mut next_turn = frame("chat_stream", Some(&sid), "new turn", 0);
            next_turn.message_id = Some("next-answer".into());
            store.receive("chat_stream", next_turn);
            let (_, run) = attach(store, &sid);
            assert_eq!(run.messages.get_untracked()[0].content, "new turn");
            store.clear();
            assert!(
                store
                    .inner
                    .with_value(|inner| inner.terminal_messages.is_empty())
            );
            assert!(
                store
                    .inner
                    .with_value(|inner| inner.terminal_order.is_empty())
            );
        });
    }

    #[test]
    fn workspace_change_clears_retained_content() {
        Owner::new().with(|| {
            let store = ChatRunStore::new();
            let (_, run) = attach(store, "a");
            store.receive("chat_stream", frame("chat_stream", Some("a"), "private", 0));
            store.clear();
            assert!(run.messages.get_untracked().is_empty());
            assert_eq!(run.chat_state.state().get_untracked(), ChatState::Idle);
            assert!(store.inner.with_value(|inner| inner.runs.is_empty()));
        });
    }
}
