// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared deployment command generators and UI primitives for Kyomi Connect.
//!
//! Ports `apps/frontend/src/components/settings/datasources/shared/components/
//! connectDeploymentCommands.js`. Consumed by both the create flow
//! (`ConnectCreateSuccessView` in `datasources.rs`) and the status/edit flow
//! (`ConnectStatusPanel`) — they both need, given a datasource type + port +
//! optional token, the four install commands to render across the Linux /
//! Docker / Kubernetes / Compose tabs.
//!
//! Command strings are **byte-equivalent** to the React version — users
//! may have muscle memory for them, so no reformatting, no re-ordering
//! flags, no "cleaner" whitespace.
//!
//! This module also owns the **shared UI primitives** both call sites
//! render around those command strings:
//! * [`DeploymentTabStrip`] — the horizontal tab bar of Linux/Docker/k8s/
//!   Compose buttons.
//! * [`CopyButton`] — the small clipboard-copy button (phosphor COPY → CHECK
//!   with a 2s flash) used for both the token and the active command block.

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

/// A deployment tab descriptor: stable id for keyed lists + human-readable label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeploymentTab {
    pub id: &'static str,
    pub label: &'static str,
}

/// The four deployment tabs, in the exact order the React UI presents them.
///
/// Order matters: `Linux`, `Docker`, `Kubernetes`, `Compose`. Do not
/// alphabetize — the React UI (and therefore user expectations) present
/// them in this precise sequence.
pub const DEPLOYMENT_TABS: &[DeploymentTab] = &[
    DeploymentTab {
        id: "linux",
        label: "Linux / macOS",
    },
    DeploymentTab {
        id: "docker",
        label: "Docker",
    },
    DeploymentTab {
        id: "kubernetes",
        label: "Kubernetes",
    },
    DeploymentTab {
        id: "compose",
        label: "Compose",
    },
];

/// Default TCP port for a given datasource type.
///
/// Mirrors the `DEFAULT_PORTS` lookup from the JS module. Unknown types
/// fall back to `5432` — same behavior as the JS `|| '5432'` default.
pub const fn default_port(datasource_type: &str) -> u16 {
    // Can't match on &str in a const fn stably, so use byte comparison.
    // Order and values are load-bearing — do not reorder without also
    // updating the JS module they mirror.
    match datasource_type.as_bytes() {
        b"postgres" => 5432,
        b"redshift" => 5432,
        b"mysql" => 3306,
        b"clickhouse" => 8123,
        b"sqlserver" => 1433,
        b"synapse" => 1433,
        _ => 5432,
    }
}

/// Whether a datasource type supports SSH tunneling to reach the database
/// through a bastion host.
///
/// Mirrors `kyomi_core::datasource_registry`'s `supports_ssh_tunnel` flag on
/// `DatasourceTypeMetadata`. `kyomi-core` is an `ssr`-only optional
/// dependency of `kyomi-ui` (see Cargo.toml), so `SshTunnelSection` in
/// `datasources.rs` — which must gate its rendering on the WASM client too —
/// cannot call the registry directly. This const-fn mirror exists for that,
/// following the same pattern as [`default_port`] above. Kept in sync via
/// the `supports_ssh_tunnel_matches_registry` test below (`kyomi-core` is
/// available there as a dev-dependency regardless of feature flags).
pub const fn supports_ssh_tunnel(type_id: &str) -> bool {
    // Can't match on &str in a const fn stably, so use byte comparison —
    // same technique as `default_port`.
    matches!(
        type_id.as_bytes(),
        b"postgres" | b"mysql" | b"redshift" | b"clickhouse" | b"sqlserver"
    )
}

/// Placeholder substituted into command text when no token is available.
///
/// The React `ConnectStatus` passes this literal string in place of a real
/// token before the user has provisioned one. Matching it exactly means
/// copy-pasted commands render identically between the two frontends.
pub const TOKEN_PLACEHOLDER: &str = "<YOUR_TOKEN>";

/// The rendered command text for each of the four deployment tabs.
///
/// Keyed by tab id rather than returned as a `Vec<(id, text)>` so the
/// downstream `ConnectStatusPanel` (Task 3) and create-mode flow (Task 4)
/// can pluck out the active tab's text with a field access — no runtime
/// lookup, no panics on missing ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeploymentCommands {
    pub linux: String,
    pub docker: String,
    pub kubernetes: String,
    pub compose: String,
}

impl DeploymentCommands {
    /// Fetch the command text for a tab id (`linux` / `docker` /
    /// `kubernetes` / `compose`). Returns an empty string for unknown ids,
    /// matching the JS `getTabContent` default branch.
    pub fn for_tab(&self, tab_id: &str) -> &str {
        match tab_id {
            "linux" => &self.linux,
            "docker" => &self.docker,
            "kubernetes" => &self.kubernetes,
            "compose" => &self.compose,
            _ => "",
        }
    }
}

