// SPDX-License-Identifier: AGPL-3.0-or-later

//! Personal setup wizard — first-run onboarding for desktop/personal mode.
//!
//! Two-step wizard:
//! 1. **Connect Data** — shown when no datasources exist. Offers "Connect a
//!    Database" (navigates to `/onboarding`) or "Explore with Sample Data"
//!    (navigates to `/`).
//! 2. **Connect Your AI Tool** — shown once datasources exist. Tabbed
//!    interface for Claude Code, Claude Desktop, and Cursor MCP connection
//!    instructions with copy-to-clipboard support.
//!
//! Mirrors `apps/frontend/src/pages/PersonalSetupWizard.jsx`.

use leptos::prelude::*;
use phosphor_leptos::Icon;
use leptos_router::hooks::use_navigate;

use crate::components::{
    Button, ButtonVariant, Card, CardContent, CardDescription, CardHeader, CardTitle, Spinner,
};
use crate::server_fns::setup::check_has_datasources;

// ─── Page Component ──────────────────────────────────────────────────────────

/// Personal setup wizard page.
#[component]
pub fn PersonalSetupPage() -> impl IntoView {
    let has_datasources = Resource::new(|| (), |_| check_has_datasources());

    view! {
        <Transition fallback=move || view! { <LoadingState/> }>
            {move || {
                has_datasources.get().map(|result| {
                    let has_ds = result.unwrap_or(false);
                    if has_ds {
                        view! { <ConnectAiToolStep/> }.into_any()
                    } else {
                        view! { <ConnectDataStep/> }.into_any()
                    }
                })
            }}
        </Transition>
    }
}

// ─── Loading State ───────────────────────────────────────────────────────────

/// Full-screen centered loading spinner shown while the server function resolves.
#[component]
fn LoadingState() -> impl IntoView {
    view! {
        <div class="min-h-screen bg-background flex items-center justify-center">
            <Spinner class="text-muted-foreground"/>
        </div>
    }
}

// ─── Step 1: Connect Data ────────────────────────────────────────────────────

/// Step 1 — shown when no datasources exist. Offers two options:
/// connect a real database or skip with sample data.
#[component]
fn ConnectDataStep() -> impl IntoView {
    let navigate = use_navigate();
    let nav_onboarding = {
        let navigate = navigate.clone();
        move |_| {
            navigate("/onboarding", Default::default());
        }
    };
    let nav_home = move |_| {
        navigate("/", Default::default());
    };

    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card class="max-w-xl w-full">
                <CardHeader class="text-center">
                    <CardTitle class="text-xl">"Connect Your Data"</CardTitle>
                    <CardDescription>
                        "Kyomi works best when it can query your data directly. Choose how to get started."
                    </CardDescription>
                </CardHeader>
                <CardContent class="space-y-4">
                    // Option 1: Connect a Database
                    <div class="border border-border rounded-lg p-5">
                        <div class="flex items-start gap-4">
                            <Icon
                                icon=phosphor_leptos::DATABASE
                                attr:class="mt-0.5 text-muted-foreground flex-shrink-0"
                            />
                            <div class="flex-1">
                                <h3 class="font-semibold mb-1">"Connect a Database"</h3>
                                <p class="text-sm text-muted-foreground mb-3">
                                    "Connect your database to ask questions about your real data"
                                </p>
                                <Button class="w-full" on:click=nav_onboarding>
                                    "Connect Datasource"
                                </Button>
                            </div>
                        </div>
                    </div>

                    // Option 2: Explore with Sample Data
                    <div class="border border-border rounded-lg p-5">
                        <div class="flex items-start gap-4">
                            <Icon
                                icon=phosphor_leptos::DATABASE
                                attr:class="mt-0.5 text-muted-foreground flex-shrink-0"
                            />
                            <div class="flex-1">
                                <h3 class="font-semibold mb-1">"Explore with Sample Data"</h3>
                                <p class="text-sm text-muted-foreground mb-3">
                                    "Skip setup and explore Kyomi right away"
                                </p>
                                <Button variant=ButtonVariant::Outline class="w-full" on:click=nav_home>
                                    "Skip for Now"
                                </Button>
                            </div>
                        </div>
                    </div>

                    <p class="text-xs text-center text-muted-foreground pt-2">
                        "You can always add datasources later in Settings"
                    </p>
                </CardContent>
            </Card>
        </div>
    }
}

