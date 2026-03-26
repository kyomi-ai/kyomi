// SPDX-License-Identifier: AGPL-3.0-or-later

//! Trial chat page — full implementation matching `apps/frontend/src/components/TrialChat.jsx`.
//!
//! Standalone full-screen page (no sidebar, no auth) that allows anonymous
//! users to explore a sample SaaS dataset via AI chat. Sessions are tracked
//! server-side by IP with HMAC-signed tokens.
//!
//! ## Architecture
//!
//! - Session tokens are kept in reactive signals (not localStorage).
//! - Message rendering is inline (not reusing the main chat module) since
//!   trial mode is intentionally isolated from the authenticated chat flow.
//! - Assistant messages are rendered with `MarkdownRenderer`, which supports
//!   full markdown (via pulldown-cmark) and inline ChartML chart rendering.
//!   Trial charts use embedded data so no datasource query execution is needed.
//! - The `?q=` query parameter supports auto-submission from the marketing site.

use leptos::prelude::*;
use leptos_router::hooks::use_query_map;

use crate::components::dashboard::MarkdownRenderer;
use crate::components::{Button, ButtonSize, Spinner};
use crate::server_fns::trial::{
    create_trial_session, send_trial_message, ConversationEntry, TrialChatResponse,
};

// ─────────────────────────────────────────────────────────────────────────────
// Constants
// ─────────────────────────────────────────────────────────────────────────────

/// Suggested questions shown in the welcome state.
/// Matches the React `SUGGESTED_QUESTIONS` array.
const SUGGESTED_QUESTIONS: &[&str] = &[
    "What's our current MRR?",
    "Show me user signups over time",
    "Which subscription plans are most popular?",
    "What's our conversion funnel look like?",
];

/// Maximum number of conversation history entries sent to the server.
/// Each exchange = 2 entries (user + assistant), so 10 entries = 5 exchanges.
const MAX_CONVERSATION_HISTORY: usize = 10;

// ─────────────────────────────────────────────────────────────────────────────
// A2.4: Trial message type
// ─────────────────────────────────────────────────────────────────────────────