/// Resolve the token to render into commands, applying the
/// `<YOUR_TOKEN>` placeholder fallback when the caller hasn't provisioned
/// one yet.
fn resolve_token(token: Option<&str>) -> &str {
    token.unwrap_or(TOKEN_PLACEHOLDER)
}

/// Resolve the port to render into commands, falling back to the
/// datasource-type default when no explicit port is supplied.
fn resolve_port(datasource_type: &str, port: Option<u16>) -> u16 {
    port.unwrap_or_else(|| default_port(datasource_type))
}

fn linux_command(token: Option<&str>) -> String {
    // The JS branches on `token && !token.startsWith('<')` — i.e. a real,
    // non-placeholder token gets the one-shot installer flag; the
    // placeholder (or a missing token) falls through to interactive setup.
    // `None` + the `<YOUR_TOKEN>` placeholder both hit the interactive path.
    match token {
        Some(t) if !t.starts_with('<') => format!(
            "# Install Kyomi Connect and run setup\n\
             curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token \"{t}\""
        ),
        _ => String::from(
            "# Install Kyomi Connect and run interactive setup\n\
             curl -fsSL https://connect.kyomi.ai/install.sh | sh",
        ),
    }
}

fn docker_command(token: &str, port: u16) -> String {
    format!(
        "# Use \"host.docker.internal\" for DB_HOST if your database is on localhost\n\
         docker run -d \\\n  \
           --restart=always \\\n  \
           --name kyomi-connect \\\n  \
           -e KYOMI_TOKEN=\"{token}\" \\\n  \
           -e DB_HOST=\"your-database-host\" \\\n  \
           -e DB_PORT=\"{port}\" \\\n  \
           -e DB_NAME=\"your-database\" \\\n  \
           -e DB_USER=\"your-username\" \\\n  \
           -e DB_PASSWORD=\"your-password\" \\\n  \
           ghcr.io/kyomi-ai/kyomi-connect:latest"
    )
}

fn kubernetes_command(token: &str, port: u16) -> String {
    format!(
        "# Create the token secret\n\
         kubectl create secret generic kyomi-connect-token \\\n  \
           --from-literal=token=\"{token}\"\n\
         \n\
         # Create the database password secret\n\
         kubectl create secret generic kyomi-connect-db \\\n  \
           --from-literal=password=\"your-password\"\n\
         \n\
         # Install with Helm (OCI registry)\n\
         helm install kyomi-connect \\\n  \
           oci://ghcr.io/kyomi-ai/charts/kyomi-connect \\\n  \
           --set existingSecret.name=kyomi-connect-token \\\n  \
           --set target.host=\"your-database-host\" \\\n  \
           --set target.port={port} \\\n  \
           --set target.database=\"your-database\" \\\n  \
           --set target.user=\"your-username\" \\\n  \
           --set target.passwordSecretName=kyomi-connect-db"
    )
}

fn compose_snippet(token: &str, port: u16) -> String {
    format!(
        "# Use \"host.docker.internal\" for DB_HOST if your database is on localhost\n\
         services:\n  \
           kyomi-connect:\n    \
             image: ghcr.io/kyomi-ai/kyomi-connect:latest\n    \
             restart: always\n    \
             environment:\n      \
               KYOMI_TOKEN: \"{token}\"\n      \
               DB_HOST: \"your-database-host\"\n      \
               DB_PORT: \"{port}\"\n      \
               DB_NAME: \"your-database\"\n      \
               DB_USER: \"your-username\"\n      \
               DB_PASSWORD: \"your-password\""
    )
}