// ─── Step 2: Connect Your AI Tool ────────────────────────────────────────────

/// Tab identifiers for the AI tool connection tabs.
#[derive(Clone, Copy, PartialEq)]
enum AiToolTab {
    ClaudeCode,
    ClaudeDesktop,
    Cursor,
}

impl AiToolTab {
    fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Cursor => "Cursor",
        }
    }
}

const TABS: [AiToolTab; 3] = [
    AiToolTab::ClaudeCode,
    AiToolTab::ClaudeDesktop,
    AiToolTab::Cursor,
];

/// Step 2 — shown when datasources exist. Tabbed MCP connection instructions.
#[component]
fn ConnectAiToolStep() -> impl IntoView {
    let navigate = use_navigate();
    let (active_tab, set_active_tab) = signal(AiToolTab::ClaudeCode);

    // Compute MCP URL from browser location
    let mcp_url = compute_mcp_url();
    let nav_dashboards = {
        let navigate = navigate.clone();
        move |_| {
            navigate("/dashboards", Default::default());
        }
    };
    let nav_settings = move |_| {
        navigate("/settings", Default::default());
    };

    view! {
        <div class="min-h-screen flex items-center justify-center bg-background p-4">
            <Card class="max-w-2xl w-full">
                <CardHeader class="text-center">
                    <CardTitle class="text-xl">"Connect Kyomi to Your AI Tool"</CardTitle>
                    <CardDescription>
                        "Add Kyomi as an MCP server so your AI assistant can query your data."
                    </CardDescription>
                </CardHeader>
                <CardContent class="space-y-6">
                    // Tab bar
                    <div class="flex border-b border-border">
                        {TABS.map(|tab| {
                            let is_active = move || active_tab.get() == tab;
                            view! {
                                <button
                                    class=move || {
                                        let base = "px-4 py-2 text-sm font-medium transition-colors -mb-px";
                                        if is_active() {
                                            format!("{base} border-b-2 border-primary text-foreground")
                                        } else {
                                            format!("{base} text-muted-foreground hover:text-foreground")
                                        }
                                    }
                                    on:click=move |_| set_active_tab.set(tab)
                                >
                                    {tab.label()}
                                </button>
                            }
                        }).collect_view()}
                    </div>

                    // Tab content
                    {
                        let mcp_url = mcp_url.clone();
                        move || {
                            let url = mcp_url.clone();
                            match active_tab.get() {
                                AiToolTab::ClaudeCode => claude_code_tab(&url).into_any(),
                                AiToolTab::ClaudeDesktop => claude_desktop_tab(&url).into_any(),
                                AiToolTab::Cursor => cursor_tab(&url).into_any(),
                            }
                        }
                    }

                    // Actions
                    <div class="space-y-3 pt-2">
                        <Button class="w-full" on:click=nav_dashboards>
                            "I've Connected"
                            <Icon icon=phosphor_leptos::ARROW_RIGHT attr:class="ml-2"/>
                        </Button>
                        <p class="text-sm text-center">
                            <button
                                class="text-muted-foreground hover:text-foreground transition-colors"
                                on:click=nav_settings
                            >
                                "Or use Kyomi\u{2019}s built-in chat instead \u{2192}"
                            </button>
                        </p>
                    </div>
                </CardContent>
            </Card>
        </div>
    }
}

// ─── Tab Content ─────────────────────────────────────────────────────────────

/// Claude Code tab — CLI command + JSON config block.
fn claude_code_tab(mcp_url: &str) -> impl IntoView {
    let cli_command = format!("claude mcp add --transport http kyomi {mcp_url}");
    let config_json = format!(
        "{{\n  \"mcpServers\": {{\n    \"kyomi\": {{\n      \"type\": \"http\",\n      \"url\": \"{mcp_url}\"\n    }}\n  }}\n}}"
    );

    view! {
        <div class="space-y-4">
            <div class="space-y-2">
                <h4 class="font-medium text-foreground text-sm">"Run this command:"</h4>
                <CodeBlock text=cli_command/>
            </div>
            <div class="space-y-2">
                <h4 class="font-medium text-foreground text-sm">"Or add to your config:"</h4>
                <CodeBlock text=config_json/>
            </div>
        </div>
    }
}

