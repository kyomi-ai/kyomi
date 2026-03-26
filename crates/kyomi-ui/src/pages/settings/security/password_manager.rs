// SPDX-License-Identifier: AGPL-3.0-or-later

//! Password Manager card — security settings section for password management.
//!
//! Replaces `apps/frontend/src/components/PasswordManager.jsx` (250 lines).
//!
//! Shows:
//! - If user has no password: "Set Password" form (new password + confirm)
//! - If user has password: "Change Password" form (current + new + confirm)
//! - Password validation (min 8 chars, match confirmation)
//! - Show/hide toggle on password fields
//! - Success/error feedback

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, Label, Skeleton, INPUT_CLASS,
};
use crate::server_fns::security::{change_password, has_password, set_password};

/// Password field with show/hide toggle button.
///
/// Matches the React pattern: relative container with input + absolute-positioned
/// ghost icon button on the right.
#[component]
fn PasswordField(
    /// Label text displayed above the field.
    label: &'static str,
    /// HTML id and name for the input.
    id: &'static str,
    /// Placeholder text.
    placeholder: &'static str,
    /// Two-way binding for the field value.
    value: RwSignal<String>,
    /// Whether the password is currently visible.
    visible: RwSignal<bool>,
    /// Whether to enforce `minlength="8"` on the input. Should be false for
    /// current-password fields (legacy passwords may be shorter).
    #[prop(optional)]
    enforce_min_length: bool,
) -> impl IntoView {
    let input_type = move || if visible.get() { "text" } else { "password" };
    let min_attr = if enforce_min_length { Some("8") } else { None };

    view! {
        <div class="space-y-2">
            <Label html_for=id>{label}</Label>
            <div class="relative">
                <input
                    type=input_type
                    id=id
                    name=id
                    class=format!("{INPUT_CLASS} pr-12")
                    placeholder=placeholder
                    required
                    minlength=min_attr
                    prop:value=move || value.get()
                    on:input=move |ev| {
                        value.set(event_target_value(&ev));
                    }
                />
                <button
                    type="button"
                    class="absolute right-1 top-1/2 -translate-y-1/2 h-7 w-7 inline-flex items-center justify-center rounded-md text-foreground hover:bg-accent hover:text-accent-foreground transition-colors"
                    on:click=move |_| visible.update(|v| *v = !*v)
                >
                    <Show
                        when=move || visible.get()
                        fallback=|| view! { <Icon icon=icondata_lu::LuEye width="16" height="16"/> }
                    >
                        <Icon icon=icondata_lu::LuEyeOff width="16" height="16"/>
                    </Show>
                </button>
            </div>
        </div>
    }
}