/// Build the four deployment commands for a given datasource.
///
/// * `datasource_type` — `postgres`, `mysql`, `clickhouse`, etc. Drives
///   the default port when `port` is `None`.
/// * `token` — the Connect API token, or `None` to substitute the literal
///   `<YOUR_TOKEN>` placeholder exactly where the React version does.
/// * `port` — an explicit TCP port override, or `None` to use
///   [`default_port`] for the datasource type.
pub fn build_deployment_commands(
    datasource_type: &str,
    token: Option<&str>,
    port: Option<u16>,
) -> DeploymentCommands {
    let resolved_token = resolve_token(token);
    let resolved_port = resolve_port(datasource_type, port);

    DeploymentCommands {
        linux: linux_command(token),
        docker: docker_command(resolved_token, resolved_port),
        kubernetes: kubernetes_command(resolved_token, resolved_port),
        compose: compose_snippet(resolved_token, resolved_port),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared UI primitives — the tab strip + copy button rendered by both the
// create-success view (`datasources.rs`) and the edit-mode status panel
// (`connect_status_panel.rs`). Kept here so there's one source of truth for
// the visual treatment — change it once, both call sites update together.
// ─────────────────────────────────────────────────────────────────────────────

/// Horizontal tab strip rendering every [`DEPLOYMENT_TABS`] entry as an
/// underline-active button.
///
/// The visual treatment matches DESIGN.md: active tab gets a `border-primary`
/// underline and `text-foreground`; inactive tabs get a transparent underline
/// and `text-muted-foreground` with a foreground-on-hover.
///
/// * `active_tab` — the currently selected tab id. Driven by the caller so
///   both the tab strip and the sibling code-block stay in sync.
/// * `set_active_tab` — write handle used when the user clicks a tab.
#[component]
pub fn DeploymentTabStrip(
    active_tab: Signal<String>,
    set_active_tab: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="flex border-b border-border">
            {DEPLOYMENT_TABS.iter().map(|tab| {
                let tab_id: &'static str = tab.id;
                let tab_label: &'static str = tab.label;
                let is_active = move || active_tab.get() == tab_id;
                view! {
                    <button
                        type="button"
                        class=move || {
                            if is_active() {
                                "px-4 py-2 text-sm font-medium border-b-2 border-primary text-foreground transition-colors"
                            } else {
                                "px-4 py-2 text-sm font-medium border-b-2 border-transparent text-muted-foreground hover:text-foreground transition-colors"
                            }
                        }
                        on:click=move |_| set_active_tab.set(tab_id.to_string())
                    >
                        {tab_label}
                    </button>
                }
            }).collect_view()}
        </div>
    }
}

/// Small copy-to-clipboard button used throughout the Connect deployment UI.
///
/// Takes a reactive `Signal<String>` so the copied text stays in sync with
/// whatever the parent is currently displaying (e.g. the active deployment
/// tab's command, the freshly-rotated token, or the just-issued create-flow
/// token). Matches the visual treatment of the copy buttons in
/// Profile/Analytics settings — phosphor COPY → CHECK on click, 2s flash,
/// bold-on-flash.
#[component]
pub fn CopyButton(#[prop(into)] text: Signal<String>) -> impl IntoView {
    let (copied, set_copied) = signal(false);

    let on_click = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            let to_copy = text.get_untracked();
            leptos::task::spawn_local(async move {
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let promise = clipboard.write_text(&to_copy);
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
        <button
            type="button"
            class="p-1.5 rounded-md text-muted-foreground hover:text-foreground hover:bg-secondary transition-colors"
            on:click=on_click
            title="Copy to clipboard"
        >
            {move || {
                if copied.get() {
                    view! {
                        <Icon
                            icon=phosphor_leptos::CHECK
                            weight=IconWeight::Bold
                            size="16px"
                        />
                    }.into_any()
                } else {
                    view! {
                        <Icon
                            icon=phosphor_leptos::COPY
                            weight=IconWeight::Light
                            size="16px"
                        />
                    }.into_any()
                }
            }}
        </button>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_tabs_match_react_order() {
        assert_eq!(DEPLOYMENT_TABS.len(), 4);
        assert_eq!(DEPLOYMENT_TABS[0].id, "linux");
        assert_eq!(DEPLOYMENT_TABS[0].label, "Linux / macOS");
        assert_eq!(DEPLOYMENT_TABS[1].id, "docker");
        assert_eq!(DEPLOYMENT_TABS[1].label, "Docker");
        assert_eq!(DEPLOYMENT_TABS[2].id, "kubernetes");
        assert_eq!(DEPLOYMENT_TABS[2].label, "Kubernetes");
        assert_eq!(DEPLOYMENT_TABS[3].id, "compose");
        assert_eq!(DEPLOYMENT_TABS[3].label, "Compose");
    }

    #[test]
    fn default_port_matches_js_table() {
        assert_eq!(default_port("postgres"), 5432);
        assert_eq!(default_port("redshift"), 5432);
        assert_eq!(default_port("mysql"), 3306);
        assert_eq!(default_port("clickhouse"), 8123);
        assert_eq!(default_port("sqlserver"), 1433);
        assert_eq!(default_port("synapse"), 1433);
        // Unknown type falls back to 5432 (JS `|| '5432'`).
        assert_eq!(default_port("bogus"), 5432);
        assert_eq!(default_port(""), 5432);
    }

    #[test]
    fn supports_ssh_tunnel_matches_registry() {
        for (type_id, meta) in kyomi_core::datasource_registry::all_metadata() {
            assert_eq!(
                supports_ssh_tunnel(type_id),
                meta.supports_ssh_tunnel,
                "supports_ssh_tunnel mirror mismatch for {type_id}"
            );
        }
    }

    #[test]
    fn linux_interactive_when_token_missing() {
        let cmds = build_deployment_commands("postgres", None, None);
        assert_eq!(
            cmds.linux,
            "# Install Kyomi Connect and run interactive setup\n\
             curl -fsSL https://connect.kyomi.ai/install.sh | sh"
        );
    }

    #[test]
    fn linux_interactive_when_token_is_placeholder() {
        // JS `!token.startsWith('<')` check — any angle-bracketed placeholder
        // must fall through to interactive setup rather than be embedded in
        // the --token flag.
        let cmds = build_deployment_commands("postgres", Some("<YOUR_TOKEN>"), None);
        assert_eq!(
            cmds.linux,
            "# Install Kyomi Connect and run interactive setup\n\
             curl -fsSL https://connect.kyomi.ai/install.sh | sh"
        );
    }

    #[test]
    fn linux_one_shot_when_real_token() {
        let cmds = build_deployment_commands("postgres", Some("abc123"), None);
        assert_eq!(
            cmds.linux,
            "# Install Kyomi Connect and run setup\n\
             curl -fsSL https://connect.kyomi.ai/install.sh | sh -s -- --token \"abc123\""
        );
    }

    #[test]
    fn docker_substitutes_token_placeholder_when_missing() {
        let cmds = build_deployment_commands("postgres", None, None);
        assert!(cmds.docker.contains("-e KYOMI_TOKEN=\"<YOUR_TOKEN>\""));
        assert!(cmds.docker.contains("-e DB_PORT=\"5432\""));
    }

    #[test]
    fn docker_command_byte_equivalent_to_js() {
        // Locked-in byte-for-byte snapshot of the JS output for
        // (token="tkn", datasourceType="mysql") — port 3306.
        let cmds = build_deployment_commands("mysql", Some("tkn"), None);
        let expected = "# Use \"host.docker.internal\" for DB_HOST if your database is on localhost\n\
                        docker run -d \\\n  --restart=always \\\n  --name kyomi-connect \\\n  -e KYOMI_TOKEN=\"tkn\" \\\n  -e DB_HOST=\"your-database-host\" \\\n  -e DB_PORT=\"3306\" \\\n  -e DB_NAME=\"your-database\" \\\n  -e DB_USER=\"your-username\" \\\n  -e DB_PASSWORD=\"your-password\" \\\n  ghcr.io/kyomi-ai/kyomi-connect:latest";
        assert_eq!(cmds.docker, expected);
    }

    #[test]
    fn kubernetes_command_byte_equivalent_to_js() {
        let cmds = build_deployment_commands("clickhouse", Some("tkn"), None);
        let expected = "# Create the token secret\n\
                        kubectl create secret generic kyomi-connect-token \\\n  --from-literal=token=\"tkn\"\n\
                        \n\
                        # Create the database password secret\n\
                        kubectl create secret generic kyomi-connect-db \\\n  --from-literal=password=\"your-password\"\n\
                        \n\
                        # Install with Helm (OCI registry)\n\
                        helm install kyomi-connect \\\n  oci://ghcr.io/kyomi-ai/charts/kyomi-connect \\\n  --set existingSecret.name=kyomi-connect-token \\\n  --set target.host=\"your-database-host\" \\\n  --set target.port=8123 \\\n  --set target.database=\"your-database\" \\\n  --set target.user=\"your-username\" \\\n  --set target.passwordSecretName=kyomi-connect-db";
        assert_eq!(cmds.kubernetes, expected);
    }

    #[test]
    fn compose_snippet_byte_equivalent_to_js() {
        let cmds = build_deployment_commands("sqlserver", Some("tkn"), None);
        let expected = "# Use \"host.docker.internal\" for DB_HOST if your database is on localhost\n\
                        services:\n  kyomi-connect:\n    image: ghcr.io/kyomi-ai/kyomi-connect:latest\n    restart: always\n    environment:\n      KYOMI_TOKEN: \"tkn\"\n      DB_HOST: \"your-database-host\"\n      DB_PORT: \"1433\"\n      DB_NAME: \"your-database\"\n      DB_USER: \"your-username\"\n      DB_PASSWORD: \"your-password\"";
        assert_eq!(cmds.compose, expected);
    }

    #[test]
    fn explicit_port_overrides_default() {
        let cmds = build_deployment_commands("postgres", Some("tkn"), Some(6543));
        assert!(cmds.docker.contains("-e DB_PORT=\"6543\""));
        assert!(cmds.kubernetes.contains("--set target.port=6543"));
        assert!(cmds.compose.contains("DB_PORT: \"6543\""));
    }

    #[test]
    fn for_tab_dispatches_by_id() {
        let cmds = build_deployment_commands("postgres", Some("tkn"), None);
        assert_eq!(cmds.for_tab("linux"), cmds.linux);
        assert_eq!(cmds.for_tab("docker"), cmds.docker);
        assert_eq!(cmds.for_tab("kubernetes"), cmds.kubernetes);
        assert_eq!(cmds.for_tab("compose"), cmds.compose);
        assert_eq!(cmds.for_tab("bogus"), "");
    }
}