/// Local message type for the trial chat UI.
///
/// This is intentionally separate from the authenticated chat's message type.
/// Trial mode does not need pinning, dashboards, or any of the other
/// interactive features the full chat supports. ChartML charts are rendered
/// inline via `MarkdownRenderer` using embedded data (no query execution).
#[derive(Clone, Debug)]
struct TrialMessage {
    /// Unique client-side message ID.
    id: String,
    /// `"user"` or `"assistant"`.
    role: String,
    /// Message content (plain text for user, possibly markdown for assistant).
    content: String,
    /// `true` while waiting for the server response (placeholder state).
    is_loading: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// A2.3: Signup prompt modal
// ─────────────────────────────────────────────────────────────────────────────

/// Non-dismissible modal shown when the trial query limit is reached.
///
/// Matches `SignupPromptModal` from React `TrialChat.jsx`.
/// The modal cannot be closed — the user must sign up to continue.
#[component]
fn SignupPromptModal(
    /// Whether the modal is visible.
    show: ReadSignal<bool>,
) -> impl IntoView {
    view! {
        <Show when=move || show.get()>
            <div class="fixed inset-0 z-50 flex items-center justify-center">
                // Backdrop — not dismissible (no click handler)
                <div class="absolute inset-0 bg-[var(--color-overlay)]"></div>

                // Modal content
                <div class="relative z-10 w-full max-w-md p-6 mx-4 bg-card rounded-lg shadow border border-border">
                    <h2 class="text-xl font-semibold text-foreground mb-2">
                        "You've reached your trial limit"
                    </h2>
                    <p class="text-muted-foreground mb-4">
                        "Sign up to get unlimited access to Kyomi and connect your own data."
                    </p>

                    // Feature bullets
                    <ul class="text-sm text-muted-foreground space-y-2 mb-6">
                        <li class="flex items-start gap-2">
                            <span class="text-primary mt-0.5">"✓"</span>
                            <span>"Connect your own databases and data warehouses"</span>
                        </li>
                        <li class="flex items-start gap-2">
                            <span class="text-primary mt-0.5">"✓"</span>
                            <span>"Unlimited AI-powered data analysis"</span>
                        </li>
                        <li class="flex items-start gap-2">
                            <span class="text-primary mt-0.5">"✓"</span>
                            <span>"Interactive dashboards and visualizations"</span>
                        </li>
                        <li class="flex items-start gap-2">
                            <span class="text-primary mt-0.5">"✓"</span>
                            <span>"Automated data watches and alerts"</span>
                        </li>
                        <li class="flex items-start gap-2">
                            <span class="text-primary mt-0.5">"✓"</span>
                            <span>"Team collaboration and sharing"</span>
                        </li>
                    </ul>

                    <a href="/login" class="block">
                        <Button class="w-full">
                            "Sign Up Free"
                        </Button>
                    </a>
                </div>
            </div>
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Welcome state component
// ─────────────────────────────────────────────────────────────────────────────

/// Welcome state shown when no messages have been sent.
///
/// Matches the React welcome state: heading, description, data availability
/// grid, and suggested question buttons. Clicking a suggestion auto-submits.
#[component]
fn WelcomeState(
    /// Whether the session is still initializing (disables buttons).
    is_initializing: ReadSignal<bool>,
    /// Whether a message is currently being sent (disables buttons).
    is_loading: ReadSignal<bool>,
    /// Callback to submit a suggested question.
    on_suggest: Callback<String>,
) -> impl IntoView {
    let disabled = Signal::derive(move || is_loading.get() || is_initializing.get());

    view! {
        <div class="space-y-6">
            // Heading
            <div class="text-center">
                <h2 class="text-2xl font-semibold text-foreground mb-2">
                    "Welcome to Kyomi"
                </h2>
                <p class="text-muted-foreground max-w-md mx-auto">
                    "Ask questions about the sample SaaS dataset below. "
                    "We've loaded 18 months of data from a fictional company called Acme Analytics."
                </p>
            </div>

            // Data availability grid — matches React exactly
            <div class="bg-card border border-border rounded-lg p-4">
                <h3 class="font-medium text-foreground mb-3">"Available Data"</h3>
                <div class="grid grid-cols-2 gap-3 text-sm">
                    <DataItem
                        title="Subscriptions"
                        description="MRR, plans, churn data"
                    />
                    <DataItem
                        title="Users"
                        description="Signups, roles, activity"
                    />
                    <DataItem
                        title="Events"
                        description="Feature usage, 50k+ events"
                    />
                    <DataItem
                        title="Website Sessions"
                        description="Funnel, conversions"
                    />
                </div>
            </div>

            // Suggested questions
            <div>
                <p class="text-sm text-muted-foreground mb-3 text-center">"Try one of these:"</p>
                <div class="flex flex-wrap gap-2 justify-center">
                    {SUGGESTED_QUESTIONS.iter().map(|&question| {
                        let q = question.to_string();
                        let on_suggest = on_suggest.clone();
                        view! {
                            <button
                                class="px-3 py-2 text-sm bg-card border border-border rounded-lg hover:border-primary hover:text-primary transition-colors disabled:opacity-50"
                                disabled=disabled
                                on:click=move |_| {
                                    on_suggest.run(q.clone());
                                }
                            >
                                {question}
                            </button>
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}

/// A single data availability item in the welcome grid.
#[component]
fn DataItem(
    /// Data category name.
    title: &'static str,
    /// Brief description.
    description: &'static str,
) -> impl IntoView {
    view! {
        <div class="flex items-start gap-2">
            <div class="w-2 h-2 rounded-full bg-primary mt-1.5 flex-shrink-0"></div>
            <div>
                <span class="font-medium text-foreground">{title}</span>
                <p class="text-muted-foreground text-xs">{description}</p>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Message rendering
// ─────────────────────────────────────────────────────────────────────────────

/// Render a single trial message bubble.
///
/// User messages: right-aligned with primary background (plain text).
/// Assistant messages: left-aligned with card background. Uses `MarkdownRenderer`
/// for full markdown + ChartML chart rendering. Loading state shows spinner.
#[component]
fn MessageBubble(message: TrialMessage) -> impl IntoView {
    let is_user = message.role == "user";
    let is_loading = message.is_loading;

    // Container alignment classes
    let container_class = if is_user {
        "flex justify-end"
    } else {
        "flex justify-start"
    };

    // Bubble classes — matches React ChatMessage styling
    let bubble_class = if is_user {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-primary text-primary-foreground"
    } else {
        "max-w-[80%] rounded-2xl px-4 py-3 bg-card text-card-foreground border border-border"
    };

    view! {
        <div class=container_class>
            <div class=bubble_class>
                {if is_loading {
                    // Placeholder: spinner while waiting for response
                    view! {
                        <div class="flex items-center gap-2 text-muted-foreground">
                            <Spinner />
                            <span class="text-sm">"Analyzing your question..."</span>
                        </div>
                    }.into_any()
                } else if is_user {
                    // User messages: plain text, no HTML interpretation needed
                    let content = message.content.clone();
                    view! {
                        <div class="whitespace-pre-wrap break-words">{content}</div>
                    }.into_any()
                } else {
                    // Assistant messages: full markdown + ChartML rendering via MarkdownRenderer.
                    // Trial charts have inline data (no datasource slug), so MarkdownRenderer
                    // renders them directly without needing query execution.
                    let content = message.content.clone();
                    let content_signal = Signal::derive(move || content.clone());
                    view! {
                        <MarkdownRenderer content=content_signal />
                    }.into_any()
                }}
            </div>
        </div>
    }
}


// ─────────────────────────────────────────────────────────────────────────────
// A2.1–A2.3: Main TrialChatPage component
// ─────────────────────────────────────────────────────────────────────────────

/// Trial chat page — full-screen standalone page for anonymous data exploration.
///
/// Matches `apps/frontend/src/components/TrialChat.jsx`:
/// - Header with Kyomi logo, query counter, signup button
/// - Scrollable messages area with welcome state
/// - Input footer with text input, send button, query counter
/// - Non-dismissible signup modal when limit is reached
/// - `?q=question` URL parameter for auto-submission from marketing site
#[component]
pub fn TrialChatPage() -> impl IntoView {
    // ── A2.1: State signals ─────────────────────────────────────────────
    let (messages, set_messages) = signal(Vec::<TrialMessage>::new());
    let (conversation_history, set_conversation_history) =
        signal(Vec::<ConversationEntry>::new());
    let (input_value, set_input_value) = signal(String::new());
    let (is_loading, set_is_loading) = signal(false);
    let (is_initializing, set_is_initializing) = signal(true);
    let (query_count, set_query_count) = signal(0u64);
    let (queries_remaining, set_queries_remaining) = signal(5u64);
    let (error, set_error) = signal(Option::<String>::None);
    let (show_signup_modal, set_show_signup_modal) = signal(false);
    let (session_token, set_session_token) = signal(String::new());
    let (access_token, set_access_token) = signal(String::new());

    // Message ID counter for generating unique IDs.
    let msg_counter = RwSignal::new(0u64);

    // Track whether we've already auto-submitted from `?q=` param.
    let has_auto_submitted = RwSignal::new(false);

    // Read `?q=` query parameter for auto-submission.
    let query_map = use_query_map();
    let auto_question = query_map
        .get_untracked()
        .get("q")
        .unwrap_or_default();
    let auto_question = StoredValue::new(auto_question);

    // ── Generate unique message ID ──────────────────────────────────────
    let generate_message_id = move || -> String {
        let count = msg_counter.get_untracked();
        msg_counter.set(count + 1);
        #[cfg(target_arch = "wasm32")]
        {
            format!(
                "trial-msg-{}-{}",
                js_sys::Date::now() as u64,
                count
            )
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            format!("trial-msg-0-{}", count)
        }
    };

    // ── A2.2: Send message handler ──────────────────────────────────────
    //
    // This is a closure stored in a signal so it can be called from both
    // the form submit handler and the suggested question buttons.

    let send_message = Callback::new(move |message_text: String| {
        let message_text = message_text.trim().to_string();
        if message_text.is_empty() || is_loading.get_untracked() {
            return;
        }

        set_error.set(None);
        set_input_value.set(String::new());
        set_is_loading.set(true);

        // Generate message IDs
        let user_msg_id = generate_message_id();
        let assistant_msg_id = generate_message_id();

        // Add user message (optimistic)
        set_messages.update(|msgs| {
            msgs.push(TrialMessage {
                id: user_msg_id.clone(),
                role: "user".to_string(),
                content: message_text.clone(),
                is_loading: false,
            });
        });

        // Add placeholder assistant message (loading state)
        let assistant_id_for_update = assistant_msg_id.clone();
        set_messages.update(|msgs| {
            msgs.push(TrialMessage {
                id: assistant_msg_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                is_loading: true,
            });
        });

        // Scroll to bottom after adding messages
        scroll_to_bottom();

        // Capture current state for the async block
        let current_session_token = session_token.get_untracked();
        let current_access_token = access_token.get_untracked();
        let current_history = conversation_history.get_untracked();
        let user_msg_id_for_error = user_msg_id.clone();

        leptos::task::spawn_local(async move {
            // Get the user's timezone for the system prompt
            let current_time_user_tz = get_user_timezone();

            let result: Result<TrialChatResponse, leptos::prelude::ServerFnError> =
                send_trial_message(
                    message_text.clone(),
                    current_history,
                    current_session_token,
                    current_access_token,
                    current_time_user_tz,
                )
                .await;

            match result {
                Ok(response) => {
                    // Update query state
                    set_query_count.set(response.query_count);
                    set_queries_remaining.set(response.queries_remaining.max(0) as u64);

                    // Replace placeholder with real response
                    set_messages.update(|msgs| {
                        if let Some(msg) = msgs
                            .iter_mut()
                            .find(|m| m.id == assistant_id_for_update)
                        {
                            msg.id = response.message_id;
                            msg.content = response.response.clone();
                            msg.is_loading = false;
                        }
                    });

                    // Refresh tokens if included in response
                    if let Some(new_session_token) = response.session_token {
                        set_session_token.set(new_session_token);
                    }
                    if let Some(new_access_token) = response.trial_access_token {
                        set_access_token.set(new_access_token);
                    }

                    // Update conversation history (keep last MAX_CONVERSATION_HISTORY entries)
                    set_conversation_history.update(|history| {
                        history.push(ConversationEntry {
                            role: "user".to_string(),
                            content: message_text,
                        });
                        history.push(ConversationEntry {
                            role: "assistant".to_string(),
                            content: response.response,
                        });
                        // Trim to last MAX_CONVERSATION_HISTORY entries
                        if history.len() > MAX_CONVERSATION_HISTORY {
                            let excess = history.len() - MAX_CONVERSATION_HISTORY;
                            history.drain(..excess);
                        }
                    });

                    // Scroll to bottom to show the response
                    scroll_to_bottom();
                }
                Err(err) => {
                    let err_msg = err.to_string();

                    if err_msg.contains("limit")
                        || err_msg.contains("rate limit")
                        || err_msg.contains("429")
                    {
                        // Query limit reached — show signup modal
                        set_show_signup_modal.set(true);
                        // Remove both user and assistant messages
                        set_messages.update(|msgs| {
                            msgs.retain(|m| {
                                m.id != user_msg_id_for_error
                                    && m.id != assistant_id_for_update
                            });
                        });
                    } else if err_msg.contains("expired") || err_msg.contains("401") {
                        // Token expired — clear tokens and show error
                        set_session_token.set(String::new());
                        set_access_token.set(String::new());
                        set_error.set(Some(
                            "Your session has expired. Please refresh the page to continue."
                                .to_string(),
                        ));
                        // Remove both messages
                        set_messages.update(|msgs| {
                            msgs.retain(|m| {
                                m.id != user_msg_id_for_error
                                    && m.id != assistant_id_for_update
                            });
                        });
                    } else {
                        // Generic error — show message, remove both messages
                        set_error.set(Some(err_msg));
                        set_messages.update(|msgs| {
                            msgs.retain(|m| {
                                m.id != user_msg_id_for_error
                                    && m.id != assistant_id_for_update
                            });
                        });
                    }
                }
            }

            set_is_loading.set(false);
        });
    });

    // ── A2.1: Session initialization ────────────────────────────────────
    //
    // On mount, call `create_trial_session()` to get tokens and query state.
    // After initialization, auto-submit the `?q=` question if present.
    {
        let send_message = send_message.clone();
        Effect::new(move |_| {
            let send_message = send_message.clone();
            leptos::task::spawn_local(async move {
                match create_trial_session().await {
                    Ok(session) => {
                        set_session_token.set(session.session_token);
                        set_access_token.set(session.trial_access_token);
                        set_queries_remaining.set(session.queries_remaining);
                        set_query_count.set(5 - session.queries_remaining);
                        set_is_initializing.set(false);

                        // Auto-submit from ?q= parameter
                        let q = auto_question.get_value();
                        if !q.is_empty() && !has_auto_submitted.get_untracked() {
                            has_auto_submitted.set(true);
                            send_message.run(q);
                        }
                    }
                    Err(err) => {
                        let err_msg = err.to_string();
                        set_error.set(Some(format!(
                            "Failed to initialize trial session: {err_msg}"
                        )));
                        set_is_initializing.set(false);

                        if err_msg.contains("limit") {
                            set_show_signup_modal.set(true);
                        }
                    }
                }
            });
        });
    }

    // ── A2.3: Reset conversation handler ────────────────────────────────
    let handle_reset = move |_| {
        set_messages.set(Vec::new());
        set_conversation_history.set(Vec::new());
        set_error.set(None);
        // Query count is NOT reset — it's server-side
    };

    // ── Form submit handler ─────────────────────────────────────────────
    let on_submit = {
        let send_message = send_message.clone();
        move |ev: leptos::ev::SubmitEvent| {
            ev.prevent_default();
            let val = input_value.get_untracked();
            send_message.run(val);
        }
    };

    // ── Suggested question handler ──────────────────────────────────────
    let on_suggest = {
        let send_message = send_message.clone();
        Callback::new(move |question: String| {
            send_message.run(question);
        })
    };

    // ── Derived signals ─────────────────────────────────────────────────
    let has_messages = Signal::derive(move || !messages.get().is_empty());
    let has_queries = Signal::derive(move || query_count.get() > 0);
    let send_disabled = Signal::derive(move || {
        is_loading.get() || is_initializing.get() || input_value.get().trim().is_empty()
    });
    let input_disabled = Signal::derive(move || is_loading.get() || is_initializing.get());
    let input_placeholder = Signal::derive(move || {
        if is_initializing.get() {
            "Initializing trial session..."
        } else {
            "Ask a question about the data..."
        }
    });

    // ── Render ──────────────────────────────────────────────────────────
    view! {
        <div class="flex flex-col h-screen bg-background">
            // ── Header ──────────────────────────────────────────────────
            <header class="flex-shrink-0 border-b border-border bg-background px-4 py-3">
                <div class="max-w-4xl mx-auto flex items-center justify-between">
                    // Logo + label
                    <a
                        href="https://kyomi.ai"
                        class="flex items-center gap-3 hover:opacity-80 transition-opacity"
                    >
                        <img
                            src="/kyomi_full_logo.svg"
                            alt="Kyomi"
                            class="h-10 dark:hidden"
                        />
                        <img
                            src="/kyomi_full_logo_white.svg"
                            alt="Kyomi"
                            class="h-10 hidden dark:block"
                        />
                        <span class="text-muted-foreground text-sm hidden sm:inline">
                            {"\u{00B7} Sample Data Explorer"}
                        </span>
                    </a>

                    // Right side: query counter + signup + reset
                    <div class="flex items-center gap-4">
                        <span class="text-sm text-muted-foreground hidden sm:inline">
                            {move || format!("{} queries remaining", queries_remaining.get())}
                        </span>

                        // Reset button — only shown after first query
                        <Show when=move || has_queries.get()>
                            <button
                                class="text-sm text-muted-foreground hover:text-foreground transition-colors"
                                on:click=handle_reset
                            >
                                "Reset"
                            </button>
                        </Show>

                        <a href="/login">
                            <Button size=ButtonSize::Sm>
                                "Sign Up Free"
                            </Button>
                        </a>
                    </div>
                </div>
            </header>

            // ── Messages area ───────────────────────────────────────────
            <div class="flex-1 overflow-y-auto" id="trial-messages-container">
                <div class="max-w-4xl mx-auto px-4 py-6">
                    <Show
                        when=move || has_messages.get()
                        fallback=move || view! {
                            <WelcomeState
                                is_initializing=is_initializing
                                is_loading=is_loading
                                on_suggest=on_suggest.clone()
                            />
                        }
                    >
                        // Message list
                        <div class="w-full space-y-6">
                            <For
                                each=move || messages.get()
                                key=|msg| msg.id.clone()
                                let:msg
                            >
                                <MessageBubble message=msg />
                            </For>
                        </div>
                    </Show>

                    // Scroll anchor
                    <div id="trial-messages-end"></div>
                </div>
            </div>

            // ── Error display ───────────────────────────────────────────
            <Show when=move || error.get().is_some()>
                <div class="flex-shrink-0 px-4 pb-2">
                    <div class="max-w-4xl mx-auto">
                        <div class="bg-error text-error-foreground border border-error-border rounded-lg px-4 py-3 text-sm">
                            {move || error.get().unwrap_or_default()}
                        </div>
                    </div>
                </div>
            </Show>

            // ── Input area ──────────────────────────────────────────────
            <footer class="flex-shrink-0 border-t border-border bg-background px-4 py-3">
                <div class="max-w-4xl mx-auto">
                    <form on:submit=on_submit class="flex gap-2">
                        <input
                            type="text"
                            class="flex-1 px-4 py-2 border border-input rounded-lg focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 bg-background text-foreground"
                            placeholder=input_placeholder
                            disabled=input_disabled
                            prop:value=move || input_value.get()
                            on:input=move |ev| {
                                set_input_value.set(event_target_value(&ev));
                            }
                        />
                        // Native <button> for reactive disabled binding
                        // (the Button component takes static `bool`, not `Signal<bool>`)
                        <button
                            type="submit"
                            class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50 bg-primary text-primary-foreground shadow hover:bg-primary/90 h-9 px-4 py-2"
                            disabled=send_disabled
                        >
                            {move || {
                                if is_loading.get() {
                                    view! { <Spinner /> }.into_any()
                                } else {
                                    view! { "Send" }.into_any()
                                }
                            }}
                        </button>
                    </form>

                    // Query usage footer — shown after first query
                    <Show when=move || has_queries.get()>
                        <div class="flex justify-between items-center mt-2 text-xs text-muted-foreground">
                            <button
                                class="hover:text-foreground transition-colors"
                                on:click=handle_reset
                            >
                                "Reset conversation"
                            </button>
                            <span>
                                {move || format!("{}/5 queries used", query_count.get())}
                            </span>
                        </div>
                    </Show>
                </div>
            </footer>

            // ── Signup modal ────────────────────────────────────────────
            <SignupPromptModal show=show_signup_modal />
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Utility functions
// ─────────────────────────────────────────────────────────────────────────────

/// Scroll the messages container to the bottom.
///
/// Schedules a microtask via `leptos::task::spawn_local` so the DOM has
/// been updated before we scroll.
fn scroll_to_bottom() {
    #[cfg(target_arch = "wasm32")]
    {
        // spawn_local runs after the current reactive update completes,
        // giving the DOM time to render the new message.
        leptos::task::spawn_local(async {
            if let Some(el) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("trial-messages-end"))
            {
                el.scroll_into_view();
            }
        });
    }
}

/// Get the user's current timezone as an RFC 3339 string.
///
/// Returns `None` on the server or if the browser API is unavailable.
fn get_user_timezone() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        // Get the current time in ISO format with timezone offset
        let date = js_sys::Date::new_0();
        Some(date.to_iso_string().as_string().unwrap_or_default())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}