/// Password Manager component.
///
/// Loads `has_password` on mount, then shows either a summary card with
/// a "Change Password" / "Set Password" button, or the corresponding form.
#[component]
pub fn PasswordManager() -> impl IntoView {
    // ── Server data ──────────────────────────────────────────────────────
    let has_pw = Resource::new(|| (), |_| has_password());

    // ── Form state ───────────────────────────────────────────────────────
    let editing = RwSignal::new(false);
    let current_password = RwSignal::new(String::new());
    let new_password = RwSignal::new(String::new());
    let confirm_password = RwSignal::new(String::new());
    let show_current = RwSignal::new(false);
    let show_new = RwSignal::new(false);
    let show_confirm = RwSignal::new(false);
    let loading = RwSignal::new(false);
    let error = RwSignal::new(Option::<String>::None);
    let success = RwSignal::new(Option::<String>::None);

    let reset_form = move || {
        current_password.set(String::new());
        new_password.set(String::new());
        confirm_password.set(String::new());
        show_current.set(false);
        show_new.set(false);
        show_confirm.set(false);
        error.set(None);
        success.set(None);
    };

    let handle_cancel = move |_| {
        editing.set(false);
        reset_form();
    };

    let handle_submit = move |ev: leptos::ev::SubmitEvent| {
        ev.prevent_default();
        error.set(None);
        success.set(None);

        let new_pw = new_password.get();
        let confirm_pw = confirm_password.get();

        // Client-side validation
        if new_pw != confirm_pw {
            error.set(Some("New passwords do not match".to_string()));
            return;
        }
        if new_pw.len() < 8 {
            error.set(Some(
                "Password must be at least 8 characters long".to_string(),
            ));
            return;
        }

        loading.set(true);

        let current_pw = current_password.get();

        leptos::task::spawn_local(async move {
            let user_has_pw = has_pw
                .get()
                .and_then(|r| r.ok())
                .unwrap_or(false);

            let result = if user_has_pw {
                change_password(current_pw, new_pw).await
            } else {
                set_password(new_pw).await
            };

            loading.set(false);

            match result {
                Ok(message) => {
                    success.set(Some(message));
                    current_password.set(String::new());
                    new_password.set(String::new());
                    confirm_password.set(String::new());
                    show_current.set(false);
                    show_new.set(false);
                    show_confirm.set(false);
                    editing.set(false);
                    // Refetch to update the has_password state
                    has_pw.refetch();
                }
                Err(e) => {
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    // ── Derived signals ──────────────────────────────────────────────────
    let user_has_password = move || {
        has_pw
            .get()
            .and_then(|r| r.ok())
            .unwrap_or(false)
    };

    view! {
        <Transition fallback=|| view! { <Skeleton class="h-24 w-full" /> }>
            <Show
                when=move || editing.get()
                fallback=move || {
                    // ── Summary card ─────────────────────────────────────
                    view! {
                        {move || success.get().map(|msg| view! {
                            <Alert variant=AlertVariant::Success>
                                <AlertDescription>{msg}</AlertDescription>
                            </Alert>
                        })}
                        <Card>
                            <CardHeader>
                                <div class="flex items-center justify-between">
                                    <div>
                                        <CardTitle>"Password"</CardTitle>
                                        <CardDescription>
                                            {move || {
                                                if user_has_password() {
                                                    "Change your account password"
                                                } else {
                                                    "Add password authentication to your account"
                                                }
                                            }}
                                        </CardDescription>
                                    </div>
                                    <Button on:click=move |_| editing.set(true)>
                                        {move || {
                                            if user_has_password() {
                                                view! { <span>"Change Password"</span> }.into_any()
                                            } else {
                                                view! {
                                                    <Icon icon=icondata_lu::LuPlus width="16" height="16"/>
                                                    <span>"Set Password"</span>
                                                }
                                                    .into_any()
                                            }
                                        }}
                                    </Button>
                                </div>
                            </CardHeader>
                        </Card>
                    }
                }
            >
                // ── Edit form card ───────────────────────────────────
                <Card>
                    <CardHeader>
                        <CardTitle>
                            {move || {
                                if user_has_password() {
                                    "Change Password"
                                } else {
                                    "Set Password"
                                }
                            }}
                        </CardTitle>
                        <CardDescription>
                            {move || {
                                if user_has_password() {
                                    "Enter your current password and choose a new one"
                                } else {
                                    "Create a password for your account"
                                }
                            }}
                        </CardDescription>
                    </CardHeader>
                    <CardContent>
                        <form on:submit=handle_submit class="space-y-4">
                            // Error alert
                            {move || {
                                error
                                    .get()
                                    .map(|msg| {
                                        view! {
                                            <Alert variant=AlertVariant::Error>
                                                <AlertDescription>{msg}</AlertDescription>
                                            </Alert>
                                        }
                                    })
                            }}

                            // Success alert
                            {move || {
                                success
                                    .get()
                                    .map(|msg| {
                                        view! {
                                            <Alert variant=AlertVariant::Success>
                                                <AlertDescription>{msg}</AlertDescription>
                                            </Alert>
                                        }
                                    })
                            }}

                            // Current password field (only if user has a password)
                            <Show when=move || user_has_password()>
                                <PasswordField
                                    label="Current Password"
                                    id="currentPassword"
                                    placeholder="Enter current password"
                                    value=current_password
                                    visible=show_current
                                />
                            </Show>

                            // New password field
                            <PasswordField
                                label="New Password"
                                id="newPassword"
                                placeholder="Enter new password (min 8 characters)"
                                value=new_password
                                visible=show_new
                                enforce_min_length=true
                            />

                            // Confirm password field
                            <PasswordField
                                label="Confirm New Password"
                                id="confirmPassword"
                                placeholder="Confirm new password"
                                value=confirm_password
                                visible=show_confirm
                                enforce_min_length=true
                            />

                            // Action buttons
                            <div class="flex gap-3 pt-2">
                                <Button
                                    attr:r#type="submit"
                                    attr:disabled=move || loading.get()
                                >
                                    {move || {
                                        if loading.get() {
                                            if user_has_password() {
                                                "Changing..."
                                            } else {
                                                "Setting..."
                                            }
                                        } else if user_has_password() {
                                            "Change Password"
                                        } else {
                                            "Set Password"
                                        }
                                    }}
                                </Button>
                                <Button
                                    variant=ButtonVariant::Outline
                                    attr:r#type="button"
                                    attr:disabled=move || loading.get()
                                    on:click=handle_cancel
                                >
                                    "Cancel"
                                </Button>
                            </div>
                        </form>
                    </CardContent>
                </Card>
            </Show>
        </Transition>
    }
}
