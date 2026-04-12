// SPDX-License-Identifier: AGPL-3.0-or-later

//! AI settings page — workspace-level AI configuration and BYOK.
//!
//! This tab is the single home for AI-related settings:
//! - Workspace default model (admin-only, applies to all members)
//! - BYOK provider config (per-user, stored in localStorage)

use leptos::prelude::*;
use leptos_icons::Icon;

use crate::components::{
    Alert, AlertDescription, AlertVariant, Button, ButtonSize, ButtonVariant, Card, CardContent,
    CardDescription, CardHeader, CardTitle, Label, Skeleton, INPUT_CLASS,
};


use crate::components::toast::{toast_error, toast_success};
use crate::pages::settings::ai_provider::AiProviderCard;
use crate::server_fns::workspace::{get_workspace_settings, update_workspace_model};

/// Workspace model options shown in the dropdown.
///
/// Free-form input is allowed — users can type any model string supported
/// by their chosen provider. These are just suggested defaults.
const MODEL_SUGGESTIONS: &[(&str, &str)] = &[
    ("claude-sonnet-4-20250514", "Claude Sonnet 4"),
    ("claude-opus-4-20250514", "Claude Opus 4"),
    ("gpt-4o", "GPT-4o"),
    ("gpt-4o-mini", "GPT-4o mini"),
    ("gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("gemini-2.5-flash", "Gemini 2.5 Flash"),
];

#[component]
pub fn AiPage() -> impl IntoView {
    view! {
        <div class="p-4 sm:p-6 space-y-6">
            <h2 class="text-xl font-display text-foreground mb-6">"AI"</h2>

            <WorkspaceModelCard/>

            <Alert variant=AlertVariant::Info>
                <AlertDescription>
                    "Your API key is stored in this browser only — it does not sync across devices or team members. Workspace-level BYOK is coming soon."
                </AlertDescription>
            </Alert>

            <AiProviderCard/>
        </div>
    }
}

/// Admin-only card to set the workspace default model.
#[component]
fn WorkspaceModelCard() -> impl IntoView {
    let (version, set_version) = signal(0u32);
    let settings = LocalResource::new(move || {
        let _ = version.get();
        get_workspace_settings()
    });

    view! {
        <Card>
            <CardHeader>
                <CardTitle class="flex items-center gap-2">
                    <Icon icon=icondata_lu::LuCpu width="20" height="20"/>
                    "Workspace Default Model"
                </CardTitle>
                <CardDescription>
                    "The model used for all AI requests in this workspace. Individual members can override with their own provider below."
                </CardDescription>
            </CardHeader>
            <CardContent>
                <Transition fallback=|| view! {
                    <Skeleton class="h-10 w-full"/>
                }>
                    {move || Suspend::new(async move {
                        match settings.await {
                            Ok(data) => view! {
                                <ModelSelector
                                    current=data.default_model.clone()
                                    on_saved=Callback::new(move |_| {
                                        set_version.update(|v| *v += 1);
                                    })
                                />
                            }.into_any(),
                            Err(e) => {
                                let msg = e.to_string();
                                let is_admin_err = msg.contains("admin");
                                view! {
                                    <Alert variant=AlertVariant::Info>
                                        <AlertDescription>
                                            {if is_admin_err {
                                                "Only workspace admins can change the default model.".to_string()
                                            } else {
                                                format!("Failed to load settings: {msg}")
                                            }}
                                        </AlertDescription>
                                    </Alert>
                                }.into_any()
                            }
                        }
                    })}
                </Transition>
            </CardContent>
        </Card>
    }
}

#[component]
fn ModelSelector(current: String, on_saved: Callback<()>) -> impl IntoView {
    let (model_value, set_model_value) = signal(current);
    let (saving, set_saving) = signal(false);

    let save_action = Action::new(move |model: &String| {
        let model = model.clone();
        async move {
            set_saving.set(true);
            let result = update_workspace_model(model).await;
            set_saving.set(false);
            match result {
                Ok(()) => {
                    toast_success("Default model updated".to_string());
                    on_saved.run(());
                }
                Err(e) => toast_error(format!("Failed to save: {e}")),
            }
        }
    });

    view! {
        <div class="space-y-3">
            <Label>"Model identifier"</Label>
            <div class="flex gap-2">
                <input
                    type="text"
                    class=INPUT_CLASS
                    prop:value=model_value
                    on:input=move |ev| set_model_value.set(event_target_value(&ev))
                    placeholder="claude-sonnet-4-20250514"
                />
                <Button
                    variant=ButtonVariant::Default
                    disabled=Signal::derive(move || saving.get())
                    on:click=move |_| {
                        save_action.dispatch(model_value.get_untracked());
                    }
                >
                    {move || if saving.get() { "Saving..." } else { "Save" }}
                </Button>
            </div>

            // Suggestions
            <div>
                <p class="text-xs text-muted-foreground mb-2">"Suggested models:"</p>
                <div class="flex flex-wrap gap-2">
                    {MODEL_SUGGESTIONS.iter().map(|(id, label)| {
                        let id = id.to_string();
                        let label = label.to_string();
                        view! {
                            <Button
                                variant=ButtonVariant::Secondary
                                size=ButtonSize::Sm
                                on:click=move |_| set_model_value.set(id.clone())
                            >
                                {label}
                            </Button>
                        }
                    }).collect_view()}
                </div>
            </div>
        </div>
    }
}
