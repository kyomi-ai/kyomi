// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback modal — lets users submit bug reports, feature requests, or questions.
//!
//! Matches the React `FeedbackModal` component. Uses the standard `Modal` shell
//! with a type selector (pill buttons), description textarea, and submit button.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::alert::{Alert, AlertDescription, AlertVariant};
use crate::components::button::{Button, ButtonVariant};
use crate::components::modal::{Modal, ModalSize};
use crate::server_fns::feedback::submit_feedback;

/// Feedback type options matching the backend's allowed values.
const FEEDBACK_TYPES: &[(&str, &str, phosphor_leptos::IconData)] = &[
    ("bug", "Bug", phosphor_leptos::BUG),
    ("feature", "Feature Request", phosphor_leptos::LIGHTBULB),
    ("question", "Question", phosphor_leptos::QUESTION),
];

/// Textarea class — based on INPUT_CLASS but adapted for multi-line input.
const TEXTAREA_CLASS: &str = "w-full min-h-[120px] resize-y bg-transparent border border-input rounded-md px-3 py-2 text-sm text-foreground shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring md:text-sm";

/// Active pill button class — matches FilterButton active state from chat_list.rs.
const PILL_ACTIVE: &str = "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground";

/// Inactive pill button class — matches FilterButton inactive state from chat_list.rs.
const PILL_INACTIVE: &str = "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-secondary text-foreground border border-border hover:bg-secondary/80";

/// Modal for submitting user feedback.
///
/// Shows a type selector (Bug / Feature Request / Question), a description
/// textarea, and a submit button. On success, displays a thank-you message
/// and auto-closes after 1.5 seconds.
#[component]
pub fn FeedbackModal(
    /// Whether the modal is visible.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when the modal should close.
    on_close: Callback<()>,
) -> impl IntoView {
    // Form state
    let (feedback_type, set_feedback_type) = signal("bug".to_string());
    let (description, set_description) = signal(String::new());
    let (submitting, set_submitting) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);
    let (success, set_success) = signal(false);

    // Description must be >= 10 chars to enable submit
    let can_submit = Memo::new(move |_| {
        let desc = description.get();
        desc.trim().len() >= 10 && !submitting.get()
    });

    // Reset form state when modal opens
    Effect::new(move |_| {
        if open.get() {
            set_feedback_type.set("bug".to_string());
            set_description.set(String::new());
            set_error.set(None);
            set_success.set(false);
            set_submitting.set(false);
        }
    });

    // Submit action
    let submit = Action::new(move |(ft, desc): &(String, String)| {
        let ft = ft.clone();
        let desc = desc.clone();
        async move { submit_feedback(ft, desc).await }
    });

    // Handle submit result
    Effect::new(move |_| {
        if let Some(result) = submit.value().get() {
            set_submitting.set(false);
            match result {
                Ok(_) => {
                    set_success.set(true);
                    // Auto-close after 1.5 seconds
                    let close = on_close;
                    set_timeout(
                        move || {
                            close.run(());
                        },
                        std::time::Duration::from_millis(1500),
                    );
                }
                Err(e) => {
                    set_error.set(Some(e.to_string()));
                }
            }
        }
    });

    // Close handler that resets state
    let handle_close = Callback::new(move |()| {
        on_close.run(());
    });

    view! {
        <Modal
            show=open
            on_close=handle_close
            title="Send Feedback"
            size=ModalSize::Md
        >
            <Show
                when=move || !success.get()
                fallback=move || view! {
                    // Success state
                    <div class="flex flex-col items-center justify-center py-8 gap-3">
                        <div class="w-12 h-12 rounded-full bg-success/20 flex items-center justify-center">
                            <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Fill size="28px" attr:class="text-success-foreground"/>
                        </div>
                        <p class="text-sm text-foreground font-medium">"Thank you for your feedback!"</p>
                    </div>
                }
            >
                <div class="space-y-4">
                    // Type selector — pill buttons
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Type"</label>
                        <div class="flex gap-2">
                            {FEEDBACK_TYPES.iter().map(|(value, label, icon)| {
                                let value = *value;
                                let label = *label;
                                let icon = *icon;
                                view! {
                                    <button
                                        type="button"
                                        class=move || {
                                            if feedback_type.get() == value {
                                                PILL_ACTIVE
                                            } else {
                                                PILL_INACTIVE
                                            }
                                        }
                                        on:click=move |_| set_feedback_type.set(value.to_string())
                                    >
                                        <Icon icon=icon weight=IconWeight::Regular size="16px"/>
                                        {label}
                                    </button>
                                }
                            }).collect_view()}
                        </div>
                    </div>

                    // Description textarea
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Description"</label>
                        <textarea
                            class=TEXTAREA_CLASS
                            placeholder="Describe your feedback in detail (minimum 10 characters)..."
                            prop:value=move || description.get()
                            on:input=move |ev| {
                                set_description.set(event_target_value(&ev));
                                // Clear error when user starts typing
                                set_error.set(None);
                            }
                        />
                        <p class="mt-1 text-xs text-muted-foreground">
                            {move || {
                                let len = description.get().trim().len();
                                if len < 10 {
                                    format!("{} more character{} needed", 10 - len, if 10 - len == 1 { "" } else { "s" })
                                } else {
                                    format!("{len} characters")
                                }
                            }}
                        </p>
                    </div>

                    // Error message
                    <Show when=move || error.get().is_some()>
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </Show>

                    // Submit button
                    <div class="flex justify-end">
                        <Button
                            variant=ButtonVariant::Default
                            disabled=MaybeProp::derive(move || Some(!can_submit.get()))
                            on:click=move |_| {
                                set_submitting.set(true);
                                set_error.set(None);
                                submit.dispatch((feedback_type.get(), description.get()));
                            }
                        >
                            {move || if submitting.get() { "Sending..." } else { "Send Feedback" }}
                        </Button>
                    </div>
                </div>
            </Show>
        </Modal>
    }
}