/// Claude Desktop tab — JSON config for `claude_desktop_config.json`.
fn claude_desktop_tab(mcp_url: &str) -> impl IntoView {
    let config_json = format!(
        "{{\n  \"mcpServers\": {{\n    \"kyomi\": {{\n      \"url\": \"{mcp_url}\"\n    }}\n  }}\n}}"
    );

    view! {
        <div class="space-y-2">
            <h4 class="font-medium text-foreground text-sm">
                "Add to "
                <code class="text-xs bg-muted px-1 py-0.5 rounded-md">"claude_desktop_config.json"</code>
                ":"
            </h4>
            <CodeBlock text=config_json/>
        </div>
    }
}

/// Cursor tab — one-click deep link + manual JSON config.
fn cursor_tab(mcp_url: &str) -> impl IntoView {
    let deep_link = build_cursor_deep_link(mcp_url);
    let manual_config = format!(
        "{{\n  \"kyomi\": {{\n    \"type\": \"http\",\n    \"url\": \"{mcp_url}\"\n  }}\n}}"
    );

    view! {
        <div class="space-y-4">
            <div class="space-y-2">
                <h4 class="font-medium text-foreground text-sm">"One-click install:"</h4>
                <a
                    href=deep_link
                    target="_blank"
                    rel="noopener noreferrer"
                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground h-9 px-4 py-2"
                >
                    <Icon icon=phosphor_leptos::ARROW_SQUARE_OUT/>
                    "Connect with Cursor"
                </a>
            </div>
            <div class="space-y-2">
                <h4 class="font-medium text-foreground text-sm">
                    "Or add manually to "
                    <code class="text-xs bg-muted px-1 py-0.5 rounded-md">".cursor/mcp.json"</code>
                    ":"
                </h4>
                <CodeBlock text=manual_config/>
            </div>
        </div>
    }
}

// ─── Shared Components ───────────────────────────────────────────────────────

/// Code block with a copy-to-clipboard button. Matches the React `<pre>` + `<Button ghost>`
/// pattern from `PersonalSetupWizard.jsx`.
#[component]
fn CodeBlock(text: String) -> impl IntoView {
    let (copied, set_copied) = signal(false);
    let text_for_click = text.clone();

    let on_click = move |_| {
        let text = text_for_click.clone();
        let set_copied = set_copied;

        #[cfg(target_arch = "wasm32")]
        {
            leptos::task::spawn_local(async move {
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let promise = clipboard.write_text(&text);
                    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
                    set_copied.try_set(true);
                    gloo_timers::future::TimeoutFuture::new(2000).await;
                    set_copied.try_set(false);
                }
            });
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (text, set_copied);
        }
    };

    view! {
        <div class="relative">
            <pre class="p-4 bg-muted rounded-md text-sm overflow-x-auto pr-12">
                {text}
            </pre>
            <button
                class="absolute top-2 right-2 p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors"
                on:click=on_click
                title="Copy to clipboard"
            >
                {move || {
                    if copied.get() {
                        view! { <Icon icon=phosphor_leptos::CLIPBOARD_TEXT/> }.into_any()
                    } else {
                        view! { <Icon icon=phosphor_leptos::COPY/> }.into_any()
                    }
                }}
            </button>
        </div>
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

/// Compute the MCP URL from the browser's current port.
/// Falls back to port 3000 if not in a WASM context or port is empty.
fn compute_mcp_url() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        let port = web_sys::window()
            .and_then(|w| w.location().port().ok())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "3000".to_string());
        format!("http://localhost:{port}/mcp")
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        "http://localhost:3000/mcp".to_string()
    }
}

/// Build the Cursor one-click deep link URL.
///
/// The config payload is base64-encoded JSON: `{"type": "http", "url": "<mcp_url>"}`.
/// Uses standard base64 (not URL-safe) to match `btoa()` from the React source.
fn build_cursor_deep_link(mcp_url: &str) -> String {
    use base64::Engine;
    let config_json = format!(r#"{{"type":"http","url":"{mcp_url}"}}"#);
    let encoded = base64::engine::general_purpose::STANDARD.encode(config_json.as_bytes());
    format!("cursor://anysphere.cursor-deeplink/mcp/install?name=kyomi&config={encoded}")
}
