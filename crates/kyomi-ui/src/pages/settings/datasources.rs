// SPDX-License-Identifier: AGPL-3.0-or-later

//! Data Sources settings page — list, toggle, delete, create, and edit datasources.
//!
//! Replaces `apps/frontend/src/components/settings/DatasourceSettings.jsx` and
//! `apps/frontend/src/components/settings/DatasourceModal.jsx`.

use kyomi_types::Permission;
use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};
use crate::components::{
    Alert, AlertDescription, AlertTitle, AlertVariant, Badge, BadgeVariant, Button, ButtonLink,
    ButtonSize, ButtonVariant, Card, Checkbox, ConfirmDialog, EmptyState,
    Modal, ModalSize, Skeleton, Spinner, Switch, ToggleButton,
};
use crate::components::toast::toast_error;
#[cfg(target_arch = "wasm32")]
use crate::components::toast::{toast_info, toast_success};
use crate::components::Select;
use crate::pages::connect_setup::CONNECT_TYPES;
use crate::pages::settings::connect_deployment::{
    CopyButton, DeploymentCommands, DeploymentTabStrip, build_deployment_commands, default_port,
    supports_ssh_tunnel,
};
use crate::pages::settings::connect_status_panel::ConnectStatusPanel;
use crate::query_cache::{use_query, QueryCache};
use crate::server_fns::connect::{create_connect_datasource, discover_connect_containers};
use crate::server_fns::datasources::*;
use crate::server_fns::sql_editor::refresh_catalog;
#[cfg(target_arch = "wasm32")]
use crate::server_fns::sql_editor::get_catalog_refresh_status;
use crate::server_fns::onboarding::{
    check_sample_datasource_available, create_sample_datasource,
};
use crate::server_fns::datasource_oauth::{
    get_google_oauth_status, get_datasource_oauth_status,
    disconnect_google_oauth, disconnect_datasource_oauth,
    get_google_oauth_projects,
};
use crate::utils::beta_access;
use crate::utils::json::bigquery_include_public;
use crate::utils::oauth_popup::{popup_monitor_outcome_message, PopupMonitorOutcome};
use crate::utils::permissions::{use_analytics_access, use_permissions, AnalyticsAccess};

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Credential status badge text, or None if no badge needed.
fn credential_badge(ds: &DatasourceInfo) -> Option<(&'static str, BadgeVariant)> {
    match ds.credential_status.as_str() {
        "missing" => Some(("Needs Setup", BadgeVariant::Warning)),
        "expired" => Some(("Expired", BadgeVariant::Warning)),
        _ => None,
    }
}

/// Whether a `DatasourceInfo::credential_status` value counts as
/// "connected" for OAuth popup-monitor recovery purposes (KYO-440).
///
/// The list row has no `connected: bool` field like
/// `DatasourceOAuthStatus` (what `ModalOAuthStatusPanel`'s recovery
/// re-checks via `fetch_oauth_status_once`) — its only source of truth is
/// this string. Deliberately the exact inverse of the two states that
/// `DatasourceRow`'s `cred_action` match keys an OAuth Connect/Reconnect
/// button on (`"missing"` and `"expired"`, both `"oauth"`), so a
/// recovered row's status can only ever hide the button it once showed,
/// never disagree with it.
///
/// `cfg`-gated to `wasm32`-or-`test`, mirroring `oauth_popup`'s
/// `popup_poll_should_report_closed` / `translate_google_oauth_error`: its
/// only production caller lives inside the `#[cfg(target_arch = "wasm32")]`
/// popup-monitor recovery closure below, so a native, non-test `--features
/// ssr` build has no reachable caller at all — this keeps the function
/// directly unit-testable on the host target without carrying dead code
/// into that build.
#[cfg(any(target_arch = "wasm32", test))]
fn credential_status_indicates_connected(status: &str) -> bool {
    !matches!(status, "missing" | "expired")
}

/// Generate a slug from a datasource name — matches React `generateSlug`.
fn generate_slug(name: &str) -> String {
    name.to_lowercase()
        .replace(|c: char| c.is_whitespace() || c == '_', "-")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Returns the `connection_config` key that holds the catalog scope list
/// for a given datasource type.
fn catalog_config_key_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        "bigquery" => "catalog_projects",
        "clickhouse" | "mysql" | "snowflake" => "catalog_databases",
        "databricks" => "catalog_catalogs",
        _ => "catalog_schemas",
    }
}

/// Returns the human-readable label for the catalog scope items for a given
/// datasource type.
fn catalog_item_label_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        "bigquery" => "projects",
        "clickhouse" | "mysql" | "snowflake" => "databases",
        "databricks" => "catalogs",
        _ => "schemas",
    }
}

/// Returns the key within the `DiscoverResourcesResult.resources` map that
/// holds the items relevant to catalog scope selection for a given datasource
/// type.  Matches the pairs emitted by `discover_datasource_resources` in
/// `server_fns/datasources.rs`.
///
/// Does NOT cover BigQuery correctly on its own — its `_` fallthrough
/// returns "databases", a key BigQuery never populates. Callers that need
/// a key that's right for every type, BigQuery included, must go through
/// `catalog_denial_key_for_type` below instead of calling this directly
/// (KYO-544).
fn discovery_resource_key_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        // postgres / redshift / sqlserver / synapse / flaredb: catalog scope = schemas
        "postgres" | "redshift" | "sqlserver" | "synapse" | "flaredb" => "schemas",
        // databricks: catalog scope = catalogs
        "databricks" => "catalogs",
        // clickhouse / mysql / snowflake (and bigquery, handled by the
        // caller-facing wrapper below): catalog scope = databases
        _ => "databases",
    }
}

/// The `resource_errors` (and matching `resources`) key that means "this
/// type's catalog scope could not be listed" (KYO-466/KYO-474).
///
/// NOT the same as `discovery_resource_key_for_type` above: BigQuery's
/// catalog scope is `projects`, but that function falls through to its
/// `_ => "databases"` default for `"bigquery"`, and BigQuery never
/// populates a `"databases"` key in either `resources` or
/// `resource_errors` — it only ever emits `"projects"`. Reusing
/// `discovery_resource_key_for_type` directly for BigQuery silently reads
/// a key that can never be present, which is exactly the KYO-544 bug:
/// a real `resourcemanager.projects.list` denial rendered as "0 projects
/// found" instead of the denial copy, because the wrong key was checked.
///
/// `CreateModeCatalogPicker`'s caller and `EditModeCatalogTab`'s
/// `discover_action` Effect both route through this single function —
/// for the same key used to both fetch items (`resources.get(key)`) and
/// detect a denial (`resource_errors.get(key)`) — so the two components
/// can never disagree about which key means what
/// (docs/standards/code-organization/propagate-predicate-changes-to-every-copy.md).
fn catalog_denial_key_for_type(ds_type: &str) -> &'static str {
    match ds_type {
        "bigquery" => "projects",
        other => discovery_resource_key_for_type(other),
    }
}

/// Returns the human-readable provider label for a datasource type.
///
/// Used to build action button labels like "Connect BigQuery" or
/// "Reconnect Snowflake".
fn provider_label(ds_type: &str) -> &'static str {
    match ds_type {
        "bigquery" => "BigQuery",
        "snowflake" => "Snowflake",
        "synapse" => "Azure Synapse",
        "databricks" => "Databricks",
        "flaredb" => "FlareDB",
        _ => "Provider",
    }
}

/// The `auth_mode` BigQuery is treated as when a caller passes `None` (a row
/// or form whose `auth_mode` hasn't been set) — extracted into one constant
/// so [`oauth_url_for_datasource`]'s URL choice and [`list_connect_action`]'s
/// gate resolve a null `auth_mode` to the exact same effective mode. Before
/// this existed, the two lived as separate `unwrap_or("kyomi_oauth")`
/// literals; had `list_connect_action` used a different default, a
/// `auth_mode: None` row would silently bypass the KYO-442 gate while still
/// resolving to the gated Google URL underneath it (see `list_connect_action`
/// for the full failure mode this prevents).
const BIGQUERY_DEFAULT_AUTH_MODE: &str = "kyomi_oauth";

/// Builds the OAuth connect URL for a given datasource type, slug, and auth mode.
///
/// Returns an empty string for types that do not have a server-side OAuth
/// connect endpoint (i.e. non-OAuth datasource types).
///
/// BigQuery has two OAuth flows depending on `auth_mode`:
/// - `"enterprise_oauth"` → bigquery-enterprise endpoint (slug-scoped)
/// - anything else (default: [`BIGQUERY_DEFAULT_AUTH_MODE`]) → shared Google
///   OAuth endpoint
fn oauth_url_for_datasource(ds_type: &str, slug: &str, auth_mode: Option<&str>) -> String {
    match ds_type {
        "bigquery" => match auth_mode.unwrap_or(BIGQUERY_DEFAULT_AUTH_MODE) {
            "enterprise_oauth" => {
                format!(
                    "/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug={slug}"
                )
            }
            _ => "/api/v1/auth/google-oauth/connect".to_string(),
        },
        "snowflake" => {
            format!("/api/v1/auth/oauth/snowflake/connect?datasource_slug={slug}")
        }
        "databricks" => {
            format!("/api/v1/auth/oauth/databricks/connect?datasource_slug={slug}")
        }
        "synapse" => {
            format!(
                "/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug={slug}"
            )
        }
        _ => String::new(),
    }
}

/// The BigQuery kyomi_oauth **Save/Create** gate (KYO-408) — a pure
/// predicate so it's directly unit-testable, unlike the `Signal::derive`
/// closure in `DatasourceModal` that calls it. See `bq_kyomi_oauth_access_ok`'s
/// doc comment at its call site for the full design rationale; in short:
/// this is a UX nudge, not a security control, and is a no-op (always
/// satisfied) for every provider/mode except BigQuery's kyomi_oauth.
///
/// Save/Create **only** — do not reuse this for the Connect/Reconnect
/// button. See [`bq_kyomi_oauth_connect_allowed`] below for that gate and
/// why it is a genuinely different predicate, not a copy of this one
/// (KYO-477).
///
/// Returns `true` (gate satisfied, Save/Create enabled) when any of:
/// - the datasource/mode isn't BigQuery kyomi_oauth at all
/// - `oauth_connected` — a successful OAuth handshake for *this* linked
///   account is itself proof that account was already allowlisted, so
///   there's nothing left to confirm before saving a datasource that
///   already has a working connection
/// - `access_confirmed` — the user ticked the checkbox
fn bq_kyomi_oauth_access_gate_satisfied(
    ds_type: &str,
    bq_auth_mode: &str,
    oauth_connected: bool,
    access_confirmed: bool,
) -> bool {
    !(ds_type == "bigquery" && bq_auth_mode == "kyomi_oauth") || oauth_connected || access_confirmed
}

/// The BigQuery kyomi_oauth **Connect/Reconnect** gate (KYO-477) — a pure
/// predicate so it's directly unit-testable, in the same style and
/// location as [`bq_kyomi_oauth_access_gate_satisfied`] above.
///
/// Deliberately a SEPARATE predicate from `bq_kyomi_oauth_access_gate_satisfied`,
/// not a shared copy — this is not the anti-pattern `docs/CODING_STANDARDS.md`'s
/// "propagate predicate changes to every copy" standard (KYO-423) warns
/// about. That standard forbids letting the *same* predicate drift between
/// call sites; these are two predicates that answer two different
/// questions and were wrongly collapsed into one signal by KYO-427. Do
/// not "fix" that back by reintroducing `oauth_connected` here.
///
/// The reason they must differ: `oauth_connected` for kyomi_oauth is an
/// **account-level** signal (`OAuthStatusSource::GoogleAccount` — one
/// Google link per Kyomi user, shared across every BigQuery kyomi_oauth
/// datasource that user has, or ever will have). Once a user has linked
/// Google to Kyomi even once, `oauth_connected` reads `true` forever,
/// everywhere, regardless of which specific Google account or which
/// specific datasource is in front of them right now. Folding it into
/// the Connect gate (as `bq_kyomi_oauth_access_gate_satisfied` correctly
/// does for Save/Create, where "this account already has a proven
/// connection" is exactly the right question) turns Connect's gate into
/// a permanent no-op for any such user — the checkbox stops meaning
/// anything the moment they've linked Google once. That is the KYO-477
/// defect: reported three times, "fixed" once already (KYO-427, PR #389,
/// shipped in v2.6.5) by a green review and a test that only proved the
/// Connect button *read a signal* — never that the signal computed the
/// right answer. Connect must always require its own explicit,
/// in-the-moment `access_confirmed`, independent of any account-level
/// history.
///
/// Returns `true` (gate satisfied, Connect/Reconnect enabled) when
/// either:
/// - the datasource/mode isn't BigQuery kyomi_oauth at all
/// - `access_confirmed` — the user ticked the checkbox
fn bq_kyomi_oauth_connect_allowed(
    ds_type: &str,
    bq_auth_mode: &str,
    access_confirmed: bool,
) -> bool {
    !(ds_type == "bigquery" && bq_auth_mode == "kyomi_oauth") || access_confirmed
}

/// The outcome of a click on the datasource **list**'s own Connect/Reconnect
/// button (`DatasourceRow::on_oauth_click`) — KYO-442.
///
/// KYO-427/KYO-477 gated BigQuery kyomi_oauth's Connect button behind the
/// KYO-408/KYO-499 beta-access attestation, but only inside
/// `ModalOAuthStatusPanel` (the settings modal's Connection tab). The list's
/// own Connect/Reconnect button calls the exact same [`oauth_url_for_datasource`]
/// and hands the result straight to the OAuth popup with no gate and no
/// notice anywhere on that surface — a user not on Kyomi's Google OAuth
/// test-user allowlist gets a doomed round-trip to Google's `access_denied`
/// with no prior explanation and no "Request beta access" link, because both
/// live only in the modal.
///
/// Rather than duplicating the modal's notice/checkbox UI on the list
/// surface, `OpenModal` routes the user into the settings modal instead —
/// which already defaults its `active_tab` to `"connection"` on open
/// (and resets it there every time it opens), landing the user exactly where
/// the notice, checkbox, and "Request beta access" link live.
///
/// Folds [`bq_kyomi_oauth_connect_allowed`] (the gate) and
/// [`oauth_url_for_datasource`] (the URL choice) into a single function
/// precisely so the two cannot disagree — a click handler that calls them
/// separately can drift if only one side is ever updated. See
/// `bq_kyomi_oauth_connect_allowed`'s own doc comment for the KYO-477
/// precedent of exactly that class of bug (a green review and a test that
/// only proved a signal was *read*, never that it computed the right
/// answer).
#[derive(Debug, PartialEq)]
enum ListConnectAction {
    /// Launch the OAuth popup at this URL — today's behaviour, unchanged.
    LaunchPopup(String),
    /// Open the datasource modal instead of launching a popup, so the user
    /// meets the allowlist notice, the attestation checkbox, and the
    /// "Request beta access" link before any doomed round-trip to Google.
    OpenModal,
    /// This datasource type has no OAuth connect endpoint at all
    /// (`oauth_url_for_datasource` returned an empty string).
    Unsupported,
}

/// Computes [`ListConnectAction`] for a list-row Connect/Reconnect click.
///
/// `auth_mode` is passed through to [`oauth_url_for_datasource`] unchanged
/// (so the URL half is byte-identical to before this function existed); the
/// gate check instead reads `auth_mode.unwrap_or(BIGQUERY_DEFAULT_AUTH_MODE)`
/// — the SAME default `oauth_url_for_datasource` resolves `None` to
/// internally — so a row with `auth_mode: None` cannot resolve to two
/// different effective modes on the two sides of this decision. Evaluating
/// the gate against a different (or missing) default was the exact bypass
/// KYO-442 exists to close: a null `auth_mode` would silently skip the
/// attestation gate while still producing the gated Google URL underneath
/// it.
fn list_connect_action(
    ds_type: &str,
    slug: &str,
    auth_mode: Option<&str>,
    access_confirmed: bool,
) -> ListConnectAction {
    let effective_auth_mode = auth_mode.unwrap_or(BIGQUERY_DEFAULT_AUTH_MODE);
    if !bq_kyomi_oauth_connect_allowed(ds_type, effective_auth_mode, access_confirmed) {
        return ListConnectAction::OpenModal;
    }
    let url = oauth_url_for_datasource(ds_type, slug, auth_mode);
    if url.is_empty() {
        ListConnectAction::Unsupported
    } else {
        ListConnectAction::LaunchPopup(url)
    }
}

/// Whether the create-mode Connection step is satisfied (KYO-404, extended
/// KYO-411, generalized KYO-517) — a pure predicate so it's directly
/// unit-testable, following the same shape as
/// [`bq_kyomi_oauth_access_gate_satisfied`] above. Called from the
/// `connection_step_satisfied` `Signal::derive` in `DatasourceModal`,
/// which is the single source of truth read by the create-mode footer's
/// `can_next` and by all three states of the Catalog tab pill (class,
/// disabled, on:click) — see that call site for why this must stay one
/// signal rather than independent copies of the same check.
///
/// `auth_mode` is the auth mode of whichever provider `ds_type` names —
/// the caller is responsible for picking the right one of
/// `bq_auth_mode` / `sf_auth_mode` / `db_auth_mode` / `synapse_auth_mode`
/// (see the call site). Passing the wrong provider's auth-mode signal
/// either never fires the exception it should, or fires it for the wrong
/// pair — there is nothing in this function's types that catches that,
/// so get it right at the call site.
///
/// Dispatches to [`oauth_source_for_ds_type`] — the same
/// (ds_type, auth_mode) → [`OAuthStatusSource`] mapping every
/// `*AuthModeSection` already wires into `use_oauth_status_refetch` —
/// rather than hand-rolling a second, parallel mapping here that could
/// drift from it. Returns `true` when any of:
/// - the pair resolves to `OAuthStatusSource::Datasource(_)` — its OAuth
///   status (and connect endpoint) is slug-scoped
///   (`get_datasource_oauth_status(key, slug)` / `..._url`) and cannot be
///   reached before the datasource is saved
///   (`oauth_status_source_to_fetch` gates `Datasource(_)` fetches on
///   `!is_create_mode`, KYO-426), so `test_succeeded` can never become
///   true for this pair in create mode (KYO-404, extended KYO-517 from
///   BigQuery enterprise_oauth to Snowflake oauth, Databricks oauth, and
///   Synapse enterprise_oauth — all three deadlocked "Next" identically,
///   for the identical reason).
/// - the pair resolves to `OAuthStatusSource::GoogleAccount` (BigQuery
///   kyomi_oauth, the one account-level source) and `oauth_connected` —
///   a proven Google OAuth connection is equally trustworthy whether it
///   arrived via the popup's `GoogleSuccess` postMessage arm (which
///   itself sets `test_result`) or via `use_oauth_status_refetch`'s
///   account-level status fetch on modal open (KYO-411). Without this
///   arm, a returning, already-linked user's modal renders "Connected"
///   with nothing left to click, `test_result` never gets set, and Next
///   stays permanently disabled — the exact KYO-404 deadlock,
///   reintroduced for exactly the users KYO-411 exists to help.
/// - `test_succeeded` — every other pair (including a `GoogleAccount` or
///   `Datasource(_)` pair with neither an existing connection nor a
///   completed test) requires an actual successful Test & Discover.
fn connection_step_satisfied_from(
    ds_type: &str,
    auth_mode: &str,
    oauth_connected: bool,
    test_succeeded: bool,
) -> bool {
    match oauth_source_for_ds_type(ds_type, auth_mode) {
        Some(OAuthStatusSource::Datasource(_)) => true,
        Some(OAuthStatusSource::GoogleAccount) => oauth_connected || test_succeeded,
        None => test_succeeded,
    }
}

/// Builds `<Select>` options for an Authentication Mode selector from
/// registry-provided auth modes (KYO-274).
///
/// Appends `" (Recommended)"` to the default mode's label. `(Recommended)`
/// is a presentation affordance derived from `is_default` here, in the UI —
/// never baked into `AuthModeOption::display_name` itself, which describes
/// what a mode *is* and is shared with every other registry consumer (see
/// `AuthModeOption`'s doc comment in `server_fns::datasources`).
fn auth_mode_select_options(modes: &[AuthModeOption]) -> Vec<(String, String)> {
    modes
        .iter()
        .map(|m| {
            let label = if m.is_default {
                format!("{} (Recommended)", m.display_name)
            } else {
                m.display_name.clone()
            };
            (m.mode_id.clone(), label)
        })
        .collect()
}

/// Looks up the registry-provided description for the currently selected
/// auth mode (KYO-274). Returns an empty string if `mode_id` isn't found
/// (e.g. the registry data hasn't loaded yet).
fn auth_mode_description(modes: &[AuthModeOption], mode_id: &str) -> String {
    modes
        .iter()
        .find(|m| m.mode_id == mode_id)
        .map(|m| m.description.clone())
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// Main Page
// ─────────────────────────────────────────────────────────────────────────────

/// Coarse-grained branch discriminant for [`DatasourcesPage`]'s view.
///
/// Deliberately collapses `Some(Ok(_))` down to a unit variant rather than
/// carrying the fetched `Vec<DatasourceInfo>` — see the `view_state` comment
/// in `DatasourcesPage` for why (KYO-429).
#[derive(Clone, PartialEq)]
enum DatasourcesViewState {
    Loading,
    Ready,
    Failed(String),
}

/// Data Sources settings page content.
#[component]
pub fn DatasourcesPage() -> impl IntoView {
    // Layout-level QueryCache — cached across navigation and invalidated by
    // `datasource_update` WS events so other tabs (and other workspace
    // members) see create/update/delete mutations without a manual refresh.
    let datasources_signal =
        use_query("datasources", || (), |_: ()| list_datasources());

    // KYO-429: branch on a Memo, not a raw tracked read of `datasources_signal`.
    //
    // `QueryCache::invalidate` re-runs every cached entry's fetcher and, on
    // resolution, writes the result straight into the entry's signal
    // (`build_refetch` in `query_cache/mod.rs`) — including the common case
    // where the new value is `Some(Ok(_))` and the old value was already
    // `Some(Ok(_))`. A plain `{move || match datasources_signal.get() {...}}`
    // closure re-runs its whole body on *every* write regardless of whether
    // the branch actually changed, which re-constructs `DatasourcesContent`
    // (a fresh mount, fresh signals) on every refetch — including ones
    // triggered by a `datasource_update` WS event from another tab or
    // workspace member while this user has the create/edit modal open with
    // unsaved input. `Memo` only notifies when its *output* changes
    // (`PartialEq`), so collapsing the branch through `view_state` first
    // means a `Some(Ok(_))` -> `Some(Ok(_))` refetch produces an unchanged
    // `Ready` and the outer closure below does not re-run at all.
    let view_state = Memo::new(move |_| match datasources_signal.get() {
        None => DatasourcesViewState::Loading,
        Some(Ok(_)) => DatasourcesViewState::Ready,
        Some(Err(e)) => DatasourcesViewState::Failed(e.to_string()),
    });

    view! {
        {move || {
            match view_state.get() {
                DatasourcesViewState::Loading => view! { <DatasourcesLoadingSkeleton/> }.into_any(),
                DatasourcesViewState::Ready => {
                    // Untracked on purpose: this only seeds `DatasourcesContent`'s
                    // local `datasources` signal (see its own `initial_datasources`
                    // doc comment) — `DatasourcesContent` re-syncs itself from
                    // `datasources_signal` via its own Effect on every later
                    // refetch. A tracked `.get()` here would resubscribe this
                    // closure to `datasources_signal` directly and defeat the
                    // whole point of routing through `view_state`.
                    //
                    // The `Some(Ok(_))` case cannot fail to match: `view_state`
                    // just evaluated to `Ready` above, which the Memo only
                    // produces when its tracked read of `datasources_signal` is
                    // `Some(Ok(_))`; Leptos's reactive graph is single-threaded
                    // and this untracked read happens synchronously, in the same
                    // call stack, with no `.await` or re-entrant write between
                    // the two — so the value cannot have changed underneath it.
                    let datasources = match datasources_signal.get_untracked() {
                        Some(Ok(datasources)) => datasources,
                        other => unreachable!(
                            "view_state == Ready implies datasources_signal is \
                             Some(Ok(_)); got {other:?}"
                        ),
                    };
                    view! {
                        <DatasourcesContent
                            initial_datasources=datasources
                            datasources_signal=datasources_signal
                        />
                    }.into_any()
                }
                DatasourcesViewState::Failed(e) => view! {
                    <div class="p-4 sm:p-6 space-y-6">
                        <div>
                            <h2 class="text-xl font-display text-foreground">"Datasources"</h2>
                            <p class="text-sm text-muted-foreground">
                                "Manage database connections"
                            </p>
                        </div>
                        <Card>
                            <div class="p-6">
                                <p class="text-error-foreground">
                                    {format!("Failed to load datasources: {e}")}
                                </p>
                            </div>
                        </Card>
                    </div>
                }.into_any(),
            }
        }}
    }
}

/// Whether `row_id`'s in-progress delete UI should render.
///
/// `Action::pending()` is action-wide — `delete_ds_action` is shared by
/// every row in the list, so reading `pending()` alone would make *every*
/// row spin during any single delete (KYO-467). This pure, testable
/// extraction is the actual comparison Signal::derive wraps at the two
/// call sites (the row's dimmed/spinner state and its click guards) —
/// mirrors the same "extract the boolean, test it directly" shape as
/// `connection_auth_modes_unavailable_from` elsewhere in this file.
fn row_is_deleting(datasource_to_delete_id: Option<&str>, row_id: &str, delete_pending: bool) -> bool {
    delete_pending && datasource_to_delete_id == Some(row_id)
}

/// Loading skeleton shown while data is being fetched.
#[component]
fn DatasourcesLoadingSkeleton() -> impl IntoView {
    view! {
        <div class="p-6 space-y-4" style:display="block">
            <Skeleton class="h-8 w-64"/>
            <Skeleton class="h-24 w-full"/>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Content (with data loaded)
// ─────────────────────────────────────────────────────────────────────────────

/// Main content rendered after data is loaded.
#[component]
fn DatasourcesContent(
    initial_datasources: Vec<DatasourceInfo>,
    datasources_signal: Signal<Option<Result<Vec<DatasourceInfo>, ServerFnError>>>,
) -> impl IntoView {
    let (datasources, set_datasources) = signal(initial_datasources);
    let query_cache = expect_context::<QueryCache>();

    // ── Permission gating (KYO-184, KYO-189 P2) ──────────────────────────
    // Create/edit/delete require ManageDatasources server-side
    // (`ac.require(Permission::ManageDatasources, ...)` in `server_fns/datasources.rs`). Per-user
    // credential entry, the OAuth connect buttons, and the per-user enable
    // toggle stay ungated — those are intentionally available to every
    // member (see `docs/DATASOURCE_ARCHITECTURE.md` §5.2).
    let perms = use_permissions();
    let is_admin = Signal::derive(move || perms.can(Permission::ManageDatasources));

    // ── Analytics access gating (KYO-260) ────────────────────────────────
    // Whether the caller may reach `/settings/analytics` — same
    // `analytics_access` predicate the Settings tab bar and the analytics
    // page's own guard consume. Computed once here (reactive, not
    // per-row) via the shared hook, and threaded down to `DatasourceRow`
    // exactly like `is_admin` above — the hook must not be invoked again
    // inside the row/list loop.
    let analytics_access = use_analytics_access();

    // ── Modal state ──────────────────────────────────────────────────────
    // None = closed, Some(None) = create mode, Some(Some(id)) = edit mode
    let (modal_datasource_id, set_modal_datasource_id) =
        signal::<Option<Option<String>>>(None);

    let modal_open = Signal::derive(move || modal_datasource_id.get().is_some());

    let on_modal_close = Callback::new(move |()| {
        set_modal_datasource_id.set(None);
    });

    let on_datasource_saved = Callback::new(move |_result: DatasourceResult| {
        set_modal_datasource_id.set(None);
        // Refresh the datasource list — the cached signal will refetch in
        // the background and reseed `datasources` via the Effect below.
        query_cache.invalidate("datasources");
    });

    // When the shared cache refreshes (WS invalidation or explicit
    // invalidate), sync the local optimistic list.
    Effect::new(move |_| {
        if let Some(Ok(list)) = datasources_signal.get() {
            set_datasources.set(list);
        }
    });

    // ── Delete state ────────────────────────────────────────────────────
    let (delete_dialog_open, set_delete_dialog_open) = signal(false);
    let (datasource_to_delete, set_datasource_to_delete) =
        signal::<Option<DatasourceInfo>>(None);

    let delete_ds_action = Action::new(|ds_id: &String| {
        let ds_id = ds_id.clone();
        async move { delete_datasource(ds_id).await }
    });

    // Action-wide pending flag. Two distinct uses, deliberately kept
    // separate: (1) `DatasourceRow` combines this with `datasource_to_delete`
    // via `row_is_deleting` to gate the *per-row* dimmed/spinner state — using
    // this flag alone there would spin every row during any delete (KYO-467);
    // (2) `on_delete_click` below gates on this flag *alone* (action-wide) as
    // the double-dispatch guard, since `delete_ds_action` and
    // `datasource_to_delete` are both singular and shared across every row —
    // starting a second delete while one is in flight would overwrite
    // `datasource_to_delete` out from under the first delete's own Effect.
    let delete_pending = Signal::derive(move || delete_ds_action.pending().get());

    Effect::new(move |_| {
        if let Some(result) = delete_ds_action.value().get() {
            match result {
                Ok(()) => {
                    if let Some(ds) = datasource_to_delete.get_untracked() {
                        set_datasources.update(|list| {
                            list.retain(|d| d.id != ds.id);
                        });
                        #[cfg(target_arch = "wasm32")]
                        toast_success(format!("\"{}\" deleted", ds.name));
                    }
                    set_datasource_to_delete.set(None);
                }
                Err(e) => {
                    leptos::logging::error!("Failed to delete datasource: {e}");
                    // KYO-467 — previously console-only, so a failed delete was
                    // indistinguishable from a successful one: the confirm
                    // dialog had already closed, the row never moved, and
                    // there was no other signal anything went wrong. `e` is
                    // already sanitized server-side (`into_sfn` runs every
                    // datasource_service error through `kyomi_core::sanitize_error`
                    // before it crosses the wire — see `delete_datasource` in
                    // server_fns/datasources.rs), so it's safe to surface as-is;
                    // `kyomi_core` itself is an ssr-only dependency and can't be
                    // called again from this client-side Effect.
                    toast_error(format!("Failed to delete datasource: {e}"));
                    set_datasource_to_delete.set(None);
                }
            }
        }
    });

    let on_delete_confirm = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        if let Some(ds) = datasource_to_delete.get_untracked() {
            delete_ds_action.dispatch(ds.id.clone());
        }
    });

    let on_delete_cancel = Callback::new(move |()| {
        set_delete_dialog_open.set(false);
        set_datasource_to_delete.set(None);
    });

    let delete_title = "Delete Datasource?".to_string();
    let delete_message = move || {
        datasource_to_delete
            .get()
            .map(|ds| {
                format!(
                    "Are you sure you want to delete \"{}\"? This cannot be undone.",
                    ds.name
                )
            })
            .unwrap_or_default()
    };

    // ── OAuth connecting state ───────────────────────────────────────────
    // Tracks which datasource ID is currently awaiting OAuth completion.
    // None = no OAuth in progress. Some(id) = popup open for that datasource.
    let (oauth_connecting, set_oauth_connecting) = signal::<Option<String>>(None);

    // ── OAuth postMessage listener ───────────────────────────────────────
    // Installed once at the list level so all rows share a single listener.
    // The JS Closure inside install_oauth_listener is !Send, so we use
    // SendWrapper to satisfy on_cleanup's Send+Sync bound. The wrapper holds
    // a box that stores the cleanup FnOnce so drop() can call it.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::utils::oauth_popup::{
            install_oauth_listener, translate_google_oauth_error, OAuthMessage,
        };
        let query_cache_for_oauth = query_cache;
        let cleanup = install_oauth_listener(move |msg| {
            match msg {
                OAuthMessage::GoogleSuccess { .. }
                | OAuthMessage::SnowflakeSuccess { .. }
                | OAuthMessage::DatabricksSuccess { .. }
                | OAuthMessage::MicrosoftSuccess { .. }
                | OAuthMessage::MicrosoftEnterpriseSuccess { .. }
                | OAuthMessage::BigqueryEnterpriseSuccess { .. } => {
                    set_oauth_connecting.try_set(None);
                    toast_success("Datasource connected successfully");
                    query_cache_for_oauth.invalidate("datasources");
                }
                OAuthMessage::GoogleError { error } => {
                    set_oauth_connecting.try_set(None);
                    leptos::logging::warn!("OAuth error: {error}");
                    toast_error(translate_google_oauth_error(error));
                }
                OAuthMessage::SnowflakeError { error }
                | OAuthMessage::DatabricksError { error }
                | OAuthMessage::MicrosoftError { error }
                | OAuthMessage::MicrosoftEnterpriseError { error }
                | OAuthMessage::BigqueryEnterpriseError { error } => {
                    set_oauth_connecting.try_set(None);
                    leptos::logging::warn!("OAuth error: {error}");
                    toast_error(error);
                }
            }
        });
        // Box<dyn FnOnce()> is used so the inner cleanup can be called through
        // Drop without requiring Send. SendWrapper makes the box Send+Sync for
        // on_cleanup's bound while guaranteeing single-threaded access on WASM.
        let cleanup_cell = std::cell::Cell::new(
            Some(Box::new(cleanup) as Box<dyn FnOnce()>),
        );
        let cleanup_wrapper = send_wrapper::SendWrapper::new(cleanup_cell);
        on_cleanup(move || {
            if let Some(f) = cleanup_wrapper.take().take() {
                f();
            }
        });
    }

    view! {
        <div class="p-4 sm:p-6 space-y-6">
            // Header
            <div class="flex items-center justify-between">
                <div>
                    <h2 class="text-xl font-display text-foreground">"Datasources"</h2>
                    <p class="text-sm text-muted-foreground">
                        "Manage database connections"
                    </p>
                </div>
                // Header CTA — only shown when at least one datasource exists
                // AND the caller is a workspace admin (create is admin-only —
                // `create_datasource_modal` → `Permission::ManageDatasources`).
                // Empty state renders its own prominent CTA below (see `EmptyState`),
                // so double-showing the button creates a duplicate "Add Datasource" CTA.
                <Show when=move || !datasources.get().is_empty() && is_admin.get()>
                    <Button
                        on:click=move |_| set_modal_datasource_id.set(Some(None))
                    >
                        <span class="h-4 w-4 inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::PLUS/>
                        </span>
                        "Add Datasource"
                    </Button>
                </Show>
            </div>

            // Datasources List
            <Card>
                <div class="p-0">
                    <Show
                        when=move || !datasources.get().is_empty()
                        fallback=move || view! {
                            // Empty-state CTA is admin-only for the same reason as the
                            // header CTA above. Nested <Show> (not a `.get()` read inside
                            // this fallback closure) per CODING_STANDARDS.md — reading
                            // `is_admin` directly here would subscribe the outer Show's
                            // wrapping closure to it, causing the whole list branch to
                            // remount whenever admin status resolves.
                            <Show
                                when=move || is_admin.get()
                                fallback=move || view! {
                                    <EmptyState
                                        icon=std::sync::Arc::new(|| view! {
                                            <Icon icon=phosphor_leptos::DATABASE weight=IconWeight::Duotone size="64px"/>
                                        }.into_any())
                                        title="No datasources configured"
                                        description="Ask a workspace admin to connect a data source"
                                    />
                                }
                            >
                                <EmptyState
                                    icon=std::sync::Arc::new(|| view! {
                                        <Icon icon=phosphor_leptos::DATABASE weight=IconWeight::Duotone size="64px"/>
                                    }.into_any())
                                    title="No datasources configured"
                                    description="Connect a data source to start querying your data"
                                    action=std::sync::Arc::new(move || view! {
                                        <Button on:click=move |_| set_modal_datasource_id.set(Some(None))>
                                            <span class="h-4 w-4 inline-flex items-center justify-center">
                                                <Icon icon=phosphor_leptos::PLUS/>
                                            </span>
                                            "Add Datasource"
                                        </Button>
                                    }.into_any())
                                />
                            </Show>
                        }
                    >
                        <div class="divide-y divide-border">
                            <For
                                each=move || datasources.get()
                                key=|ds| ds.id.clone()
                                let:ds
                            >
                                <DatasourceRow
                                    ds=ds
                                    set_datasources=set_datasources
                                    set_delete_dialog_open=set_delete_dialog_open
                                    set_datasource_to_delete=set_datasource_to_delete
                                    set_modal_datasource_id=set_modal_datasource_id
                                    oauth_connecting=oauth_connecting
                                    set_oauth_connecting=set_oauth_connecting
                                    is_admin=is_admin
                                    analytics_access=analytics_access
                                    datasource_to_delete=datasource_to_delete
                                    delete_pending=delete_pending
                                />
                            </For>
                        </div>
                    </Show>
                </div>
            </Card>
        </div>

        // Datasource Modal (create and edit)
        <DatasourceModal
            open=modal_open
            datasource_id=Signal::derive(move || {
                modal_datasource_id.get().flatten()
            })
            on_close=on_modal_close
            on_saved=on_datasource_saved
        />

        // Delete Confirmation Dialog
        {move || view! {
            <ConfirmDialog
                open=Signal::from(delete_dialog_open)
                title=delete_title.clone()
                message=delete_message()
                confirm_text="Delete"
                on_confirm=on_delete_confirm
                on_cancel=on_delete_cancel
            />
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Datasource Row
// ─────────────────────────────────────────────────────────────────────────────

/// A single datasource row in the list.
#[component]
fn DatasourceRow(
    ds: DatasourceInfo,
    set_datasources: WriteSignal<Vec<DatasourceInfo>>,
    set_delete_dialog_open: WriteSignal<bool>,
    set_datasource_to_delete: WriteSignal<Option<DatasourceInfo>>,
    set_modal_datasource_id: WriteSignal<Option<Option<String>>>,
    /// Which datasource ID is currently awaiting OAuth completion (if any).
    oauth_connecting: ReadSignal<Option<String>>,
    /// Setter for the OAuth connecting state — passed to the popup monitor.
    set_oauth_connecting: WriteSignal<Option<String>>,
    /// Whether the caller is a workspace admin — gates the delete button
    /// (`delete_datasource` → `Permission::ManageDatasources`). Passed as a
    /// `Signal` (not snapshotted) per CODING_STANDARDS.md.
    is_admin: Signal<bool>,
    /// Whether the caller may reach `/settings/analytics` — gates the
    /// "Analytics Settings" link on analytics datasource rows (KYO-260).
    /// Computed once at the list level by the shared analytics-access hook
    /// and passed as a `Signal`, mirroring `is_admin` above.
    analytics_access: Signal<AnalyticsAccess>,
    /// Which datasource the delete confirmation currently targets, if any —
    /// the same signal `DatasourcesContent`'s delete Effect reads. Combined
    /// with `delete_pending` via `row_is_deleting` (KYO-467) so only the
    /// targeted row renders in-progress state, not every row in the list.
    datasource_to_delete: ReadSignal<Option<DatasourceInfo>>,
    /// Whether `delete_ds_action` (defined once, shared by every row, in
    /// `DatasourcesContent`) is currently in flight.
    delete_pending: Signal<bool>,
) -> impl IntoView {
    // ── Delete-in-progress state (KYO-467) ─────────────────────────────
    // Gated on this row's id matching the delete target AND the action
    // being pending — see `row_is_deleting`. Mirrors the shape of
    // `is_connecting` below (compare this row's id against a shared
    // "which id is active" signal), so two concurrent deletes can never
    // cross-contaminate: `delete_ds_action` only ever has one target at a
    // time (`on_delete_click` refuses to start a second delete while one
    // is pending — see below), and this comparison only lights up for the
    // row whose id currently matches that single target.
    let ds_id_for_delete_state = ds.id.clone();
    let is_deleting = Signal::derive(move || {
        row_is_deleting(
            datasource_to_delete.get().as_ref().map(|d| d.id.as_str()),
            &ds_id_for_delete_state,
            delete_pending.get(),
        )
    });

    // ── Toggle state ────────────────────────────────────────────────────
    let ds_for_toggle = ds.clone();
    let (local_enabled, set_local_enabled) = signal(ds.user_enabled);

    let can_enable = ds.can_enable;
    let ds_credential_status = ds.credential_status.clone();

    let toggle_action = Action::new(|(ds_id, new_val): &(String, bool)| {
        let ds_id = ds_id.clone();
        let new_val = *new_val;
        async move { toggle_datasource(ds_id, new_val).await.map(|()| new_val) }
    });

    let ds_id_for_effect = ds_for_toggle.id.clone();
    Effect::new(move |_| {
        if let Some(result) = toggle_action.value().get() {
            match result {
                Ok(new_val) => {
                    let id = ds_id_for_effect.clone();
                    set_datasources.update(|list| {
                        if let Some(d) = list.iter_mut().find(|d| d.id == id) {
                            d.user_enabled = new_val;
                        }
                    });
                }
                Err(e) => {
                    set_local_enabled.update(|v| *v = !*v);
                    leptos::logging::error!("Failed to toggle datasource: {e}");
                }
            }
        }
    });

    let switch_disabled = !can_enable;

    let on_toggle = Callback::new(move |new_val: bool| {
        if toggle_action.pending().get_untracked() {
            return;
        }
        // KYO-467 — this row is being deleted; toggling it mid-delete makes
        // no sense (and the row is visually disabled/dimmed for exactly
        // this reason). Switch's `disabled` prop is a plain bool, not
        // reactive (see its definition in switch.rs, out of this ticket's
        // scope), so this guard is the functional backstop that keeps a
        // stray keyboard toggle from doing anything during the delete.
        if is_deleting.get_untracked() {
            return;
        }
        // Gate: cannot enable a datasource with missing credentials.
        // The switch is already visually disabled (switch_disabled), but we
        // add a toast so the user understands why clicking elsewhere doesn't work.
        if new_val && ds_credential_status == "missing" {
            toast_error("Connect your credentials first to enable this datasource");
            return;
        }
        let ds_id = ds_for_toggle.id.clone();
        set_local_enabled.set(new_val);
        toggle_action.dispatch((ds_id, new_val));
    });

    // ── Delete handler ──────────────────────────────────────────────────
    let ds_for_delete = ds.clone();
    let on_delete_click = move |_: leptos::ev::MouseEvent| {
        // Action-wide guard (not per-row `is_deleting`): `delete_ds_action`
        // and `datasource_to_delete` are both singular, shared by every row.
        // Starting a second delete while one is in flight would overwrite
        // `datasource_to_delete` before the first delete's Effect reads it
        // back — corrupting which row that Effect removes/reports on.
        if delete_pending.get_untracked() {
            return;
        }
        set_datasource_to_delete.set(Some(ds_for_delete.clone()));
        set_delete_dialog_open.set(true);
    };

    // ── Settings handler ─────────────────────────────────────────────────
    let ds_id_for_settings = ds.id.clone();
    let on_settings_click = move |_: leptos::ev::MouseEvent| {
        if is_deleting.get_untracked() {
            return;
        }
        set_modal_datasource_id.set(Some(Some(ds_id_for_settings.clone())));
    };

    // ── Credential badge ────────────────────────────────────────────────
    let cred_badge = credential_badge(&ds);

    // ── Catalog attention ───────────────────────────────────────────────
    let show_catalog_warning = ds.can_enable && ds.needs_catalog_attention;

    // ── Toggle label ────────────────────────────────────────────────────
    let toggle_label = move || {
        if local_enabled.get() {
            "Enabled"
        } else {
            "Disabled"
        }
    };

    // ── OAuth recovery state (KYO-440) ───────────────────────────────────
    // `query_cache` lets the popup-monitor recovery path below adopt a
    // recovered connection the same way the list-level postMessage success
    // handler does (`query_cache.invalidate("datasources")` in
    // `DatasourcesPage`, above) — keeping the cached list (and any later
    // remount of this row) in sync with the real credential_status.
    let query_cache = expect_context::<QueryCache>();

    // `cred_action`/`cred_action_view` below are derived once from
    // `ds.credential_status` at row-mount time, not from a reactive
    // signal. `<For>`'s keyed diff (`tachys::view::keyed`) never
    // re-invokes a retained key's view function — only genuinely new keys
    // get rebuilt — so a `query_cache.invalidate("datasources")` refetch
    // alone cannot make *this already-mounted* row's Connect/Reconnect
    // button disappear once popup-monitor recovery finds the connection
    // actually succeeded. `oauth_recovered` is this row's own optimistic
    // local override, mirroring `local_enabled` above, except
    // one-directional: a row can only become recovered, never
    // un-recovered, within its own mounted lifetime. It gates the
    // `class:hidden` on the button wrapper below.
    let (oauth_recovered, set_oauth_recovered) = signal(false);

    // On native (non-WASM) targets, `query_cache` and `set_oauth_recovered`
    // are only referenced inside the `#[cfg(target_arch = "wasm32")]`
    // popup-monitor recovery closure below — same reasoning as
    // `ModalOAuthStatusPanel`'s `connect_url`/`on_recover` suppression.
    // `oauth_recovered` (the read half) stays live on every target: the
    // `class:hidden` toggle on the button wrapper reads it unconditionally.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = query_cache;
        let _ = set_oauth_recovered;
    }

    // `connect_attempt_live` is this row's own PRIVATE record of "my
    // current connect attempt is still wanted" (KYO-440 cycle 4, Option
    // A). `monitor_oauth_popup`'s `still_connecting` closure (below,
    // inside `on_oauth_click`) reads this instead of comparing the
    // list-shared `oauth_connecting: Option<String>` against this row's
    // id.
    //
    // Cycle 3 tried the opposite: keep comparing the shared signal by id,
    // and close every gap by widening the click guard to refuse a second
    // connect while ANY row was in flight. The review that opened this
    // cycle traced every writer the signal's *type* actually has, not
    // just the ones the diff touched
    // (`docs/standards/code-organization/enumerate-consumers-from-the-type-not-from-the-diff.md`),
    // and found a third: the settings modal's own `start_connect`
    // (`ModalOAuthStatusPanel`, below) guards on its own private
    // `modal_oauth_connecting` bool and never reads or writes this row's
    // shared signal at all — while the list-level `install_oauth_listener`
    // (above, in `DatasourcesPage`) clears the shared signal on ANY
    // recognised OAuth `postMessage`, because `OAuthMessage` itself
    // carries no datasource id to check. So a user connecting one row,
    // then opening Settings and completing an unrelated connect there,
    // silently clears this row's `still_connecting` out from under its
    // still-open popup — cycle 3's exact bug, still open, just with a new
    // trigger. Extending the click guard again to also cover the modal
    // would mean locking the user out of Settings for up to
    // `POPUP_CONNECT_TIMEOUT_MS` (2.5 minutes) after any abandoned popup —
    // a worse product than the bug.
    //
    // A private token sidesteps the whole class of writer: nothing outside
    // this row's own click handler, its own drain-before-arm (below), and
    // its own `on_cleanup` can ever change it, so no external writer —
    // present or future — can desync it from what this row's own popup is
    // actually doing. `oauth_connecting` keeps its original, sole
    // remaining role: driving which row's button shows
    // "Connecting..."/"Reconnecting..." and the disabled state (see
    // `is_connecting` and the button's `disabled` prop below) — display
    // only; nothing correctness-sensitive reads it anymore.
    //
    // The trade this makes: `OAuthMessage` carries no datasource id, so
    // this token cannot observe a genuine success or error `postMessage`
    // either — only `install_oauth_listener`'s shared listener can. See
    // the recovery closure below (inside `on_oauth_click`) for what that
    // costs, traced at the point it happens.
    #[cfg(target_arch = "wasm32")]
    let (connect_attempt_live, set_connect_attempt_live) = signal(false);

    // Holds the cleanup returned by `monitor_oauth_popup` for whichever
    // connect attempt this row currently has in flight (KYO-440), so this
    // row's teardown can stop the popup-closed poll and connect timeout
    // immediately rather than waiting for the next tick — mirrors the
    // `ModalOAuthStatusPanel` `popup_monitor` pattern this parallels
    // (`PopupMonitorCleanupSlot`, above). A row unmounting mid-connect (the
    // list refetches and re-renders constantly) must stop the timers right
    // away.
    //
    // Drain-before-arm (in `on_oauth_click`, below) is load-bearing here,
    // not merely defensive: `on_oauth_click`'s own click guard compares
    // only *this row's* id against the shared `oauth_connecting` signal
    // (reverted to its pre-cycle-3 shape — see `connect_attempt_live`
    // above for why the shared signal is no longer trusted for anything
    // correctness-sensitive). Because a DIFFERENT row's click can
    // overwrite that shared signal with its own id at any time — nothing
    // stops it now, concurrent per-row connects are allowed again — this
    // row's own guard can read "not connecting" and let a re-click
    // through even while this row's own earlier popup and monitor are
    // still live. Draining — invoking, not merely dropping, whatever
    // cleanup is already stashed here — before installing the new one is
    // what actually stops the superseded attempt's `Interval`/`Timeout` in
    // that case.
    #[cfg(target_arch = "wasm32")]
    let popup_monitor: PopupMonitorCleanupSlot = StoredValue::new(None);
    #[cfg(target_arch = "wasm32")]
    on_cleanup(move || {
        set_connect_attempt_live.try_set(false);
        popup_monitor.update_value(|slot| {
            if let Some(cleanup) = slot.take() {
                cleanup.take()();
            }
        });
    });

    // ── Credential action button ─────────────────────────────────────────
    // Determine whether to show a connect/reconnect button based on the
    // datasource's credential_status and auth_method.
    //
    // "missing" + "oauth"     → "Connect {Provider}"
    // "expired"  + "oauth"    → "Reconnect {Provider}"
    // "missing"  + "password" → "Enter Credentials" (opens the settings modal)
    // anything else           → no button (credentials valid or shared)
    let cred_action: Option<(&'static str, bool)> =
        match (ds.credential_status.as_str(), ds.auth_method.as_str()) {
            ("missing", "oauth") => Some(("Connect", false)),
            ("expired", "oauth") => Some(("Reconnect", true)),
            ("missing", "password") => Some(("enter_credentials", false)),
            _ => None,
        };

    let ds_id_for_cred = ds.id.clone();
    let ds_id_for_connecting = ds.id.clone();
    let ds_type_for_cred = ds.datasource_type.clone();
    let ds_slug_for_cred = ds.slug.clone();
    let ds_auth_mode_for_cred = ds.auth_mode.clone();
    let ds_id_for_modal = ds.id.clone();

    let cred_action_view = cred_action.map(|(action_key, is_warning)| {
        let ds_id = ds_id_for_cred.clone();
        let ds_type = ds_type_for_cred.clone();
        let ds_slug = ds_slug_for_cred.clone();
        let ds_auth_mode = ds_auth_mode_for_cred.clone();

        let button = if action_key == "enter_credentials" {
            // Password datasource: open the settings modal
            let ds_id_modal = ds_id_for_modal.clone();
            view! {
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    disabled=is_deleting
                    on:click=move |_| {
                        if is_deleting.get_untracked() {
                            return;
                        }
                        set_modal_datasource_id
                            .set(Some(Some(ds_id_modal.clone())));
                    }
                >
                    <span class="h-4 w-4 sm:mr-1 inline-flex items-center justify-center">
                        <Icon icon=phosphor_leptos::KEY/>
                    </span>
                    <span class="hidden sm:inline">"Enter Credentials"</span>
                </Button>
            }
            .into_any()
        } else {
            // OAuth datasource: open an OAuth popup window.
            let provider = provider_label(&ds_type);
            let button_label = format!("{action_key} {provider}");
            let connect_label = button_label.clone();
            let ds_id_clone = ds_id.clone();
            let ds_type_clone = ds_type.clone();
            let ds_slug_clone = ds_slug.clone();
            let ds_auth_mode_clone = ds_auth_mode.clone();

            // Per-row: only true while *this* row's own id is the one
            // recorded in the shared `oauth_connecting` signal — drives the
            // spinner/"Connecting..." label so only the row that's actually
            // connecting shows it, not every row (KYO-440).
            let is_connecting = {
                let ds_id_check = ds_id_for_connecting.clone();
                Signal::derive(move || {
                    oauth_connecting.get().as_deref() == Some(ds_id_check.as_str())
                })
            };

            let connecting_label = if is_warning {
                "Reconnecting..."
            } else {
                "Connecting..."
            };

            // Use an Action so any async browser popup work runs in a managed context.
            // The actual popup open is WASM-only; Action::new body is Send so we
            // do the WASM call outside the Action using a synchronous helper.
            let on_oauth_click = move |_: leptos::ev::MouseEvent| {
                // Guards on THIS row's own state again (KYO-440 cycle 4
                // reverts cycle 3's "any row connecting" widening — see
                // `connect_attempt_live`'s doc comment above for why that
                // guard's whole justification collapsed: the settings
                // modal writes its own private `modal_oauth_connecting`
                // bool and never touches this shared signal at all, so
                // widening this guard to cover "any row" never closed the
                // gap it was meant to close). This is now a plain UX
                // throttle against a double-click on THIS button, not a
                // correctness mechanism — the popup monitor's correctness
                // comes from `connect_attempt_live` (private to this row)
                // and the drain-before-arm below, neither of which this
                // guard participates in.
                if is_connecting.get_untracked() || is_deleting.get_untracked() {
                    return;
                }
                // KYO-442: the whole gate-vs-URL decision is one pure call
                // into `list_connect_action` (rather than an `if` sprinkled
                // here and the predicate tested separately) so the gate and
                // the URL it launches cannot disagree — see that function's
                // doc comment. Read at click time, not captured earlier:
                // `beta_access::read_beta_access()` reflects whatever the
                // user last ticked in the modal's checkbox, which may have
                // changed since this row mounted.
                let access_confirmed = beta_access::read_beta_access();
                let action = list_connect_action(
                    &ds_type_clone,
                    &ds_slug_clone,
                    ds_auth_mode_clone.as_deref(),
                    access_confirmed,
                );
                let url = match action {
                    ListConnectAction::LaunchPopup(url) => url,
                    ListConnectAction::OpenModal => {
                        // Send the user into the settings modal instead of
                        // launching a popup — no spinner, no popup, no
                        // `oauth_connecting` write. The modal opens on its
                        // Connection tab, where the allowlist notice, the
                        // attestation checkbox, and the "Request beta
                        // access" link live.
                        set_modal_datasource_id.set(Some(Some(ds_id_clone.clone())));
                        return;
                    }
                    ListConnectAction::Unsupported => {
                        toast_error("OAuth is not supported for this datasource type".to_string());
                        return;
                    }
                };
                set_oauth_connecting.set(Some(ds_id_clone.clone()));

                #[cfg(target_arch = "wasm32")]
                {
                    use crate::utils::oauth_popup::{
                        monitor_oauth_popup, open_oauth_popup as open_popup,
                    };
                    match open_popup(&url, &ds_id_clone) {
                        Some(popup) => {
                            // `still_connecting` reads this row's own
                            // private `connect_attempt_live` token, never
                            // the list-shared `oauth_connecting` signal
                            // (KYO-440 cycle 4 — see that token's doc
                            // comment above this function's OAuth-recovery
                            // section for the full reasoning: comparing
                            // the shared signal by id, even scoped to this
                            // row's own id, is still reachable by writers
                            // outside this row, namely the settings
                            // modal's independent listener). `try_get_untracked`
                            // because this runs inside a deferred
                            // `gloo_timers` callback, not a reactive scope
                            // — the token may already be disposed if this
                            // row unmounted (see the disposal-safety
                            // standard).
                            let ds_id_for_recover = ds_id_clone.clone();
                            let cleanup = monitor_oauth_popup(
                                popup,
                                move || connect_attempt_live.try_get_untracked().unwrap_or(false),
                                move |outcome| {
                                    // KYO-524: did an OAuth `postMessage`
                                    // already resolve this connect attempt
                                    // before this monitor's own outcome
                                    // fired?
                                    //
                                    // `oauth_connecting` is the list-shared
                                    // signal that the list-level
                                    // `install_oauth_listener` (above, in
                                    // `DatasourcesPage`) clears to `None`
                                    // the instant ANY recognized OAuth
                                    // `postMessage` arrives — success or
                                    // error, for any row — because
                                    // `OAuthMessage` carries no datasource
                                    // id for that listener to filter on.
                                    // `still_connecting` above
                                    // (`connect_attempt_live`) never
                                    // observes that message — by design,
                                    // it is this row's own private
                                    // "attempt still wanted" token, touched
                                    // by nothing outside this row's own
                                    // click handler, drain, and cleanup —
                                    // so `on_outcome` fires here
                                    // regardless of whether a message
                                    // already resolved this attempt. This
                                    // check is the missing piece.
                                    //
                                    // Consulted ONLY to decide whether to
                                    // speak, never to decide whether to
                                    // keep monitoring: `connect_attempt_live`
                                    // stays the sole input to
                                    // `still_connecting`, completely
                                    // unchanged by this check, so the
                                    // monitor still always runs to
                                    // completion rather than silently
                                    // self-stopping the moment an external
                                    // writer touches the shared signal —
                                    // the property KYO-440 cycle 3
                                    // required and this must not regress.
                                    // A future reader tempted to
                                    // "simplify" `still_connecting` to
                                    // read this same signal would
                                    // reintroduce that exact regression.
                                    //
                                    // Read with `try_get_untracked()` at
                                    // the moment `on_outcome` fires, for
                                    // the same disposal-safety reason as
                                    // `connect_attempt_live` above — this
                                    // still runs inside a deferred
                                    // `gloo_timers` callback, not a
                                    // reactive tracking scope.
                                    //
                                    // `None` here means a message already
                                    // arrived and the list-level listener
                                    // already gave the user an accurate,
                                    // complete response for SOME attempt
                                    // (this row's own, most of the time) —
                                    // a real `toast_success` plus cache
                                    // invalidation, or the real
                                    // `toast_error`. Recovering on top of
                                    // that would either re-run the recovery
                                    // fetch redundantly after a real
                                    // success, or worse, show a false
                                    // "connection cancelled." over a real
                                    // error the user already saw described
                                    // accurately — this was KYO-524. Say
                                    // nothing: skip the fetch and the
                                    // toast entirely, and leave this row's
                                    // local state (`oauth_connecting`) to
                                    // the guarded clear below on the next
                                    // path that reaches it — it is already
                                    // `None`, so there is nothing to
                                    // clear.
                                    //
                                    // `Some(_)` — whether it names this
                                    // row or a different one — means no
                                    // message has resolved this attempt
                                    // yet, so the recovery fetch below is
                                    // the only source of truth available,
                                    // exactly as before this fix.
                                    //
                                    // Residual gap, deliberately not
                                    // closed here: if the settings MODAL's
                                    // OAuth flow completes while this
                                    // row's own popup is still open, the
                                    // list-level listener still clears
                                    // this shared signal to `None` — it
                                    // has no way to know the message
                                    // belonged to the modal's attempt, not
                                    // this row's — so this row goes silent
                                    // on its own genuine cancel/timeout
                                    // too. The row's state still clears
                                    // correctly (no spinner-forever); the
                                    // user just gets no toast for that one
                                    // overlapping attempt. This is
                                    // strictly milder than either the
                                    // spinner-forever bug or a false toast
                                    // on a real error, so it is the safe
                                    // direction to err — but it is NOT
                                    // fixed here: `OAuthMessage` carries
                                    // no datasource id, so "whose message
                                    // was that?" is unanswerable from
                                    // inside this row. Closing it needs a
                                    // datasource id added to the message
                                    // type, which touches `oauth_popup.rs`
                                    // and the modal, both out of this
                                    // PR's scope.
                                    if oauth_connecting.try_get_untracked().flatten().is_none() {
                                        return;
                                    }

                                    let ds_id_for_fetch = ds_id_for_recover.clone();
                                    leptos::task::spawn_local(async move {
                                        // No `fetch_oauth_status_once` /
                                        // OAuthStatusSource wiring exists at
                                        // the list level — this row's only
                                        // source of truth is the
                                        // "datasources" list query, so
                                        // recovery re-fetches it and
                                        // inspects this row's own
                                        // credential_status (KYO-440),
                                        // rather than re-checking a
                                        // per-provider OAuth status
                                        // endpoint like the modal does.
                                        let recovered = list_datasources()
                                            .await
                                            .ok()
                                            .and_then(|list| {
                                                list.into_iter()
                                                    .find(|d| d.id == ds_id_for_fetch)
                                            })
                                            .map(|d| {
                                                credential_status_indicates_connected(
                                                    &d.credential_status,
                                                )
                                            })
                                            .unwrap_or(false);
                                        if recovered {
                                            // Reachable ONLY when
                                            // `oauth_connecting` was
                                            // `Some(_)` at the moment
                                            // `on_outcome` fired (the guard
                                            // above returns early
                                            // otherwise) — i.e. no OAuth
                                            // `postMessage` had resolved
                                            // this attempt yet. So
                                            // `recovered == true` here is
                                            // NOT the redundant re-check on
                                            // a real, already-toasted
                                            // success that earlier
                                            // revisions of this comment
                                            // described (KYO-524 fixed
                                            // that path — see the guard
                                            // above): the postMessage
                                            // plumbing genuinely never
                                            // delivered anything for this
                                            // attempt, yet the backend's
                                            // credential_status flipped to
                                            // connected anyway (e.g. the
                                            // popup closed itself right at
                                            // consent completion, before
                                            // the message handler ran, or
                                            // the message was lost/blocked
                                            // in transit). This is the
                                            // genuine KYO-437 recovery
                                            // case this monitor exists
                                            // for. Adopt silently — no
                                            // toast — because the user
                                            // never saw an error or a
                                            // cancellation notice for this
                                            // attempt to begin with; the
                                            // row updating itself (via
                                            // cache invalidation) is
                                            // confirmation enough.
                                            set_oauth_recovered.try_set(true);
                                            query_cache.invalidate("datasources");
                                        } else {
                                            toast_error(popup_monitor_outcome_message(
                                                provider, outcome,
                                            ));
                                        }
                                        // Guarded: only clear if this row's
                                        // id is still the one recorded —
                                        // another row may have started its
                                        // own connect while this recheck
                                        // was in flight.
                                        set_oauth_connecting.try_update(|current| {
                                            if current.as_deref()
                                                == Some(ds_id_for_fetch.as_str())
                                            {
                                                *current = None;
                                            }
                                        });
                                    });
                                },
                            );
                            popup_monitor.update_value(|slot| {
                                // Drain before arming: invoke any cleanup
                                // already stashed here for this row before
                                // installing the new one, and clear this
                                // row's `connect_attempt_live` for the
                                // superseded attempt as part of the same
                                // drain — load-bearing, not merely
                                // defensive, since the click guard above
                                // can no longer be trusted to prevent a
                                // re-click on this row while an earlier
                                // attempt here is still unresolved (see
                                // `popup_monitor`'s own doc comment above
                                // for why).
                                if let Some(previous) = slot.take() {
                                    set_connect_attempt_live.set(false);
                                    previous.take()();
                                }
                                *slot = Some(send_wrapper::SendWrapper::new(
                                    Box::new(cleanup) as Box<dyn FnOnce()>,
                                ));
                            });
                            set_connect_attempt_live.set(true);
                        }
                        None => {
                            set_oauth_connecting.set(None);
                            toast_error(
                                "Popup was blocked. Please allow popups for this site.",
                            );
                        }
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = url;
                }
            };

            view! {
                <Button
                    variant=ButtonVariant::Outline
                    size=ButtonSize::Sm
                    // Reverted to this row's own state (KYO-440 cycle 4 —
                    // see `connect_attempt_live`'s doc comment above for
                    // why "any row connecting" stopped buying anything:
                    // the settings modal, one of the writers that guard
                    // was meant to close off from, never reads or writes
                    // this signal, so widening the disabled state to every
                    // row was a UX cost paid for an invariant that didn't
                    // hold). A different row can legitimately be
                    // connecting at the same time now — this button only
                    // reflects its own row's state.
                    disabled=Signal::derive(move || is_connecting.get() || is_deleting.get())
                    on:click=on_oauth_click
                >
                    {move || if is_connecting.get() {
                        view! {
                            <span class="flex items-center gap-1.5">
                                <Spinner size="h-3 w-3"/>
                                <span class="hidden sm:inline">{connecting_label}</span>
                            </span>
                        }.into_any()
                    } else {
                        view! {
                            <span class="flex items-center gap-1.5">
                                <span class="h-4 w-4 inline-flex items-center justify-center">
                                    <Icon icon=phosphor_leptos::PLUG/>
                                </span>
                                <span class="hidden sm:inline">{connect_label.clone()}</span>
                            </span>
                        }.into_any()
                    }}
                </Button>
            }
            .into_any()
        };

        // `class:hidden` lives here, inside the `Some` arm, not on a
        // wrapper the call site renders unconditionally (KYO-440 review
        // fix). `cred_action_view` is `None` for every row whose
        // `credential_status` is "valid"/"shared" — the majority — and
        // `oauth_recovered` starts `false` for every row, so a wrapper
        // rendered unconditionally at the call site would render as a
        // real, un-hidden, empty `<div>` for every one of those rows —
        // and CSS flex `gap` puts space around any rendered flex item,
        // content or not. Keeping the wrapper inside `Some` means `None`
        // still renders nothing at all, exactly as it did before
        // `oauth_recovered` existed.
        view! {
            <div class:hidden=move || oauth_recovered.get()>{button}</div>
        }
        .into_any()
    });

    view! {
        <div class=move || {
            // KYO-467 — visible in-progress state for the row currently being
            // deleted: dimmed (matches the Watch Card "disabled" convention
            // of 70% opacity — see DESIGN.md) and `pointer-events-none` so
            // mouse interaction with this row's other controls is inert for
            // the whole 5-10s round trip, not just the delete button itself.
            // `on_toggle`/`on_settings_click`/`on_oauth_click` each also
            // guard on `is_deleting` directly (see above) so a keyboard user
            // tabbing past `pointer-events-none` still can't act on a row
            // mid-delete.
            let base = "flex flex-col sm:flex-row sm:items-center sm:justify-between p-4 gap-3 \
                 hover:bg-muted/50 transition-colors transition-opacity duration-200";
            if is_deleting.get() {
                format!("{base} opacity-70 pointer-events-none")
            } else {
                base.to_string()
            }
        }>
            // Left side: name, type badge, status badges
            <div class="flex items-center gap-3 min-w-0">
                <span class="h-6 w-6 shrink-0 text-muted-foreground inline-flex items-center justify-center">
                    <Icon icon=phosphor_leptos::DATABASE/>
                </span>
                <div class="min-w-0">
                    <div class="flex flex-wrap items-center gap-1.5 sm:gap-2">
                        <span class="font-medium truncate">{ds.name.clone()}</span>
                        <span aria-hidden="true" class="text-muted-foreground">"·"</span>
                        <span class="text-sm text-muted-foreground">{ds.type_display_name.clone()}</span>
                        {(ds.connection_type == "connect").then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                "Connect"
                            </Badge>
                        })}
                        {ds.is_sample.then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                "Sample"
                            </Badge>
                        })}
                        {ds.is_analytics.then(|| view! {
                            <Badge variant=BadgeVariant::Secondary class="text-xs">
                                <span class="inline-flex items-center gap-1">
                                    <Icon icon=phosphor_leptos::PULSE size="12px"/>
                                    "Analytics"
                                </span>
                            </Badge>
                        })}
                        {cred_badge.map(|(text, variant)| view! {
                            <Badge variant=variant class="text-xs">
                                {text}
                            </Badge>
                        })}
                        {show_catalog_warning.then(|| view! {
                            <span class="h-4 w-4 text-warning-foreground inline-flex items-center justify-center" title="Catalog needs attention">
                                <Icon icon=phosphor_leptos::WARNING/>
                            </span>
                        })}
                    </div>
                    {(!ds.slug.is_empty()).then(|| view! {
                        <p class="text-xs text-muted-foreground font-mono truncate">
                            {ds.slug.clone()}
                        </p>
                    })}
                </div>
            </div>

            // Right side: credential action, toggle, settings, delete
            <div class="flex items-center gap-2 sm:gap-3 flex-wrap sm:flex-nowrap">
                // Credential action button — only rendered when credentials are
                // missing or expired. Provides the primary call-to-action for
                // datasources that cannot be enabled yet. `cred_action_view`
                // is `None` at the call site whenever `cred_action` is
                // `None`, so a row with valid/shared credentials renders
                // nothing here — exactly as before KYO-440. The
                // `class:hidden` override (`oauth_recovered`, above — popup-
                // monitor recovery can find the connection actually
                // succeeded after the button was already rendered, and
                // `<For>` won't re-render this row from fresh data alone)
                // lives on the wrapper `<div>` inside `cred_action_view`'s
                // `Some` arm instead, so it never adds a phantom flex-`gap`
                // item to a row that has no button to hide.
                {cred_action_view}

                // User enable/disable toggle
                <div class="flex items-center gap-2">
                    <span class="text-xs text-muted-foreground hidden sm:inline">
                        {toggle_label}
                    </span>
                    <Switch
                        checked=Signal::from(local_enabled)
                        on_change=on_toggle
                        disabled=switch_disabled
                        class=if !can_enable { "opacity-50 cursor-not-allowed".to_string() } else { String::new() }
                    />
                </div>

                // Settings button — opens modal (or analytics settings link for analytics datasources)
                {if ds.is_analytics {
                    view! {
                        // The link is only rendered when the caller can actually use
                        // the page it routes to (KYO-260) — otherwise a non-admin
                        // member sees nothing in this slot (they still have the
                        // enable/disable Switch and credential-action button above).
                        // `<Show>` (not `.then()`) because `analytics_access` is a
                        // reactive Signal, unlike the static `ds.is_analytics` check
                        // that selects this branch — same distinction as the delete
                        // button's `<Show>` below.
                        <Show when=move || matches!(analytics_access.get(), AnalyticsAccess::Allowed)>
                            <ButtonLink
                                href="/settings/analytics"
                                variant=ButtonVariant::Outline
                                size=ButtonSize::Sm
                            >
                                <span class="h-4 w-4 sm:mr-1 inline-flex items-center justify-center">
                                    <Icon icon=phosphor_leptos::PULSE/>
                                </span>
                                <span class="hidden sm:inline">"Analytics Settings"</span>
                            </ButtonLink>
                        </Show>
                    }.into_any()
                } else {
                    view! {
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=is_deleting
                            on:click=on_settings_click
                        >
                            <span class="h-4 w-4 sm:mr-1 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::GEAR/>
                            </span>
                            <span class="hidden sm:inline">"Settings"</span>
                        </Button>
                    }.into_any()
                }}

                // Delete button — hidden for analytics datasources (lifecycle-managed
                // by analytics site CRUD) AND for non-admins (`delete_datasource` →
                // `Permission::ManageDatasources`). `<Show>` (not `.then()`) because
                // `is_admin` is a reactive Signal, unlike the static `is_analytics`
                // check it's combined with.
                //
                // KYO-467 — while `is_deleting` is true, the trash icon swaps for a
                // `<Spinner>` and the button is disabled. This is per-row (gated on
                // `is_deleting`, not `delete_pending` action-wide) so a row that
                // *isn't* being deleted keeps a live, clickable trash button — only
                // `on_delete_click`'s action-wide guard above stops it from actually
                // starting a second delete while one is in flight.
                <Show when=move || is_admin.get() && !ds.is_analytics>
                    <Button
                        variant=ButtonVariant::GhostDestructive
                        size=ButtonSize::Icon
                        disabled=is_deleting
                        aria_label="Delete datasource"
                        on:click=on_delete_click.clone()
                    >
                        {move || if is_deleting.get() {
                            view! { <Spinner size="h-4 w-4"/> }.into_any()
                        } else {
                            view! { <Icon icon=phosphor_leptos::TRASH/> }.into_any()
                        }}
                    </Button>
                </Show>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Datasource Modal
// ─────────────────────────────────────────────────────────────────────────────

/// CSS class for text input fields — matches the React modal input style.
const MODAL_INPUT_CLASS: &str =
    "w-full px-3 py-2 border border-input rounded-md bg-background text-sm text-foreground focus:outline-none focus:ring-1 focus:ring-ring";

/// The `connection_config` key Azure Synapse's endpoint field is stored
/// under. Every other host-taking datasource type (`postgres`, `sqlserver`,
/// `databricks`'s `server_hostname`, ...) uses `"host"`, but the Synapse
/// driver (`kyomi-connect` `crates/kyomi-datasource/src/providers/synapse.rs`)
/// requires `"server"` specifically and rejects `"host"` with
/// `Error::Provider("Azure Synapse requires a server address")`. This
/// constant is the single place that key is spelled on the UI side — both
/// `build_connection_config`'s `"synapse"` arm (write) and the edit-mode
/// load-back (read) reference it, so a future rename on either side breaks
/// a compile-time reference here rather than silently reintroducing the
/// write/read key mismatch that made every Leptos-created Synapse
/// datasource permanently unusable (KYO-516). It cannot pin the *driver's*
/// expectation — that lives in a different repo — see the KYO-516 test
/// module's doc comment for what this constant does and does not catch.
const SYNAPSE_SERVER_CONFIG_KEY: &str = "server";

/// Tab button active class.
const TAB_ACTIVE: &str =
    "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors border-primary text-primary";

/// Tab button inactive class.
const TAB_INACTIVE: &str =
    "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors border-transparent text-muted-foreground hover:text-foreground";

/// Tab button disabled class.
const TAB_DISABLED: &str =
    "px-4 py-2 text-sm font-medium border-b-2 -mb-px transition-colors border-transparent text-muted-foreground opacity-50 cursor-not-allowed";

/// Datasource types for the create-mode type selector.
/// Matches the values from `get_datasource_types()`.
const PROVIDER_TYPES: &[(&str, &str)] = &[
    ("bigquery", "BigQuery"),
    ("postgres", "PostgreSQL"),
    ("mysql", "MySQL"),
    ("clickhouse", "ClickHouse"),
    ("snowflake", "Snowflake"),
    ("databricks", "Databricks"),
    ("redshift", "Amazon Redshift"),
    ("sqlserver", "SQL Server"),
    ("synapse", "Azure Synapse"),
    ("flaredb", "FlareDB"),
];

// ─────────────────────────────────────────────────────────────────────────────
// Connection auth-mode registry fetch (KYO-274 review follow-up)
//
// `get_datasource_types()` backs the four connection Authentication Mode
// selectors (`*AuthModeSection` below). A failed fetch used to be silently
// discarded via `.ok()`, so a network blip made all four selectors render
// with zero options and no explanation. `connection_auth_modes_unavailable_from`
// is the pure, testable extraction of "did the fetch fail?" (logging on
// failure); see the `Memo` at its call site for why the failure check must
// stay on its own reactive scope rather than living inside the per-type
// `connection_auth_modes` derive.
// ─────────────────────────────────────────────────────────────────────────────

/// Whether the datasource-type registry fetch backing the four connection
/// Authentication Mode selectors has resolved to an error. `None` (still
/// loading) and `Some(Ok(_))` (succeeded) both resolve to `false` — only a
/// resolved error counts as "unavailable". Logs on failure so it isn't
/// otherwise invisible; stays silent while merely loading.
fn connection_auth_modes_unavailable_from(
    datasource_types: &Option<Result<Vec<DatasourceTypeInfo>, ServerFnError>>,
) -> bool {
    match datasource_types {
        Some(Err(e)) => {
            leptos::logging::warn!(
                "failed to load datasource-type registry — connection auth mode \
                 options unavailable: {e}"
            );
            true
        }
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BigQuery project-discovery reset (KYO-468 review follow-up)
//
// `bq_projects`, `bq_projects_error`, and `bq_projects_attempted` travel
// together everywhere in this file: `bq_projects.is_empty()` alone can't
// distinguish "never attempted" from "attempted and came back empty" from
// "attempted and failed" — only `bq_projects_attempted` (whether a listing
// run has actually started/completed) and `bq_projects_error` (why it
// failed, if it did) disambiguate those. Three separate review cycles each
// found one more site that cleared some but not all three: the auth-mode
// and `ds_type` `on_change` handlers (cycle 1), the service_account "Remove"
// chip (cycle 2), and `do_test_and_discover` plus both OAuth-disconnect
// Effects (cycle 3, KYO-468). Patching sites individually only bought one
// more cycle each time, so every reset site now funnels through one of
// these two functions instead of writing the three signals inline —
// resetting one or two of the three without the other is no longer
// possible without skipping the call entirely.
//
// Two variants, not one, because this file's disposal-safety convention
// (see `BqProjectField`'s doc comment below, KYO-500/KYO-429) depends on
// *where* the write happens: same-scope writes (inside `DatasourceModal`
// itself, which owns these signals) use `.set()` so a genuine disposal bug
// still panics instead of failing silently; writes crossing into a child
// component's props (`BigQueryAuthModeSection`) use `.try_set()` because the
// parent may have already begun tearing down. Population sites — anywhere
// that writes real discovered data, a `Some(...)` error, or flips
// `bq_projects_attempted` to `true` — are deliberately untouched by both.

/// Resets all three BigQuery project-discovery signals together. For sites
/// in `DatasourceModal`'s own scope (the same scope that owns the signals).
fn reset_bq_projects_signals(
    set_bq_projects: WriteSignal<Vec<(String, String)>>,
    set_bq_projects_error: WriteSignal<Option<String>>,
    set_bq_projects_attempted: WriteSignal<bool>,
) {
    set_bq_projects.set(vec![]);
    set_bq_projects_error.set(None);
    set_bq_projects_attempted.set(false);
}

/// Resets all three BigQuery project-discovery signals together, via
/// `try_set`. For sites in a child component (e.g. `BigQueryAuthModeSection`)
/// writing across the parent/child signal boundary from a plain event
/// handler, where the parent scope may already be disposed.
fn try_reset_bq_projects_signals(
    set_bq_projects: WriteSignal<Vec<(String, String)>>,
    set_bq_projects_error: WriteSignal<Option<String>>,
    set_bq_projects_attempted: WriteSignal<bool>,
) {
    set_bq_projects.try_set(vec![]);
    set_bq_projects_error.try_set(None);
    set_bq_projects_attempted.try_set(false);
}

/// Modal state for create/edit mode.
///
/// Matches `apps/frontend/src/components/settings/DatasourceModal.jsx`.
#[component]
pub fn DatasourceModal(
    /// Whether the modal is open.
    open: Signal<bool>,
    /// None = create mode, Some(id) = edit mode.
    datasource_id: Signal<Option<String>>,
    /// Called when modal is closed.
    on_close: Callback<()>,
    /// Called when a datasource was successfully saved.
    on_saved: Callback<DatasourceResult>,
) -> impl IntoView {
    // ── Permission context ────────────────────────────────────────────────
    // Shared resource provided by the parent Layout (see settings_shell.rs).
    // Gates the SSH Tunnel section (ManageDatasources-only, SSH-capable types
    // only — see `supports_ssh_tunnel`) as well as the Add/Edit/Save surfaces
    // below: the header/empty-state "Add Datasource" CTAs, the delete button,
    // the edit-mode connection-config fields, and the Catalog tab (KYO-184).
    let perms = use_permissions();
    let is_admin = Signal::derive(move || perms.can(Permission::ManageDatasources));

    // `datasource_id` is `Some` only in edit mode. Used by `SshTunnelSection`
    // to pick placeholder copy for the BYOK private-key/passphrase fields
    // ("leave blank to keep the existing key" only makes sense once a key
    // already exists server-side).
    let is_edit_mode = Signal::derive(move || datasource_id.get().is_some());

    // ── Form state ───────────────────────────────────────────────────────
    let (name, set_name) = signal(String::new());
    let (slug, set_slug) = signal(String::new());
    let (slug_manually_edited, set_slug_manually_edited) = signal(false);
    let (ds_type, set_ds_type) = signal("bigquery".to_string());

    // connection_config fields (stored individually so Leptos reactivity works)
    // host, port, ssl_mode, database, schema, warehouse, account, role,
    // catalog, server_hostname, http_path, secure, encrypt, trust_server_certificate
    // bigquery: auth_mode, oauth_client_id, oauth_client_secret, service_account_json
    // snowflake: auth_mode
    let (cfg_host, set_cfg_host) = signal(String::new());
    let (cfg_port, set_cfg_port) = signal(String::new());
    let (cfg_ssl_mode, set_cfg_ssl_mode) = signal("require".to_string());
    let (cfg_database, set_cfg_database) = signal(String::new());
    let (cfg_schema, set_cfg_schema) = signal(String::new());
    let (cfg_warehouse, set_cfg_warehouse) = signal(String::new());
    let (cfg_account, set_cfg_account) = signal(String::new());
    let (cfg_role, set_cfg_role) = signal(String::new());
    let (cfg_catalog, set_cfg_catalog) = signal(String::new());
    let (cfg_server_hostname, set_cfg_server_hostname) = signal(String::new());
    let (cfg_http_path, set_cfg_http_path) = signal(String::new());
    let (cfg_secure, set_cfg_secure) = signal(false);
    let (cfg_encrypt, set_cfg_encrypt) = signal(true);
    let (cfg_trust_cert, set_cfg_trust_cert) = signal(false);
    let (cfg_shared_credentials, set_cfg_shared_credentials) = signal(false);

    // Indexing credentials — dedicated credentials for catalog indexing,
    // separate from the user's primary OAuth/password credentials. Required
    // for OAuth datasources because background jobs cannot refresh tokens.
    let (use_indexing_credentials, set_use_indexing_credentials) = signal(false);
    let (indexing_creds_type, set_indexing_creds_type) = signal(String::new());
    let (indexing_creds_json, set_indexing_creds_json) = signal(String::new());
    let (indexing_username, set_indexing_username) = signal(String::new());
    let (indexing_password, set_indexing_password) = signal(String::new());
    let (indexing_token, set_indexing_token) = signal(String::new());
    let (indexing_client_id, set_indexing_client_id) = signal(String::new());
    let (indexing_client_secret, set_indexing_client_secret) = signal(String::new());
    let (indexing_tenant_id, set_indexing_tenant_id) = signal(String::new());
    // True when the loaded datasource had masked indexing credentials and the
    // user hasn't modified them. On save, we must send MASKED_VALUE to
    // preserve the stored encrypted blob instead of re-submitting empty fields.
    let (indexing_creds_unchanged, set_indexing_creds_unchanged) = signal(false);

    // SSH tunnel state — admin-only, SSH-capable types only (see
    // `supports_ssh_tunnel`). `cfg_ssh_port` is kept as a `String` (like
    // `cfg_port`) and parsed at build time, defaulting to "22".
    // `ssh_public_key` / `ssh_private_key_generated` are populated by a freshly
    // generated keypair this session. The private key held here is PLAINTEXT
    // (`generate_ssh_key` returns plaintext; the save path encrypts it at rest
    // via `finalize_connection_config_secrets`). It is force-masked on read
    // server-side, so it is never loaded back on edit (see
    // `build_connection_config`, which only writes `ssh_private_key` when
    // `ssh_private_key_generated` is `Some`, to avoid clobbering the stored
    // value with the mask).
    let (cfg_ssh_enabled, set_cfg_ssh_enabled) = signal(false);
    let (cfg_ssh_host, set_cfg_ssh_host) = signal(String::new());
    let (cfg_ssh_port, set_cfg_ssh_port) = signal("22".to_string());
    let (cfg_ssh_username, set_cfg_ssh_username) = signal(String::new());
    let (ssh_public_key, set_ssh_public_key) = signal::<Option<String>>(None);
    let (ssh_private_key_generated, set_ssh_private_key_generated) = signal::<Option<String>>(None);
    let (ssh_key_generating, set_ssh_key_generating) = signal(false);
    // Non-sensitive: pinned bastion host key fingerprint (KYO-133). Written
    // to `connection_config.ssh_host_fingerprint` as a plain string — never
    // encrypted, never masked.
    let (cfg_ssh_host_fingerprint, set_cfg_ssh_host_fingerprint) = signal(String::new());
    // Key-source choice (KYO-134): "generate" (default — current UX, Kyomi
    // generates and holds the keypair) or "byok" (user pastes their own
    // private key, optionally passphrase-protected). `cfg_ssh_private_key_input`
    // and `cfg_ssh_passphrase` are BYOK-only fields — like the generated
    // private key, the stored values are force-masked server-side
    // (`ssh_passphrase` is in `COMMON_SENSITIVE` alongside `ssh_private_key`)
    // so they are deliberately never loaded back on edit; both start empty
    // with a "leave blank to keep the existing key" placeholder instead.
    let (cfg_ssh_key_mode, set_cfg_ssh_key_mode) = signal("generate".to_string());
    let (cfg_ssh_private_key_input, set_cfg_ssh_private_key_input) = signal(String::new());
    let (cfg_ssh_passphrase, set_cfg_ssh_passphrase) = signal(String::new());

    // BigQuery-specific
    let (bq_auth_mode, set_bq_auth_mode) = signal("kyomi_oauth".to_string());
    let (cfg_oauth_client_id, set_cfg_oauth_client_id) = signal(String::new());
    let (cfg_oauth_client_secret, set_cfg_oauth_client_secret) = signal(String::new());
    let (cfg_service_account_json, set_cfg_service_account_json) = signal(String::new());
    let (service_account_email, set_service_account_email) = signal(String::new());
    // KYO-408 — user has ticked the beta-access confirmation checkbox for
    // the kyomi_oauth mode's Google-account allowlist (see the checkbox's
    // own copy in the view tree below — deliberately not quoted here; this
    // doc comment is scanned as part of the file by
    // `utils::beta_access`'s whole-file structural tests, and an echoed
    // copy string here would let a regression in the real markup pass
    // unnoticed). Persisted to
    // `localStorage["hasBetaAccess"]` via `utils::beta_access` (KYO-499),
    // matching the React original — an earlier attempt (KYO-478) shipped
    // this deliberately NOT persisted, on the reasoning that persistence
    // would make the attestation "look real but be pre-satisfied"; that
    // reasoning was wrong (this was never a security control — Google's
    // allowlist is the actual enforcement) and React always persisted it.
    // Re-read from storage every time the modal opens (`reset_form` in
    // create mode, the edit-mode settings load below) rather than left
    // untouched, so a value ticked on another tab/surface since this modal
    // last opened is picked up. Deliberately separate from `bq_auth_mode` /
    // `modal_oauth_connected` rather than folded into either — this is
    // purely the user's self-report, not derived connection state.
    let (bq_access_confirmed, set_bq_access_confirmed) = signal(beta_access::read_beta_access());

    // Snowflake-specific
    let (sf_auth_mode, set_sf_auth_mode) = signal("password".to_string());

    // Databricks-specific
    let (db_auth_mode, set_db_auth_mode) = signal("token".to_string());

    // Synapse-specific
    // auth_mode: "sql" | "service_principal" | "enterprise_oauth"
    let (synapse_auth_mode, set_synapse_auth_mode) = signal("sql".to_string());
    // Tenant ID — used for both service_principal and enterprise_oauth modes
    let (cfg_tenant_id, set_cfg_tenant_id) = signal(String::new());
    // Service Principal credentials (distinct from cred_client_id used for other purposes)
    let (cred_sp_client_id, set_cred_sp_client_id) = signal(String::new());
    let (cred_sp_client_secret, set_cred_sp_client_secret) = signal(String::new());

    // Credentials form
    let (cred_username, set_cred_username) = signal(String::new());
    let (cred_password, set_cred_password) = signal(String::new());
    // Tracks whether a password is already stored server-side for this
    // datasource (edit mode only). Drives the "stored" placeholder hint on
    // the password field so the user knows they don't have to re-type it.
    let (cred_password_stored, set_cred_password_stored) = signal(false);
    let (cred_access_token, set_cred_access_token) = signal(String::new());
    let (cred_private_key, set_cred_private_key) = signal(String::new());
    let (cred_billing_project, set_cred_billing_project) = signal(String::new());

    // ── Tab state ────────────────────────────────────────────────────────
    // "connection" or "catalog"
    let (active_tab, set_active_tab) = signal("connection".to_string());

    // ── Operation state ──────────────────────────────────────────────────
    let (test_result, set_test_result) = signal::<Option<TestConnectionResult>>(None);
    let (settings_loading, set_settings_loading) = signal(false);
    let (error_msg, set_error_msg) = signal::<Option<String>>(None);

    // ── Connect datasource state ─────────────────────────────────────────
    // `connection_type` is `"direct"` for standard provider datasources and
    // `"connect"` for Kyomi Connect agent datasources. In edit mode we branch
    // on this to swap the connection/auth form for `ConnectStatusPanel`.
    // Used by BOTH modes: loaded from `settings.connection_type` in edit mode,
    // driven by the create-mode Direct / Kyomi Connect toggle in create mode.
    let (connection_type, set_connection_type) = signal::<String>("direct".to_string());

    // ── Create-mode Connect flow state ───────────────────────────────────
    // `connect_token`: populated after `create_connect_datasource` succeeds;
    // presence of a token swaps the modal body to the post-create view
    // (token display + deployment tabs + Done button).
    // `connect_created_name` / `connect_created_type`: snapshot of the
    // datasource we just created (so we can render its name in the post-
    // create header and its default port in the deployment commands).
    // `creating_connect`: in-flight flag for the create server_fn — disables
    // the submit button and swaps its label for "Creating...".
    // `active_deploy_tab`: which of the four deployment tabs is active in
    // the post-create view. Default `"linux"` (matches `ConnectStatusPanel`).
    let (connect_token, set_connect_token) = signal::<Option<String>>(None);
    let (connect_created_name, set_connect_created_name) = signal::<String>(String::new());
    let (connect_created_type, set_connect_created_type) = signal::<String>(String::new());
    let (creating_connect, set_creating_connect) = signal(false);
    let (active_deploy_tab, set_active_deploy_tab) = signal::<String>("linux".to_string());

    // ── Sample datasource state ──────────────────────────────────────────
    // `is_sample`: true when editing an existing sample datasource (loaded from
    // connection_config.is_sample). Gates the read-only sample view.
    // `sample_available`: sample ClickHouse is configured on the server.
    // `sample_already_added`: current workspace already has a sample datasource.
    // `creating_sample`: in-flight POST for quick-add.
    let (is_sample, set_is_sample) = signal(false);
    let (sample_available, set_sample_available) = signal(false);
    let (sample_already_added, set_sample_already_added) = signal(false);
    let (creating_sample, set_creating_sample) = signal(false);

    // ── Discovery state ──────────────────────────────────────────────────
    // "idle", "loading", "success", "error"
    let (discovery_status, set_discovery_status) = signal("idle".to_string());
    let (discovered_databases, set_discovered_databases) = signal::<Vec<String>>(vec![]);
    let (discovered_schemas, set_discovered_schemas) = signal::<Vec<String>>(vec![]);
    let (discovered_warehouses, set_discovered_warehouses) = signal::<Vec<String>>(vec![]);
    let (discovered_catalogs, set_discovered_catalogs) = signal::<Vec<String>>(vec![]);
    // KYO-474: true only when the last Test & Discover attempt succeeded at
    // the connection level but `resource_errors` (KYO-466) named a denial
    // for *this type's* catalog-scope key specifically — never inferred
    // from `discovered_*` being empty, which is also true for "not
    // attempted yet" and "succeeded, genuinely nothing there" (KYO-452
    // still owns the copy for both of those). Read once here, in the same
    // Effect that already reads `resource_errors` for `bq_projects_error`
    // below, so `CreateModeCatalogPicker` never re-derives this fact from
    // the raw resources map on its own.
    let (catalog_discovery_denied, set_catalog_discovery_denied) = signal(false);

    // ── Catalog tab state (edit mode) ────────────────────────────────────
    // Selected catalog scope items (projects / databases / schemas / catalogs).
    // Stored at modal level so `build_connection_config` can include them when
    // saving from the Connection tab after the user configures them on the
    // Catalog tab.
    let (catalog_selected, set_catalog_selected) = signal::<Vec<String>>(vec![]);
    let (catalog_scope_touched, set_catalog_scope_touched) = signal(false);
    // BigQuery-specific: whether to include public datasets in catalog indexing.
    let (bq_include_public, set_bq_include_public) = signal(false);

    // ── Catalog tab state (create mode) ──────────────────────────────────
    // These parallel the edit-mode signals above but are written only during
    // create mode (discovery already ran on the Connection tab).  They are
    // included in `build_connection_config` only when `is_create_mode` is true.
    let (create_catalog_selected, set_create_catalog_selected) = signal::<Vec<String>>(vec![]);
    let (create_catalog_text, set_create_catalog_text) = signal::<String>(String::new());
    let (create_include_public_datasets, set_create_include_public_datasets) = signal(false);

    // ── Modal-level OAuth status state ───────────────────────────────────
    // Tracks the OAuth connection status for whichever provider is active in
    // the modal (BigQuery kyomi_oauth, BigQuery enterprise_oauth, Snowflake,
    // or Databricks).  Seeded from `DatasourceSettingsResult` on modal open,
    // then refreshed by a separate `spawn_local` status fetch.
    //
    // These are distinct from the list-level `oauth_connecting` signal that
    // only tracks whether a popup is open for a given row.
    let (modal_oauth_connected, set_modal_oauth_connected) = signal(false);
    let (modal_oauth_email, set_modal_oauth_email) = signal::<Option<String>>(None);
    let (modal_oauth_expired, set_modal_oauth_expired) = signal(false);
    let (modal_oauth_connecting, set_modal_oauth_connecting) = signal(false);

    // ── BigQuery project list (fetched after OAuth connects, or after a
    // successful Test & Discover) ──────────────────────────────────────
    // Populated by kyomi_oauth's post-connect fetch (below) or by
    // service_account's "Validate & Discover Projects" (KYO-405, via
    // `test_action`'s Effect). Never enterprise_oauth — its per-datasource
    // organizational token can't list personal GCP projects, so it never
    // attempts this at all (see the OAuth-connect Effect's comment below).
    // Used to drive a Select dropdown for billing_project instead of a
    // free-text input (`BqProjectField`), and — KYO-468 — to drive
    // `CreateModeCatalogPicker`'s BigQuery checkbox list in create mode.
    let (bq_projects, set_bq_projects) = signal::<Vec<(String, String)>>(vec![]);
    let (bq_projects_loading, set_bq_projects_loading) = signal(false);
    let (bq_projects_error, set_bq_projects_error) = signal::<Option<String>>(None);
    // KYO-468: true once a BigQuery project-listing attempt has actually
    // started/completed for kyomi_oauth or service_account — never
    // inferred from `bq_projects.is_empty()` alone, which is also true
    // for "never attempted" (enterprise_oauth) and "attempted, genuinely
    // empty". `CreateModeCatalogPicker` reads this once, passed down as a
    // prop, exactly like `catalog_discovery_denied` above — never
    // re-derived locally.
    let (bq_projects_attempted, set_bq_projects_attempted) = signal(false);

    // ── Reset form ───────────────────────────────────────────────────────
    let reset_form = move || {
        set_name.set(String::new());
        set_slug.set(String::new());
        set_slug_manually_edited.set(false);
        set_ds_type.set("bigquery".to_string());
        set_cfg_host.set(String::new());
        set_cfg_port.set(String::new());
        set_cfg_ssl_mode.set("require".to_string());
        set_cfg_database.set(String::new());
        set_cfg_schema.set(String::new());
        set_cfg_warehouse.set(String::new());
        set_cfg_account.set(String::new());
        set_cfg_role.set(String::new());
        set_cfg_catalog.set(String::new());
        set_cfg_server_hostname.set(String::new());
        set_cfg_http_path.set(String::new());
        set_cfg_secure.set(false);
        set_cfg_encrypt.set(true);
        set_cfg_trust_cert.set(false);
        set_cfg_shared_credentials.set(false);
        set_cfg_ssh_enabled.set(false);
        set_cfg_ssh_host.set(String::new());
        set_cfg_ssh_port.set("22".to_string());
        set_cfg_ssh_username.set(String::new());
        set_ssh_public_key.set(None);
        set_ssh_private_key_generated.set(None);
        set_ssh_key_generating.set(false);
        set_cfg_ssh_host_fingerprint.set(String::new());
        set_cfg_ssh_key_mode.set("generate".to_string());
        set_cfg_ssh_private_key_input.set(String::new());
        set_cfg_ssh_passphrase.set(String::new());
        set_bq_auth_mode.set("kyomi_oauth".to_string());
        set_cfg_oauth_client_id.set(String::new());
        set_cfg_oauth_client_secret.set(String::new());
        set_cfg_service_account_json.set(String::new());
        set_service_account_email.set(String::new());
        // KYO-499 — re-read from localStorage rather than hardcoding false,
        // so a value ticked on another tab/surface since this modal was
        // last open is honored; see `bq_access_confirmed`'s own doc comment.
        set_bq_access_confirmed.set(beta_access::read_beta_access());
        set_sf_auth_mode.set("password".to_string());
        set_db_auth_mode.set("token".to_string());
        set_synapse_auth_mode.set("sql".to_string());
        set_cfg_tenant_id.set(String::new());
        set_cred_sp_client_id.set(String::new());
        set_cred_sp_client_secret.set(String::new());
        set_cred_username.set(String::new());
        set_cred_password.set(String::new());
        set_cred_password_stored.set(false);
        set_cred_access_token.set(String::new());
        set_cred_private_key.set(String::new());
        set_cred_billing_project.set(String::new());
        set_active_tab.set("connection".to_string());
        set_test_result.set(None);
        set_error_msg.set(None);
        set_discovery_status.set("idle".to_string());
        set_discovered_databases.set(vec![]);
        set_discovered_schemas.set(vec![]);
        set_discovered_warehouses.set(vec![]);
        set_discovered_catalogs.set(vec![]);
        set_catalog_discovery_denied.set(false);
        set_is_sample.set(false);
        set_connection_type.set("direct".to_string());
        set_connect_token.set(None);
        set_connect_created_name.set(String::new());
        set_connect_created_type.set(String::new());
        set_creating_connect.set(false);
        set_active_deploy_tab.set("linux".to_string());
        // Don't reset sample_available / sample_already_added here — those are
        // refreshed by an async effect when the modal opens in create mode.
        set_creating_sample.set(false);
        set_catalog_selected.set(vec![]);
        set_catalog_scope_touched.set(false);
        set_bq_include_public.set(false);
        set_create_catalog_selected.set(vec![]);
        set_create_catalog_text.set(String::new());
        set_create_include_public_datasets.set(false);
        set_modal_oauth_connected.set(false);
        set_modal_oauth_email.set(None);
        set_modal_oauth_expired.set(false);
        set_modal_oauth_connecting.set(false);
        set_bq_projects_loading.set(false);
        reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);
        set_use_indexing_credentials.set(false);
        set_indexing_creds_type.set(String::new());
        set_indexing_creds_json.set(String::new());
        set_indexing_username.set(String::new());
        set_indexing_password.set(String::new());
        set_indexing_token.set(String::new());
        set_indexing_client_id.set(String::new());
        set_indexing_client_secret.set(String::new());
        set_indexing_tenant_id.set(String::new());
        set_indexing_creds_unchanged.set(false);
    };

    // ── Load settings when switching to edit mode ─────────────────────────
    Effect::new(move |_| {
        if !open.get() {
            // Modal closed — clear the Connect post-create state now, while the
            // success view is unmounted (Modal gates children on `show`). If we
            // left a stale token here, the next open would briefly re-mount the
            // success view against it before `reset_form` runs, and the token
            // accessor could observe the Some→None transition mid-render (KYO-121).
            set_connect_token.set(None);
            set_connect_created_name.set(String::new());
            set_connect_created_type.set(String::new());
            return;
        }
        let id = datasource_id.get();
        match id {
            None => {
                // Create mode — reset form
                reset_form();
            }
            Some(ds_id) => {
                // Edit mode — load settings
                set_settings_loading.set(true);
                set_active_tab.set("connection".to_string());
                set_test_result.set(None);
                set_error_msg.set(None);
                set_discovery_status.set("idle".to_string());
                set_catalog_scope_touched.set(false);
                // KYO-408/KYO-499 — re-read per modal-open, same as create
                // mode's reset_form(); see bq_access_confirmed's own doc
                // comment for why this reads localStorage rather than
                // hardcoding false.
                set_bq_access_confirmed.set(beta_access::read_beta_access());

                leptos::task::spawn_local(async move {
                    match get_datasource_settings(ds_id).await {
                        Ok(settings) => {
                            set_name.try_set(settings.name.clone());
                            set_slug.try_set(settings.slug.clone());
                            set_ds_type.try_set(settings.datasource_type.clone());
                            set_connection_type.try_set(settings.connection_type.clone());

                            // Load connection_config fields
                            let cfg = &settings.connection_config;
                            let str_val = |key: &str| -> String {
                                cfg.get(key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            };
                            let bool_val = |key: &str| -> bool {
                                cfg.get(key)
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false)
                            };
                            // Synapse stores its endpoint under
                            // SYNAPSE_SERVER_CONFIG_KEY ("server"), not "host" — see
                            // that constant's doc comment for why (KYO-516).
                            set_cfg_host.try_set(if settings.datasource_type == "synapse" {
                                str_val(SYNAPSE_SERVER_CONFIG_KEY)
                            } else {
                                str_val("host")
                            });
                            // Legacy datasources (created by the old React frontend)
                            // store port as a JSON string (e.g. "5439"). Try the
                            // number shape first, then fall back to string so the
                            // field isn't left blank on edit.
                            set_cfg_port.try_set(
                                cfg.get("port")
                                    .and_then(|v| {
                                        v.as_i64()
                                            .map(|n| n.to_string())
                                            .or_else(|| v.as_str().map(str::to_string))
                                    })
                                    .unwrap_or_default(),
                            );
                            set_cfg_ssl_mode.try_set(
                                if str_val("ssl_mode").is_empty() {
                                    "require".to_string()
                                } else {
                                    str_val("ssl_mode")
                                }
                            );
                            set_cfg_database.try_set(str_val("database"));
                            set_cfg_schema.try_set(str_val("schema"));
                            set_cfg_warehouse.try_set(str_val("warehouse"));
                            set_cfg_account.try_set(str_val("account"));
                            set_cfg_role.try_set(str_val("role"));
                            set_cfg_catalog.try_set(str_val("catalog"));
                            set_cfg_server_hostname.try_set(str_val("server_hostname"));
                            set_cfg_http_path.try_set(str_val("http_path"));
                            set_cfg_secure.try_set(bool_val("secure"));
                            set_cfg_encrypt.try_set(
                                cfg.get("encrypt")
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(true),
                            );
                            set_cfg_trust_cert.try_set(bool_val("trust_server_certificate"));
                            set_is_sample.try_set(bool_val("is_sample"));
                            set_cfg_shared_credentials.try_set(settings.shared_credentials);

                            // SSH tunnel — `ssh_private_key` and `ssh_passphrase`
                            // are force-masked server-side (COMMON_SENSITIVE) so
                            // they are deliberately NOT loaded back here; only the
                            // public key (safe to display) and non-sensitive
                            // connection fields are restored.
                            let ssh_enabled_val = bool_val("ssh_enabled");
                            set_cfg_ssh_enabled.try_set(ssh_enabled_val);
                            set_cfg_ssh_host.try_set(str_val("ssh_host"));
                            set_cfg_ssh_username.try_set(str_val("ssh_username"));
                            set_cfg_ssh_port.try_set(
                                cfg.get("ssh_port")
                                    .and_then(|v| {
                                        v.as_i64()
                                            .map(|n| n.to_string())
                                            .or_else(|| v.as_str().map(str::to_string))
                                    })
                                    .filter(|s| !s.is_empty())
                                    .unwrap_or_else(|| "22".to_string()),
                            );
                            set_cfg_ssh_host_fingerprint.try_set(str_val("ssh_host_fingerprint"));
                            let ssh_public_key_val = cfg
                                .get("ssh_public_key")
                                .and_then(|v| v.as_str())
                                .map(str::to_string);
                            // Key-source heuristic (KYO-134): the stored private
                            // key is masked and can't tell us which mode was used,
                            // but the public key is only ever present for a
                            // Kyomi-generated keypair (BYOK never writes one back).
                            // So: public key present → "generate"; SSH enabled but
                            // no public key → the user brought their own → "byok".
                            set_cfg_ssh_key_mode.try_set(
                                if ssh_public_key_val.is_some() {
                                    "generate".to_string()
                                } else if ssh_enabled_val {
                                    "byok".to_string()
                                } else {
                                    "generate".to_string()
                                },
                            );
                            set_ssh_public_key.try_set(ssh_public_key_val);

                            set_cfg_oauth_client_id.try_set(str_val("oauth_client_id"));
                            set_cfg_oauth_client_secret.try_set(str_val("oauth_client_secret"));

                            // Auth mode — branched by datasource type so each
                            // provider's signal gets its own authoritative value.
                            if let Some(ref auth_mode) = settings.auth_mode {
                                match settings.datasource_type.as_str() {
                                    "bigquery" => {
                                        set_bq_auth_mode.try_set(auth_mode.clone());
                                    }
                                    "snowflake" => {
                                        set_sf_auth_mode.try_set(auth_mode.clone());
                                    }
                                    "databricks" => {
                                        set_db_auth_mode.try_set(auth_mode.clone());
                                    }
                                    "synapse" => {
                                        set_synapse_auth_mode.try_set(auth_mode.clone());
                                    }
                                    _ => {}
                                }
                            }

                            // Service account
                            if let Some(ref email) = settings.service_account_email {
                                set_service_account_email.try_set(email.clone());
                            }

                            // Load catalog scope selections from connection_config
                            let catalog_key =
                                catalog_config_key_for_type(&settings.datasource_type);
                            let selected_items: Vec<String> = cfg
                                .get(catalog_key)
                                .and_then(|v| v.as_array())
                                .map(|arr| {
                                    arr.iter()
                                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                        .collect()
                                })
                                .unwrap_or_default();
                            set_catalog_selected.try_set(selected_items);
                            let include_public = bigquery_include_public(cfg);
                            set_bq_include_public.try_set(include_public);

                            // Load indexing credentials from connection_config.
                            // The API masks secrets, so the value may be an object
                            // (unmasked) or the masked string "********".
                            if let Some(ic) = cfg.get("indexing_credentials") {
                                let is_masked = ic.as_str() == Some("********");
                                if ic.is_object() || is_masked {
                                    set_use_indexing_credentials.try_set(true);
                                    if is_masked {
                                        // Credentials exist but are masked — the user
                                        // hasn't changed them yet. Mark as unchanged so
                                        // save sends MASKED_VALUE to preserve the blob.
                                        set_indexing_creds_unchanged.try_set(true);
                                    }
                                    if let Some(ic_obj) = ic.as_object() {
                                        set_indexing_creds_type.try_set(
                                            ic_obj.get("type").and_then(|v| v.as_str()).unwrap_or("").to_string()
                                        );
                                        if let Some(sa_json) = ic_obj.get("service_account_json").and_then(|v| v.as_str()) {
                                            set_indexing_creds_json.try_set(sa_json.to_string());
                                        }
                                        if let Some(u) = ic_obj.get("username").and_then(|v| v.as_str()) {
                                            set_indexing_username.try_set(u.to_string());
                                        }
                                        if let Some(t) = ic_obj.get("access_token").and_then(|v| v.as_str()) {
                                            set_indexing_token.try_set(t.to_string());
                                        }
                                        if let Some(cid) = ic_obj.get("client_id").and_then(|v| v.as_str()) {
                                            set_indexing_client_id.try_set(cid.to_string());
                                        }
                                        if let Some(tid) = ic_obj.get("tenant_id").and_then(|v| v.as_str()) {
                                            set_indexing_tenant_id.try_set(tid.to_string());
                                        }
                                        // Sensitive fields — only load if the object is
                                        // unmasked (dev/test). In production the whole
                                        // blob is masked, so these won't be present.
                                        if let Some(p) = ic_obj.get("password").and_then(|v| v.as_str()) {
                                            set_indexing_password.try_set(p.to_string());
                                        }
                                        if let Some(cs) = ic_obj.get("client_secret").and_then(|v| v.as_str()) {
                                            set_indexing_client_secret.try_set(cs.to_string());
                                        }
                                    }
                                }
                            }

                            // Load user settings (masked credentials)
                            let user = &settings.user_settings;
                            let user_str = |key: &str| -> String {
                                user.get(key)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string()
                            };
                            set_cred_billing_project.try_set(user_str("billing_project"));
                            // BigQuery service_account mode stores this field
                            // in connection_config instead (workspace-level,
                            // not per-user) — see `build_connection_config`
                            // (KYO-405). Override the per-user load above when
                            // that's the active mode, so the field the admin
                            // configured is still shown on reopen.
                            if cfg.get("auth_mode").and_then(|v| v.as_str()) == Some("service_account")
                                && let Some(bp) = cfg.get("billing_project").and_then(|v| v.as_str())
                            {
                                set_cred_billing_project.try_set(bp.to_string());
                            }
                            // Restore the stored (non-sensitive) username so the user
                            // doesn't have to re-type it. Reuses `user_str` (empty
                            // string when absent, matching the reset-form default).
                            set_cred_username.try_set(user_str("username"));
                            // Note: passwords are not pre-filled (security). We do
                            // surface whether one is already stored so the UI can
                            // hint at it via the password field's placeholder.
                            set_cred_password_stored.try_set(settings.has_password);

                            // Synapse: load tenant_id and service principal creds
                            if settings.datasource_type == "synapse" {
                                set_cfg_tenant_id.try_set(str_val("tenant_id"));
                                // SP client_id lives in user_settings (per-user credential)
                                let sp_client_id = user
                                    .get("client_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                set_cred_sp_client_id.try_set(sp_client_id);
                                // Note: sp_client_secret is not pre-filled (security)
                            }

                            // Seed OAuth status from settings as initial state
                            // while the dedicated status fetch runs.
                            let is_oauth_connected = settings.has_oauth;
                            let oauth_email_seed = settings.oauth_email.clone();
                            let is_expired = settings.credential_status == "expired";
                            set_modal_oauth_connected.try_set(is_oauth_connected);
                            set_modal_oauth_email.try_set(oauth_email_seed);
                            set_modal_oauth_expired.try_set(is_expired && is_oauth_connected);

                            // Now fetch fresh OAuth status from the server.
                            // auth_mode and slug are already loaded above so
                            // we capture their seeded values directly.
                            let ds_type_for_fetch = settings.datasource_type.clone();
                            let slug_for_fetch = settings.slug.clone();
                            let auth_mode_for_fetch = settings.auth_mode.clone();

                            leptos::task::spawn_local(async move {
                                // Determine which status function to call.
                                let (new_connected, new_email, new_expired) =
                                    match ds_type_for_fetch.as_str() {
                                        "bigquery" => {
                                            let mode = auth_mode_for_fetch
                                                .as_deref()
                                                .unwrap_or("kyomi_oauth");
                                            if mode == "enterprise_oauth" {
                                                match get_datasource_oauth_status(
                                                    "bigquery-enterprise".to_string(),
                                                    slug_for_fetch,
                                                )
                                                .await
                                                {
                                                    Ok(s) => (
                                                        s.connected,
                                                        s.provider_email,
                                                        s.token_expired,
                                                    ),
                                                    Err(_) => return,
                                                }
                                            } else {
                                                match get_google_oauth_status().await {
                                                    Ok(s) => (
                                                        s.connected,
                                                        s.google_email,
                                                        s.token_expired,
                                                    ),
                                                    Err(_) => return,
                                                }
                                            }
                                        }
                                        "snowflake" => {
                                            match get_datasource_oauth_status(
                                                "snowflake".to_string(),
                                                slug_for_fetch,
                                            )
                                            .await
                                            {
                                                Ok(s) => (
                                                    s.connected,
                                                    s.provider_email,
                                                    s.token_expired,
                                                ),
                                                Err(_) => return,
                                            }
                                        }
                                        "databricks" => {
                                            match get_datasource_oauth_status(
                                                "databricks".to_string(),
                                                slug_for_fetch,
                                            )
                                            .await
                                            {
                                                Ok(s) => (
                                                    s.connected,
                                                    s.provider_email,
                                                    s.token_expired,
                                                ),
                                                Err(_) => return,
                                            }
                                        }
                                        "synapse" => {
                                            // Only enterprise_oauth mode has OAuth status
                                            let mode = auth_mode_for_fetch
                                                .as_deref()
                                                .unwrap_or("sql");
                                            if mode != "enterprise_oauth" {
                                                return;
                                            }
                                            match get_datasource_oauth_status(
                                                "microsoft-enterprise".to_string(),
                                                slug_for_fetch,
                                            )
                                            .await
                                            {
                                                Ok(s) => (
                                                    s.connected,
                                                    s.provider_email,
                                                    s.token_expired,
                                                ),
                                                Err(_) => return,
                                            }
                                        }
                                        // Not an OAuth datasource — skip fetch.
                                        _ => return,
                                    };

                                set_modal_oauth_connected.try_set(new_connected);
                                set_modal_oauth_email.try_set(new_email);
                                set_modal_oauth_expired.try_set(new_expired);
                            });
                        }
                        Err(e) => {
                            set_error_msg.try_set(Some(format!("Failed to load settings: {e}")));
                        }
                    }
                    set_settings_loading.try_set(false);
                });
            }
        }
    });

    // ── Sample availability check (create mode) ─────────────────────────
    // Matches the React `checkSample` effect in DatasourceModal.jsx — fetches
    // `/api/v1/datasources/sample/available` when the modal opens in create mode.
    Effect::new(move |_| {
        if !open.get() {
            return;
        }
        if datasource_id.get().is_some() {
            // Edit mode — sample tile is not shown.
            return;
        }
        leptos::task::spawn_local(async move {
            match check_sample_datasource_available().await {
                Ok(result) => {
                    set_sample_available.try_set(result.configured && result.is_admin);
                    set_sample_already_added.try_set(result.already_added);
                }
                Err(_) => {
                    set_sample_available.try_set(false);
                    set_sample_already_added.try_set(false);
                }
            }
        });
    });

    // ── Sample quick-add handler ─────────────────────────────────────────
    // Matches React `handleCreateSample` — creates the sample datasource and
    // closes the modal, then refreshes the datasource list via on_saved.
    let do_create_sample = move || {
        set_creating_sample.set(true);
        set_error_msg.set(None);
        leptos::task::spawn_local(async move {
            match create_sample_datasource().await {
                Ok(()) => {
                    // Signal success — DatasourcesContent refreshes the list.
                    on_saved.run(DatasourceResult {
                        id: String::new(),
                        slug: "acme-analytics-sample".to_string(),
                        name: "Acme Analytics (Sample)".to_string(),
                        datasource_type: "clickhouse".to_string(),
                    });
                }
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("already") {
                        set_sample_already_added.try_set(true);
                    }
                    set_error_msg.try_set(Some(msg));
                }
            }
            set_creating_sample.try_set(false);
        });
    };

    // ── Name change handler ──────────────────────────────────────────────
    let handle_name_change = move |new_name: String| {
        if !slug_manually_edited.get_untracked() {
            set_slug.set(generate_slug(&new_name));
        }
        set_name.set(new_name);
    };

    // ── Build connection_config JSON ─────────────────────────────────────
    let build_connection_config = move || -> serde_json::Value {
        let t = ds_type.get_untracked();
        let mut map = serde_json::Map::new();

        match t.as_str() {
            "postgres" | "mysql" | "redshift" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if let Ok(port) = cfg_port.get_untracked().parse::<i64>() {
                    map.insert("port".to_string(), serde_json::json!(port));
                }
                map.insert("ssl_mode".to_string(), serde_json::json!(cfg_ssl_mode.get_untracked()));
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
            }
            "clickhouse" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if let Ok(port) = cfg_port.get_untracked().parse::<i64>() {
                    map.insert("port".to_string(), serde_json::json!(port));
                }
                map.insert("secure".to_string(), serde_json::json!(cfg_secure.get_untracked()));
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
            }
            "snowflake" => {
                if !cfg_account.get_untracked().is_empty() {
                    map.insert("account".to_string(), serde_json::json!(cfg_account.get_untracked()));
                }
                map.insert("auth_mode".to_string(), serde_json::json!(sf_auth_mode.get_untracked()));
                if !cfg_warehouse.get_untracked().is_empty() {
                    map.insert("warehouse".to_string(), serde_json::json!(cfg_warehouse.get_untracked()));
                }
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
                if !cfg_role.get_untracked().is_empty() {
                    map.insert("role".to_string(), serde_json::json!(cfg_role.get_untracked()));
                }
                if !cfg_oauth_client_id.get_untracked().is_empty() {
                    map.insert("oauth_client_id".to_string(), serde_json::json!(cfg_oauth_client_id.get_untracked()));
                }
                if !cfg_oauth_client_secret.get_untracked().is_empty() {
                    map.insert("oauth_client_secret".to_string(), serde_json::json!(cfg_oauth_client_secret.get_untracked()));
                }
            }
            "databricks" => {
                map.insert("auth_mode".to_string(), serde_json::json!(db_auth_mode.get_untracked()));
                if !cfg_server_hostname.get_untracked().is_empty() {
                    map.insert("server_hostname".to_string(), serde_json::json!(cfg_server_hostname.get_untracked()));
                }
                if !cfg_http_path.get_untracked().is_empty() {
                    map.insert("http_path".to_string(), serde_json::json!(cfg_http_path.get_untracked()));
                }
                if !cfg_catalog.get_untracked().is_empty() {
                    map.insert("catalog".to_string(), serde_json::json!(cfg_catalog.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
                if db_auth_mode.get_untracked() == "oauth" {
                    if !cfg_oauth_client_id.get_untracked().is_empty() {
                        map.insert("oauth_client_id".to_string(), serde_json::json!(cfg_oauth_client_id.get_untracked()));
                    }
                    if !cfg_oauth_client_secret.get_untracked().is_empty() {
                        map.insert("oauth_client_secret".to_string(), serde_json::json!(cfg_oauth_client_secret.get_untracked()));
                    }
                }
            }
            "sqlserver" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert("host".to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if let Ok(port) = cfg_port.get_untracked().parse::<i64>() {
                    map.insert("port".to_string(), serde_json::json!(port));
                }
                map.insert("encrypt".to_string(), serde_json::json!(cfg_encrypt.get_untracked()));
                map.insert("trust_server_certificate".to_string(), serde_json::json!(cfg_trust_cert.get_untracked()));
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
            }
            "synapse" => {
                if !cfg_host.get_untracked().is_empty() {
                    map.insert(SYNAPSE_SERVER_CONFIG_KEY.to_string(), serde_json::json!(cfg_host.get_untracked()));
                }
                if !cfg_database.get_untracked().is_empty() {
                    map.insert("database".to_string(), serde_json::json!(cfg_database.get_untracked()));
                }
                if !cfg_schema.get_untracked().is_empty() {
                    map.insert("schema".to_string(), serde_json::json!(cfg_schema.get_untracked()));
                }
                // Synapse auth-mode fields
                let syn_mode = synapse_auth_mode.get_untracked();
                map.insert("auth_mode".to_string(), serde_json::json!(syn_mode.clone()));
                if !cfg_tenant_id.get_untracked().is_empty() {
                    map.insert("tenant_id".to_string(), serde_json::json!(cfg_tenant_id.get_untracked()));
                }
                // Enterprise OAuth admin credentials live in connection_config
                if syn_mode == "enterprise_oauth" {
                    if !cfg_oauth_client_id.get_untracked().is_empty() {
                        map.insert("oauth_client_id".to_string(), serde_json::json!(cfg_oauth_client_id.get_untracked()));
                    }
                    if !cfg_oauth_client_secret.get_untracked().is_empty() {
                        map.insert("oauth_client_secret".to_string(), serde_json::json!(cfg_oauth_client_secret.get_untracked()));
                    }
                }
            }
            "bigquery" => {
                let bq_mode = bq_auth_mode.get_untracked();
                map.insert("auth_mode".to_string(), serde_json::json!(bq_mode));
                if !cfg_oauth_client_id.get_untracked().is_empty() {
                    map.insert("oauth_client_id".to_string(), serde_json::json!(cfg_oauth_client_id.get_untracked()));
                }
                if !cfg_oauth_client_secret.get_untracked().is_empty() {
                    map.insert("oauth_client_secret".to_string(), serde_json::json!(cfg_oauth_client_secret.get_untracked()));
                }
                if !cfg_service_account_json.get_untracked().is_empty() {
                    map.insert("service_account_json".to_string(), serde_json::json!(cfg_service_account_json.get_untracked()));
                }
                // A service account is shared by the whole workspace, so its
                // billing project belongs in workspace-level
                // connection_config — not per-user credentials (the OAuth
                // modes' storage below in `build_credentials`). The driver
                // corroborates this: `resolve_billing_project`
                // (bigquery.rs:61-79) reads `connection_config["billing_project"]`
                // before ever looking at credentials (KYO-405).
                if bq_mode == "service_account"
                    && !cred_billing_project.get_untracked().is_empty()
                {
                    map.insert("billing_project".to_string(), serde_json::json!(cred_billing_project.get_untracked()));
                }
            }
            _ => {}
        }

        // SSH tunnel — admin-only, SSH-capable types only (KYO-137: the
        // whole block, including the disabled-state clear below, is gated on
        // `supports_ssh_tunnel` so non-SSH-capable types never see ssh_* keys
        // in their connection_config at all).
        //
        // `ssh_private_key` / `ssh_passphrase` writes depend on key-source
        // mode (KYO-134):
        // - "generate": `ssh_private_key` is only written when a key was
        //   freshly generated THIS session (`ssh_private_key_generated` is
        //   `Some`); on edit, with SSH already enabled and no new key
        //   generated, we must not overwrite the stored ciphertext with the
        //   masked placeholder the field loads back as. This mirrors the
        //   "don't overwrite masked secret" rule used for
        //   password/shared_password elsewhere in this modal, but is easier
        //   to enforce here since the private key is never loaded into a
        //   signal at all (see the edit-mode load-back effect above).
        // - "byok": `ssh_private_key` / `ssh_passphrase` are written only
        //   when the user typed something this session; a blank field on
        //   edit means "keep the existing key/passphrase" (same masked-secret
        //   discipline), so it's omitted rather than sent as empty. No
        //   `ssh_public_key` is written — BYOK users manage their own
        //   keypair, there is nothing for Kyomi to display.
        if supports_ssh_tunnel(&t) {
            if cfg_ssh_enabled.get_untracked() {
                map.insert("ssh_enabled".to_string(), serde_json::json!(true));
                let ssh_host = cfg_ssh_host.get_untracked();
                if !ssh_host.is_empty() {
                    map.insert("ssh_host".to_string(), serde_json::json!(ssh_host));
                }
                let ssh_username = cfg_ssh_username.get_untracked();
                if !ssh_username.is_empty() {
                    map.insert("ssh_username".to_string(), serde_json::json!(ssh_username));
                }
                let ssh_port = cfg_ssh_port.get_untracked().parse::<i64>().unwrap_or(22);
                map.insert("ssh_port".to_string(), serde_json::json!(ssh_port));

                // Host fingerprint (KYO-133) — plain string, not sensitive.
                let fingerprint = cfg_ssh_host_fingerprint.get_untracked();
                if !fingerprint.is_empty() {
                    map.insert("ssh_host_fingerprint".to_string(), serde_json::json!(fingerprint));
                }

                match cfg_ssh_key_mode.get_untracked().as_str() {
                    "byok" => {
                        let private_key = cfg_ssh_private_key_input.get_untracked();
                        if !private_key.is_empty() {
                            map.insert("ssh_private_key".to_string(), serde_json::json!(private_key));
                        }
                        let passphrase = cfg_ssh_passphrase.get_untracked();
                        if !passphrase.is_empty() {
                            map.insert("ssh_passphrase".to_string(), serde_json::json!(passphrase));
                        }
                    }
                    _ => {
                        // "generate" (default)
                        if let Some(public_key) = ssh_public_key.get_untracked() {
                            map.insert("ssh_public_key".to_string(), serde_json::json!(public_key));
                        }
                        if let Some(private_key) = ssh_private_key_generated.get_untracked() {
                            map.insert("ssh_private_key".to_string(), serde_json::json!(private_key));
                        }
                    }
                }
            } else {
                map.insert("ssh_enabled".to_string(), serde_json::json!(false));
                // Explicit clear: disabling the tunnel must drop the stored
                // ciphertext, not just flip the flag. `preserve_masked_connection_config`
                // treats an *absent* sensitive field as "not resupplied, restore
                // the existing value" (the normal edit case) — so silently
                // omitting `ssh_private_key` / `ssh_passphrase` here would leave
                // the old encrypted secrets orphaned in `connection_config`
                // forever. An explicit JSON `null` is the signal that means
                // "clear it," not "didn't touch it."
                map.insert("ssh_private_key".to_string(), serde_json::Value::Null);
                map.insert("ssh_passphrase".to_string(), serde_json::Value::Null);
            }
        }

        if cfg_shared_credentials.get_untracked() {
            map.insert("shared_credentials".to_string(), serde_json::json!(true));
        }

        // Catalog scope — edit mode vs create mode are kept separate so the two
        // sets of signals never conflict.
        let in_create_mode = datasource_id.get_untracked().is_none();
        if in_create_mode {
            // Create-mode catalog scope: prefer checkbox selections, fall back to
            // the comma-separated text input.
            let key = catalog_config_key_for_type(&t);
            let selected = create_catalog_selected.get_untracked();
            if !selected.is_empty() {
                map.insert(key.to_string(), serde_json::json!(selected));
            } else {
                let text = create_catalog_text.get_untracked();
                let items: Vec<String> = text
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !items.is_empty() {
                    map.insert(key.to_string(), serde_json::json!(items));
                }
            }
            if t == "bigquery" {
                map.insert(
                    "include_public_datasets".to_string(),
                    serde_json::json!(create_include_public_datasets.get_untracked()),
                );
            }
        } else {
            // Edit-mode catalog scope: write when the user has made a selection
            // or explicitly cleared the scope. An empty Vec means "index nothing";
            // an absent key means "index all" (the default). Only write when
            // the user touched the picker, to preserve the default for
            // datasources that were never configured for scoped indexing.
            let selected = catalog_selected.get_untracked();
            if !selected.is_empty() || catalog_scope_touched.get_untracked() {
                let key = catalog_config_key_for_type(&t);
                map.insert(key.to_string(), serde_json::json!(selected));
            }
            if t == "bigquery" {
                map.insert(
                    "include_public_datasets".to_string(),
                    serde_json::json!(bq_include_public.get_untracked()),
                );
            }
        }

        // Indexing credentials
        if use_indexing_credentials.get_untracked() {
            if indexing_creds_unchanged.get_untracked() {
                // Credentials were loaded masked and the user didn't change
                // them. Send MASKED_VALUE so finalize_connection_config_secrets
                // restores the existing encrypted blob.
                map.insert(
                    "indexing_credentials".to_string(),
                    serde_json::Value::String("********".to_string()),
                );
            } else {
                let ic_type = indexing_creds_type.get_untracked();
                if !ic_type.is_empty() {
                    let mut ic_map = serde_json::Map::new();
                    ic_map.insert("type".to_string(), serde_json::json!(ic_type));
                    match ic_type.as_str() {
                        "service_account" => {
                            let sa_json = indexing_creds_json.get_untracked();
                            if !sa_json.is_empty() {
                                ic_map.insert("service_account_json".to_string(), serde_json::json!(sa_json));
                            }
                        }
                        "password" | "sql" => {
                            let u = indexing_username.get_untracked();
                            let p = indexing_password.get_untracked();
                            if !u.is_empty() && !p.is_empty() {
                                ic_map.insert("username".to_string(), serde_json::json!(u));
                                ic_map.insert("password".to_string(), serde_json::json!(p));
                            }
                        }
                        "token" => {
                            let t = indexing_token.get_untracked();
                            if !t.is_empty() {
                                ic_map.insert("access_token".to_string(), serde_json::json!(t));
                            }
                        }
                        "service_principal" => {
                            let cid = indexing_client_id.get_untracked();
                            let cs = indexing_client_secret.get_untracked();
                            let tid = indexing_tenant_id.get_untracked();
                            if !cid.is_empty() && !cs.is_empty() && !tid.is_empty() {
                                ic_map.insert("client_id".to_string(), serde_json::json!(cid));
                                ic_map.insert("client_secret".to_string(), serde_json::json!(cs));
                                ic_map.insert("tenant_id".to_string(), serde_json::json!(tid));
                            }
                        }
                        _ => {}
                    }
                    if ic_map.len() > 1 {
                        map.insert(
                            "indexing_credentials".to_string(),
                            serde_json::Value::Object(ic_map),
                        );
                    }
                }
            }
        } else if datasource_id.get_untracked().is_some() {
            // Edit mode with toggle off: emit explicit null to clear the stored
            // indexing_credentials. An absent field would be restored by
            // finalize_connection_config_secrets (same pattern as SSH keys).
            map.insert(
                "indexing_credentials".to_string(),
                serde_json::Value::Null,
            );
        }

        serde_json::Value::Object(map)
    };

    // ── Build credentials JSON ────────────────────────────────────────────
    let build_credentials = move || -> serde_json::Value {
        let t = ds_type.get_untracked();
        let mut map = serde_json::Map::new();

        match t.as_str() {
            "databricks" => {
                if db_auth_mode.get_untracked() == "oauth" {
                    map.insert("auth_type".to_string(), serde_json::json!("oauth"));
                } else if !cred_access_token.get_untracked().is_empty() {
                    map.insert("access_token".to_string(), serde_json::json!(cred_access_token.get_untracked()));
                }
            }
            "bigquery" => {
                let bq_mode = bq_auth_mode.get_untracked();
                match bq_mode.as_str() {
                    "kyomi_oauth" | "enterprise_oauth"
                        if !cred_billing_project.get_untracked().is_empty() =>
                    {
                        map.insert("billing_project".to_string(), serde_json::json!(cred_billing_project.get_untracked()));
                    }
                    // service_account: this is shared workspace config, not
                    // a personal credential — written into connection_config
                    // by `build_connection_config` above instead (KYO-405).
                    _ => {}
                }
            }
            "snowflake" => {
                let sf_mode = sf_auth_mode.get_untracked();
                match sf_mode.as_str() {
                    "oauth" => {
                        map.insert("auth_type".to_string(), serde_json::json!("oauth"));
                    }
                    _ => {
                        // password or keypair — username + password/private_key
                        if !cred_username.get_untracked().is_empty() {
                            map.insert("username".to_string(), serde_json::json!(cred_username.get_untracked()));
                        }
                        if !cred_password.get_untracked().is_empty() {
                            map.insert("password".to_string(), serde_json::json!(cred_password.get_untracked()));
                        }
                        if !cred_private_key.get_untracked().is_empty() {
                            map.insert("private_key".to_string(), serde_json::json!(cred_private_key.get_untracked()));
                        }
                    }
                }
            }
            "synapse" => {
                let syn_mode = synapse_auth_mode.get_untracked();
                match syn_mode.as_str() {
                    "service_principal" => {
                        // The Synapse driver (kyomi-connect
                        // providers/synapse.rs) reads tenant_id, client_id
                        // and client_secret all three from `credentials`
                        // for this mode. client_id/client_secret already
                        // landed here; tenant_id was the odd one out — it
                        // was written only to connection_config below,
                        // which the driver never consults for
                        // service_principal — so every Service Principal
                        // connection failed with "Service Principal
                        // requires tenant_id" even with the field filled
                        // in on screen (KYO-522). The connection_config
                        // copy stays: enterprise_oauth mode still needs it
                        // there. cfg_tenant_id is the same signal
                        // build_connection_config reads below, so edit
                        // mode's load-back (which populates cfg_tenant_id
                        // from connection_config) keeps both copies in
                        // sync on re-save.
                        if !cfg_tenant_id.get_untracked().is_empty() {
                            map.insert("tenant_id".to_string(), serde_json::json!(cfg_tenant_id.get_untracked()));
                        }
                        if !cred_sp_client_id.get_untracked().is_empty() {
                            map.insert("client_id".to_string(), serde_json::json!(cred_sp_client_id.get_untracked()));
                        }
                        if !cred_sp_client_secret.get_untracked().is_empty() {
                            map.insert("client_secret".to_string(), serde_json::json!(cred_sp_client_secret.get_untracked()));
                        }
                    }
                    "enterprise_oauth" => {
                        map.insert("auth_type".to_string(), serde_json::json!("oauth"));
                    }
                    _ => {
                        // SQL authentication — username + password
                        if !cred_username.get_untracked().is_empty() {
                            map.insert("username".to_string(), serde_json::json!(cred_username.get_untracked()));
                        }
                        if !cred_password.get_untracked().is_empty() {
                            map.insert("password".to_string(), serde_json::json!(cred_password.get_untracked()));
                        }
                    }
                }
            }
            _ => {
                if !cred_username.get_untracked().is_empty() {
                    map.insert("username".to_string(), serde_json::json!(cred_username.get_untracked()));
                }
                if !cred_password.get_untracked().is_empty() {
                    map.insert("password".to_string(), serde_json::json!(cred_password.get_untracked()));
                }
                if !cred_private_key.get_untracked().is_empty() {
                    map.insert("private_key".to_string(), serde_json::json!(cred_private_key.get_untracked()));
                }
            }
        }

        serde_json::Value::Object(map)
    };

    // ── Test & Discover ──────────────────────────────────────────────────
    // Input: (ds_type, conn_cfg, creds, ds_id, slug_val)
    type TestDiscoverInput = (String, serde_json::Value, serde_json::Value, Option<String>, String);

    let test_action = Action::new(|input: &TestDiscoverInput| {
        let (ds_type_val, conn_cfg, creds, ds_id, slug_val) = input.clone();
        async move {
            // In edit mode, use the existing datasource ID for OAuth credential lookup.
            let ds_slug = ds_id
                .as_deref()
                .map(|_| slug_val)
                .filter(|s| !s.is_empty());
            discover_datasource_resources(ds_type_val, conn_cfg, creds, ds_slug).await
        }
    });

    Effect::new(move |_| {
        if let Some(result) = test_action.value().get() {
            match result {
                Ok(r) => {
                    if r.success {
                        set_test_result.set(Some(TestConnectionResult {
                            success: true,
                            message: "Connected successfully".to_string(),
                        }));
                        set_discovery_status.set("success".to_string());

                        if let Some(dbs) = r.resources.get("databases") {
                            set_discovered_databases.set(dbs.clone());
                        }
                        if let Some(schemas) = r.resources.get("schemas") {
                            set_discovered_schemas.set(schemas.clone());
                        }
                        if let Some(wh) = r.resources.get("warehouses") {
                            set_discovered_warehouses.set(wh.clone());
                        }
                        if let Some(cats) = r.resources.get("catalogs") {
                            set_discovered_catalogs.set(cats.clone());
                        }
                        // BigQuery service_account mode (KYO-405) — the
                        // server's `list_projects()` arm returns bare project
                        // ids (no display name, unlike the richer
                        // `get_google_oauth_projects()` used by kyomi_oauth
                        // mode below), so id and label are the same string.
                        // Absent from `resources` (not an empty vec) when the
                        // service account lacks resourcemanager.projects.list
                        // — see `build_resources_map` server-side — in which
                        // case the reason is carried in `resource_errors`
                        // instead of vanishing (KYO-466), and surfaced here
                        // through `bq_projects_error` — the same signal
                        // `get_google_oauth_projects()`'s failure arm below
                        // already populates for the kyomi_oauth path.
                        // `BqProjectField` falls back to a free-text input
                        // either way.
                        if let Some(projects) = r.resources.get("projects") {
                            let opts: Vec<(String, String)> =
                                projects.iter().map(|p| (p.clone(), p.clone())).collect();
                            set_bq_projects.set(opts);
                            set_bq_projects_error.set(None);
                            set_bq_projects_attempted.set(true);
                        } else if let Some(reason) = r.resource_errors.get("projects") {
                            set_bq_projects_error.set(Some(format!("Couldn't list projects: {reason}")));
                            set_bq_projects_attempted.set(true);
                        }

                        // KYO-474: same `resource_errors` read as the block
                        // above, scoped to whichever key
                        // `CreateModeCatalogPicker` actually renders for the
                        // current type. `catalog_denial_key_for_type` (not
                        // `discovery_resource_key_for_type`, which silently
                        // reads a key BigQuery never populates — KYO-544) is
                        // the exact function `EditModeCatalogTab`'s
                        // `discover_action` Effect also uses, so the two
                        // components can never disagree about which key
                        // means what.
                        let ds_type_val = ds_type.get_untracked();
                        set_catalog_discovery_denied.set(
                            r.resource_errors
                                .contains_key(catalog_denial_key_for_type(&ds_type_val)),
                        );
                    } else {
                        set_test_result.set(Some(TestConnectionResult {
                            success: false,
                            message: r.message,
                        }));
                        set_discovery_status.set("error".to_string());
                        set_catalog_discovery_denied.set(false);
                    }
                }
                Err(e) => {
                    set_test_result.set(Some(TestConnectionResult {
                        success: false,
                        message: e.to_string(),
                    }));
                    set_discovery_status.set("error".to_string());
                    set_catalog_discovery_denied.set(false);
                }
            }
        }
    });

    let do_test_and_discover = move || {
        set_test_result.set(None);
        set_discovery_status.set("loading".to_string());
        set_discovered_databases.set(vec![]);
        set_discovered_schemas.set(vec![]);
        set_discovered_warehouses.set(vec![]);
        set_discovered_catalogs.set(vec![]);
        set_catalog_discovery_denied.set(false);
        // BigQuery service_account mode (KYO-405) — clear any stale project
        // list (and any error/attempted state from a previous validate)
        // before dispatching a fresh one. No-op for every other
        // provider/mode, which never populate these. KYO-468: also resets
        // `bq_projects_error` — a fresh validate means no attempt has
        // completed yet for *this* run, so a stale prior error must not
        // outlive the dispatch that's about to replace it either.
        reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);

        let ds_type_val = ds_type.get_untracked();
        let conn_cfg = build_connection_config();
        let creds = build_credentials();
        let ds_id = datasource_id.get_untracked();
        let slug_val = slug.get_untracked();

        test_action.dispatch((ds_type_val, conn_cfg, creds, ds_id, slug_val));
    };

    // BigQuery service_account mode's "Validate & Discover Projects" button
    // (KYO-405) — a Callback wrapper so `BigQueryAuthModeSection` can trigger
    // the same `do_test_and_discover`/`test_action` every other provider's
    // Test & Discover button uses, without needing the parent's private
    // `TestDiscoverInput` type in its own signature.
    let bq_test_pending = Signal::derive(move || test_action.pending().get());
    let on_bq_validate = Callback::new(move |()| {
        if !test_action.pending().get_untracked() {
            do_test_and_discover();
        }
    });

    // ── OAuth disconnect actions ─────────────────────────────────────────
    // Two separate Actions — one for Google OAuth (BigQuery kyomi_oauth mode),
    // one for per-datasource OAuth (BigQuery enterprise, Snowflake, Databricks).
    // Per CODING_STANDARDS: user-triggered async mutations → Action.

    // Input: unused `()` because there are no dispatch-time parameters.
    let google_disconnect_action = Action::new(move |_: &()| async move {
        disconnect_google_oauth().await
    });

    Effect::new(move |_| {
        if let Some(result) = google_disconnect_action.value().get() {
            match result {
                Ok(_) => {
                    set_modal_oauth_connected.set(false);
                    set_modal_oauth_email.set(None);
                    set_modal_oauth_expired.set(false);
                    // Clear project list (and error/attempted, KYO-468 — see
                    // `reset_bq_projects_signals`) so dropdowns revert to text
                    // inputs and no stale failure message survives the
                    // disconnect.
                    reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);
                    // KYO-413 — the credentials that produced `test_result` no
                    // longer exist once the Google account is disconnected;
                    // leaving it `Some(success: true)` would keep "Next"
                    // enabled for a gate that no longer has anything backing
                    // it. Mirrors the reset `do_test_and_discover` performs
                    // before every fresh validate, `datasource_disconnect_action`'s
                    // Effect below, and each provider's Authentication Mode
                    // selector `on_change` (which resets it directly, not via
                    // an Effect — there is no auth-mode-change Effect in this
                    // component).
                    set_test_result.set(None);
                    set_discovery_status.set("idle".to_string());
                    #[cfg(target_arch = "wasm32")]
                    toast_success("Google account disconnected");
                }
                Err(e) => {
                    toast_error(format!("Failed to disconnect: {e}"));
                }
            }
        }
    });

    // Input: (provider, datasource_slug).
    let datasource_disconnect_action =
        Action::new(move |(provider, slug_val): &(String, String)| {
            let provider = provider.clone();
            let slug_val = slug_val.clone();
            async move { disconnect_datasource_oauth(provider, slug_val).await }
        });

    Effect::new(move |_| {
        if let Some(result) = datasource_disconnect_action.value().get() {
            match result {
                Ok(_) => {
                    set_modal_oauth_connected.set(false);
                    set_modal_oauth_email.set(None);
                    set_modal_oauth_expired.set(false);
                    // Clear project list (and error/attempted, KYO-468 — see
                    // `reset_bq_projects_signals`) so dropdowns revert to text
                    // inputs and no stale failure message survives the
                    // disconnect.
                    reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);
                    // KYO-413 — shared by BigQuery enterprise_oauth, Snowflake,
                    // Databricks, and Synapse: whichever provider just
                    // disconnected, the credentials backing `test_result` are
                    // gone, so the "Next" gate must re-close. See the mirror
                    // fix on `google_disconnect_action` above.
                    set_test_result.set(None);
                    set_discovery_status.set("idle".to_string());
                    #[cfg(target_arch = "wasm32")]
                    toast_success("Account disconnected");
                }
                Err(e) => {
                    toast_error(format!("Failed to disconnect: {e}"));
                }
            }
        }
    });

    // ── SSH tunnel keypair generation ────────────────────────────────────
    // Per CODING_STANDARDS: user-triggered async mutations → Action.
    // Input: unused `()` — no dispatch-time parameters, mirrors
    // `google_disconnect_action` above.
    let ssh_key_action = Action::new(move |_: &()| async move { generate_ssh_key().await });

    Effect::new(move |_| {
        if let Some(result) = ssh_key_action.value().get() {
            match result {
                Ok(GeneratedSshKey { public_key, private_key }) => {
                    set_ssh_public_key.set(Some(public_key));
                    set_ssh_private_key_generated.set(Some(private_key));
                }
                Err(e) => {
                    toast_error(format!("Failed to generate SSH key: {e}"));
                }
            }
        }
    });

    // `ssh_key_generating` mirrors `ssh_key_action.pending()` as a plain
    // signal so it can be reset by `reset_form` and read like any other
    // form-state signal by `SshTunnelSection`.
    Effect::new(move |_| {
        set_ssh_key_generating.set(ssh_key_action.pending().get());
    });

    // Auto-generate a keypair the first time SSH tunneling is enabled and no
    // key exists yet — "generate" key-source mode only (KYO-134). BYOK mode
    // also has `ssh_public_key.get().is_none()` (BYOK never populates a
    // public key), so without the mode check this would fire an unwanted
    // auto-generation every time a BYOK user enables the tunnel.
    // Guarded on `!settings_loading.get()` so this cannot race the edit-mode
    // load-back effect above: `cfg_ssh_enabled` and `ssh_public_key` are both
    // set synchronously (in the same async task, before `settings_loading`
    // flips back to `false`), so waiting for `settings_loading` to clear
    // guarantees we observe their final loaded values before deciding
    // whether a key is missing. Without this guard, an intermediate tick
    // where `cfg_ssh_enabled` has loaded `true` but `ssh_public_key` hasn't
    // been set yet would trigger a spurious generation that discards the
    // datasource's real stored key.
    Effect::new(move |_| {
        if cfg_ssh_enabled.get()
            && cfg_ssh_key_mode.get() == "generate"
            && ssh_public_key.get().is_none()
            && !settings_loading.get()
            && !ssh_key_action.pending().get_untracked()
        {
            ssh_key_action.dispatch(());
        }
    });

    // ── Save ─────────────────────────────────────────────────────────────
    // Input: (ds_id, name, slug, conn_cfg, creds, ds_type, is_admin)
    //
    // `is_admin` is threaded through the Action input (resolved at dispatch
    // time in `do_save` via `get_untracked()`, per CODING_STANDARDS.md
    // "resolve derived signal values at click time") rather than read live
    // inside the async block, so a mid-save admin-status change can't alter
    // which branch runs.
    type SaveInput = (Option<String>, String, String, serde_json::Value, serde_json::Value, String, bool);

    let save_action = Action::new(|input: &SaveInput| {
        let (ds_id, name_val, slug_val, conn_cfg, creds, ds_type_val, is_admin_val) = input.clone();
        async move {
            match ds_id {
                None => {
                    // Create mode — the header/empty-state "Add Datasource" CTAs
                    // are admin-gated (KYO-184), so this path is only reachable
                    // by admins in the UI; `create_datasource_modal` enforces it
                    // server-side regardless.
                    create_datasource_modal(name_val, slug_val, ds_type_val, conn_cfg, creds).await
                }
                Some(id) if is_admin_val => {
                    // Admin edit — save connection settings first, then credentials.
                    let update_result = update_datasource_settings(id.clone(), name_val, slug_val, conn_cfg).await;
                    match update_result {
                        Ok(r) => {
                            // Save credentials if any were entered. Connection
                            // settings have already persisted at this point, so a
                            // credential-save failure here is a *partial*
                            // success, not a failure of the whole save — return
                            // Ok(r) either way (a blanket `?` would incorrectly
                            // tell the caller settings were never saved), but
                            // surface the failure via toast rather than
                            // discarding it. A toast (not `error_msg`) is
                            // required here specifically because `on_saved`
                            // closes the modal on Ok, so an `error_msg` Alert
                            // — which only renders inside the modal body —
                            // would never be seen.
                            let creds_obj = creds.as_object().map(|o| !o.is_empty()).unwrap_or(false);
                            if creds_obj
                                && let Err(e) = save_datasource_credentials(id, creds).await
                            {
                                toast_error(format!(
                                    "Connection settings saved, but credentials failed to save: {e}"
                                ));
                            }
                            Ok(r)
                        }
                        Err(e) => Err(e),
                    }
                }
                Some(id) => {
                    // Non-admin edit — `update_datasource_settings` is
                    // workspace-admin-gated (KYO-184); a non-admin can only save
                    // their own credentials. Skip the call entirely when no
                    // credentials were entered — same `has_creds` guard the
                    // create-mode branch uses above — so merely opening the
                    // modal and clicking "Save Credentials" can't insert an
                    // empty credential row.
                    let has_creds = creds.as_object().map(|o| !o.is_empty()).unwrap_or(false);
                    if has_creds {
                        save_datasource_credentials(id.clone(), creds).await?;
                    }
                    Ok(DatasourceResult {
                        id,
                        slug: slug_val,
                        name: name_val,
                        datasource_type: ds_type_val,
                    })
                }
            }
        }
    });

    Effect::new(move |_| {
        if let Some(result) = save_action.value().get() {
            match result {
                Ok(saved) => {
                    on_saved.run(saved);
                }
                Err(e) => {
                    set_error_msg.set(Some(e.to_string()));
                }
            }
        }
    });

    let do_save = move || {
        let ds_id = datasource_id.get_untracked();
        let name_val = name.get_untracked();
        let slug_val = slug.get_untracked();
        let conn_cfg = build_connection_config();
        let creds = build_credentials();
        let ds_type_val = ds_type.get_untracked();

        if name_val.is_empty() {
            set_error_msg.set(Some("Name is required".to_string()));
            return;
        }

        // Validate indexing credentials completeness when enabled
        if use_indexing_credentials.get_untracked() && !indexing_creds_unchanged.get_untracked() {
            let ic_type = indexing_creds_type.get_untracked();
            let incomplete = match ic_type.as_str() {
                "service_account" => indexing_creds_json.get_untracked().is_empty(),
                "password" | "sql" => {
                    indexing_username.get_untracked().is_empty()
                        || indexing_password.get_untracked().is_empty()
                }
                "token" => indexing_token.get_untracked().is_empty(),
                "service_principal" => {
                    indexing_client_id.get_untracked().is_empty()
                        || indexing_client_secret.get_untracked().is_empty()
                        || indexing_tenant_id.get_untracked().is_empty()
                }
                _ => false,
            };
            if incomplete {
                set_error_msg.set(Some(
                    "Indexing credentials are incomplete. Fill in all required fields or disable dedicated indexing credentials.".to_string()
                ));
                return;
            }
        }

        set_error_msg.set(None);
        save_action.dispatch((ds_id, name_val, slug_val, conn_cfg, creds, ds_type_val, is_admin.get_untracked()));
    };

    // ── Create-mode: create a Connect datasource ─────────────────────────
    // Called from the create-mode footer when `connection_type == "connect"`.
    // On success, stashes the returned token + datasource name/type so the
    // modal body swaps to the post-create deployment view. The datasource
    // list refresh happens later, when the user clicks "Done" (which calls
    // on_saved, same as the direct create path's success callback).
    let do_create_connect = move || {
        let name_val = name.get_untracked().trim().to_string();
        let slug_val = slug.get_untracked();
        let ds_type_val = ds_type.get_untracked();

        if name_val.is_empty() {
            set_error_msg.set(Some("Name is required".to_string()));
            return;
        }

        set_creating_connect.set(true);
        set_error_msg.set(None);

        let slug_opt = if slug_val.trim().is_empty() {
            None
        } else {
            Some(slug_val)
        };

        leptos::task::spawn_local(async move {
            match create_connect_datasource(name_val.clone(), slug_opt, ds_type_val.clone()).await {
                Ok(result) => {
                    set_connect_created_name.try_set(name_val);
                    set_connect_created_type.try_set(ds_type_val);
                    set_connect_token.try_set(Some(result.connect_token));
                }
                Err(e) => {
                    set_error_msg.try_set(Some(e.to_string()));
                }
            }
            set_creating_connect.try_set(false);
        });
    };

    // ── Derived: is create mode ──────────────────────────────────────────
    let is_create_mode = Signal::derive(move || datasource_id.get().is_none());

    // ── Derived: is this a Kyomi Connect datasource? ─────────────────────
    // True in edit mode when the loaded datasource has `connection_type ==
    // "connect"`, and true in create mode when the user has picked "Kyomi
    // Connect" on the top-of-modal toggle.
    let is_connect = Signal::derive(move || connection_type.get() == "connect");

    // ── Derived: have we finished creating a Connect datasource? ─────────
    // When true, the modal body swaps to the post-create view (token +
    // deployment tabs + Done button). Cleared on modal close via reset_form.
    let connect_create_complete = Signal::derive(move || connect_token.get().is_some());

    // ── Modal title ──────────────────────────────────────────────────────
    let modal_title = Signal::derive(move || {
        if is_create_mode.get() {
            "Add Datasource".to_string()
        } else {
            format!("{} Settings", name.get())
        }
    });

    // ── Modal-level OAuth postMessage listener ───────────────────────────
    // Installed when the modal is mounted so the modal's own OAuth state
    // (connected/email) is updated when a popup closes.  The list-level
    // listener (in DatasourcesContent) handles the datasource-list refresh;
    // this listener handles the modal's internal status display.
    //
    // Also auto-triggers Test & Discover for Snowflake / Databricks after a
    // successful connection so the warehouse/catalog dropdowns populate.
    #[cfg(target_arch = "wasm32")]
    {
        use crate::utils::oauth_popup::{
            install_oauth_listener, translate_google_oauth_error, OAuthMessage,
        };
        let cleanup = install_oauth_listener(move |msg| {
            match msg {
                OAuthMessage::GoogleSuccess { email }
                | OAuthMessage::BigqueryEnterpriseSuccess { email } => {
                    set_modal_oauth_connected.try_set(true);
                    set_modal_oauth_email.try_set(email);
                    set_modal_oauth_expired.try_set(false);
                    set_modal_oauth_connecting.try_set(false);
                    // BigQuery OAuth never runs Test & Discover (the generic
                    // button is hidden for this type — the remote system is
                    // verified by the OAuth handshake itself, not a query).
                    // Without this, `test_result` would stay `None` forever
                    // and the create-mode "Next" gate below would never
                    // unlock, since that gate is the only other writer.
                    set_test_result.try_set(Some(TestConnectionResult {
                        success: true,
                        message: "Connected to Google".to_string(),
                    }));
                    set_discovery_status.try_set("success".to_string());
                }
                OAuthMessage::SnowflakeSuccess { email }
                | OAuthMessage::DatabricksSuccess { email } => {
                    set_modal_oauth_connected.try_set(true);
                    set_modal_oauth_email.try_set(email);
                    set_modal_oauth_expired.try_set(false);
                    set_modal_oauth_connecting.try_set(false);
                    // Auto-run Test & Discover so warehouse/catalog dropdowns
                    // populate immediately after OAuth completes.
                    do_test_and_discover();
                }
                OAuthMessage::GoogleError { error } => {
                    set_modal_oauth_connecting.try_set(false);
                    toast_error(translate_google_oauth_error(error));
                }
                OAuthMessage::SnowflakeError { error }
                | OAuthMessage::DatabricksError { error }
                | OAuthMessage::MicrosoftError { error }
                | OAuthMessage::MicrosoftEnterpriseError { error }
                | OAuthMessage::BigqueryEnterpriseError { error } => {
                    set_modal_oauth_connecting.try_set(false);
                    toast_error(error);
                }
                OAuthMessage::MicrosoftSuccess { .. } => {
                    set_modal_oauth_connecting.try_set(false);
                }
                OAuthMessage::MicrosoftEnterpriseSuccess { email } => {
                    // Microsoft Enterprise OAuth success — used by Synapse enterprise_oauth mode
                    set_modal_oauth_connected.try_set(true);
                    set_modal_oauth_email.try_set(email);
                    set_modal_oauth_expired.try_set(false);
                    set_modal_oauth_connecting.try_set(false);
                    // Auto-run Test & Discover so schema dropdowns populate
                    do_test_and_discover();
                }
            }
        });
        let cleanup_cell =
            std::cell::Cell::new(Some(Box::new(cleanup) as Box<dyn FnOnce()>));
        let cleanup_wrapper = send_wrapper::SendWrapper::new(cleanup_cell);
        on_cleanup(move || {
            if let Some(f) = cleanup_wrapper.take().take() {
                f();
            }
        });
    }

    // ── Fetch BigQuery project list when OAuth connects ───────────────────
    // Fires only in kyomi_oauth mode — that is the only mode where a personal
    // Google OAuth token is present. Enterprise OAuth uses per-datasource
    // organizational tokens that cannot list the user's personal GCP projects;
    // enterprise admins enter project IDs manually via the text input fallback.
    Effect::new(move |_| {
        let connected = modal_oauth_connected.get();
        let mode = bq_auth_mode.get();
        if connected && mode == "kyomi_oauth" {
            set_bq_projects_loading.set(true);
            set_bq_projects_error.set(None);
            // KYO-468: mark the attempt as made as soon as the fetch starts,
            // not only on success — CreateModeCatalogPicker uses this (never
            // set for enterprise_oauth, which skips this whole branch) to
            // tell "still loading" / "attempted, failed" apart from "never
            // attempted".
            set_bq_projects_attempted.set(true);
            leptos::task::spawn_local(async move {
                match get_google_oauth_projects().await {
                    Ok(result) => {
                        let options: Vec<(String, String)> = result
                            .projects
                            .into_iter()
                            .map(|p| {
                                let label = if p.name.is_empty() || p.name == p.project_id {
                                    p.project_id.clone()
                                } else {
                                    format!("{} ({})", p.name, p.project_id)
                                };
                                (p.project_id, label)
                            })
                            .collect();
                        set_bq_projects.try_set(options);
                        if let Some(msg) = result.message.filter(|m| !m.is_empty()) {
                            set_bq_projects_error.try_set(Some(msg));
                        }
                    }
                    Err(e) => {
                        set_bq_projects_error.try_set(Some(
                            format!("Failed to fetch BigQuery projects: {e}"),
                        ));
                    }
                }
                set_bq_projects_loading.try_set(false);
            });
        }
    });

    // ── Discovery section: show post-test fields ──────────────────────────
    let discovery_succeeded = Signal::derive(move || {
        discovery_status.get() == "success"
    });

    // ── Connection-step-satisfied predicate (KYO-404, extended KYO-411,
    // generalized KYO-517) ──────────────────────────────────────────────
    // BigQuery enterprise_oauth, Snowflake oauth, Databricks oauth, and
    // Synapse enterprise_oauth each have a slug-scoped connect endpoint
    // that can't be reached before the datasource exists
    // (`*_url` needs `datasource_slug`), so `test_result` can never
    // become `Some` for any of those four pairs in create mode — the
    // OAuth connect button for each stays gated behind "save first" (see
    // the `!is_create_mode` Show around each pair's
    // `ModalOAuthStatusPanel`). BigQuery kyomi_oauth is the one
    // account-level exception, satisfied by `modal_oauth_connected` alone
    // (KYO-411): that signal is written both by the popup's
    // `GoogleSuccess` postMessage arm (which also sets `test_result`
    // itself) and by `use_oauth_status_refetch`'s account-level status
    // fetch on modal open — an already-linked user has nothing to click
    // that would ever produce a `test_result`, so without this arm Next
    // stays permanently disabled for them (see
    // `connection_step_satisfied_from`'s doc comment for the full KYO-404
    // deadlock this reintroduces if omitted). Every other type/mode
    // requires an actual successful test. This is the single source of
    // truth for "is the Connection tab done" — read by the create-mode
    // footer's `can_next` and by all three states of the Catalog tab pill
    // (class, disabled, on:click). A cheap, pure projection over
    // same-scope signals with no side effect, so `Signal::derive` applies
    // (docs/CODING_STANDARDS.md: reserve `Signal::derive` for cheap, pure
    // projections; use `Memo` only when the body does more than read).
    //
    // `connection_step_satisfied_from` needs the auth mode of whichever
    // provider is currently selected, not `bq_auth_mode` specifically —
    // select the matching signal here, following the same
    // `match ds_type.get().as_str() { "bigquery" => ..., "snowflake" =>
    // ..., ... }` idiom `build_credentials` above already uses to pick a
    // provider-specific signal by `ds_type`.
    let connection_step_satisfied: Signal<bool> = Signal::derive(move || {
        let t = ds_type.get();
        let auth_mode = match t.as_str() {
            "bigquery" => bq_auth_mode.get(),
            "snowflake" => sf_auth_mode.get(),
            "databricks" => db_auth_mode.get(),
            "synapse" => synapse_auth_mode.get(),
            _ => String::new(),
        };
        connection_step_satisfied_from(
            &t,
            &auth_mode,
            modal_oauth_connected.get(),
            test_result.get().map(|r| r.success).unwrap_or(false),
        )
    });

    // ── BigQuery kyomi_oauth access-confirmation gates (KYO-408, KYO-477) ───
    // Kyomi's shared Google OAuth app only accepts Google accounts a Kyomi
    // admin has added as test users in the Cloud Console consent screen —
    // Kyomi has no programmatic access to that list, so Google is the only
    // enforcement layer. Neither gate below is a security control: there is
    // nothing here for Kyomi to protect, and no dishonest tick bypasses
    // anything Google wouldn't already stop. They exist purely so the user
    // pauses long enough to request access before burning a doomed OAuth
    // round-trip — see the notice + checkbox this reads, in
    // `BigQueryAuthModeSection`'s kyomi_oauth `<Show>` block.
    //
    // Deliberately kept separate from `connection_step_satisfied` above:
    // that signal answers "is the Connection tab done" and gates
    // Next/the Catalog tab, matching the KYO-404 create-mode flow. Both
    // gates below answer a narrower question — "has the user acknowledged
    // the OAuth allowlist" — never tab navigation.
    //
    // TWO signals, not one shared between Save/Create and Connect/Reconnect.
    // KYO-427 originally pointed Connect/Reconnect at the *same* signal
    // Save/Create reads (reasoning: "Save/Create alone is unreachable in
    // create mode until OAuth already succeeded, so it gated nothing where
    // it mattered" — true, but the fix was wrong). KYO-477 is the fix to
    // that fix: `oauth_connected` is an account-level signal for
    // kyomi_oauth (one Google link per Kyomi user, shared across every
    // BigQuery kyomi_oauth datasource forever), so OR-ing it into the
    // Connect gate — as the shared signal did — makes Connect's gate a
    // permanent no-op for anyone who has ever linked Google once,
    // regardless of `access_confirmed`. Reported three times in
    // production before this fix. Read each signal's own inline comment
    // below before "simplifying" them back into one — that is exactly the
    // regression KYO-477 exists to prevent, and this is NOT the KYO-423
    // "duplicated predicate" anti-pattern: these two answer genuinely
    // different questions (see `bq_kyomi_oauth_connect_allowed`'s doc
    // comment for the full explanation).
    //
    // `bq_kyomi_oauth_access_ok` — gates Save/Create only (read directly by
    // the footer buttons below, and NOT threaded into
    // `BigQueryAuthModeSection`). Auto-satisfied once `modal_oauth_connected`
    // is true: a successful OAuth handshake for *this* linked account IS
    // proof that account was already allowlisted (Google would have
    // refused it otherwise), so there is nothing left to confirm before
    // saving a datasource that already has a working connection — this is
    // also what keeps a returning, already-connected user from being
    // nagged by the checkbox on every visit. (`bq_access_confirmed` is
    // separately persisted to localStorage — see its own doc comment —
    // but that persistence is orthogonal to this auto-satisfaction path:
    // a user can reach "not nagged" either by a proven OAuth connection or
    // by a remembered checkbox tick, and this predicate only needs the
    // former.)
    let bq_kyomi_oauth_access_ok: Signal<bool> = Signal::derive(move || {
        bq_kyomi_oauth_access_gate_satisfied(
            &ds_type.get(),
            &bq_auth_mode.get(),
            modal_oauth_connected.get(),
            bq_access_confirmed.get(),
        )
    });

    // `bq_kyomi_oauth_connect_ok` — gates the Connect/Reconnect button
    // (`connect_blocked` on `ModalOAuthStatusPanel`, threaded into
    // `BigQueryAuthModeSection` below) only. Deliberately does NOT read
    // `modal_oauth_connected` — see `bq_kyomi_oauth_connect_allowed`'s doc
    // comment for why folding that account-level signal in here would
    // reintroduce KYO-477.
    let bq_kyomi_oauth_connect_ok: Signal<bool> = Signal::derive(move || {
        bq_kyomi_oauth_connect_allowed(
            &ds_type.get(),
            &bq_auth_mode.get(),
            bq_access_confirmed.get(),
        )
    });

    // KYO-499 — keep `bq_access_confirmed` in sync with
    // `localStorage["hasBetaAccess"]` across tabs/surfaces: this modal's
    // kyomi_oauth notice and the pre-auth Google sign-in notice in
    // `pages/auth/login.rs` read/write the same key via `utils::beta_access`.
    // Installed once here — `DatasourceModal` mounts once for the settings
    // page's lifetime (visibility toggles via the `open` prop; it is never
    // conditionally mounted/unmounted, see `reset_form`'s own doc context
    // above) — mirroring the OAuth postMessage listener pattern earlier in
    // this file (`install_oauth_listener`, `DatasourcesList`).
    #[cfg(target_arch = "wasm32")]
    {
        use crate::utils::beta_access::install_beta_access_listener;
        let cleanup = install_beta_access_listener(move |value| {
            set_bq_access_confirmed.try_set(value);
        });
        // Box<dyn FnOnce()> is used so the inner cleanup can be called
        // through Drop without requiring Send. SendWrapper makes the box
        // Send+Sync for on_cleanup's bound while guaranteeing
        // single-threaded access on WASM — same pattern as the OAuth
        // postMessage listener's cleanup above.
        let cleanup_cell = std::cell::Cell::new(Some(Box::new(cleanup) as Box<dyn FnOnce()>));
        let cleanup_wrapper = send_wrapper::SendWrapper::new(cleanup_cell);
        on_cleanup(move || {
            if let Some(f) = cleanup_wrapper.take().take() {
                f();
            }
        });
    }

    // ── Datasource-type registry data (KYO-274) ─────────────────────────────
    // Which auth modes the four Authentication Mode selectors below offer —
    // and their labels/descriptions — is registry-owned
    // (`DatasourceTypeMetadata::auth_modes`), not hardcoded per component.
    // `use_query` is the shared list-query cache keyed by "datasource-types";
    // `EditModeCatalogTab` already calls it for `indexing_auth_modes` (KYO-187),
    // so this reuses the same cached fetch rather than adding a second one.
    let datasource_types = use_query("datasource-types", || (), |_: ()| get_datasource_types());

    // Observe the registry fetch's error state via `connection_auth_modes_unavailable_from`
    // (defined above `DatasourceModal`) — never inline inside `connection_auth_modes`
    // below. That derive also reads `ds_type`, and `Signal::derive` re-runs
    // its *entire* body on every dependency change, not just its own
    // (docs/CODING_STANDARDS.md: "Signal::derive is not memoized"); a
    // `warn!` living there would re-fire the same stale failure every time
    // the user switches provider — the exact KYO-240 shape this document
    // warns about. Wrapping the pure fn in a `Memo` scoped ONLY to
    // `datasource_types` means it only recomputes — and therefore only
    // re-logs — when the fetch outcome itself changes.
    let connection_auth_modes_unavailable: Memo<bool> =
        Memo::new(move |_| connection_auth_modes_unavailable_from(&datasource_types.get()));

    // Per-type option list. A cheap, pure projection — the `.ok()` here is
    // safe because the error case is already observed above, on its own
    // Memo keyed on the query result alone; this derive re-running per
    // `ds_type` switch has no side effect of its own.
    let connection_auth_modes: Signal<Vec<AuthModeOption>> = Signal::derive(move || {
        let ds_type_val = ds_type.get();
        datasource_types
            .get()
            .and_then(|r| r.ok())
            .into_iter()
            .flatten()
            .find(|t| t.type_id == ds_type_val)
            .map(|t| t.connection_auth_modes)
            .unwrap_or_default()
    });

    // Footer must be Arc<dyn Fn() -> AnyView + Send + Sync> (Leptos ChildrenFn).
    let do_save_for_footer = do_save;
    let footer: std::sync::Arc<dyn Fn() -> leptos::prelude::AnyView + Send + Sync> =
        std::sync::Arc::new(move || {
            let do_save = do_save_for_footer;
            view! {
                <Show
                    when=move || is_create_mode.get()
                    fallback=move || {
                        let do_save = do_save;
                        let is_saving = save_action.pending().get();
                        let sample = is_sample.get();
                        let connect = is_connect.get();
                        // Connect + sample both render as read-only in the
                        // body, so neither has a Save action. Label the
                        // dismiss button "Close" in both cases to signal the
                        // read-only nature.
                        let read_only = sample || connect;
                        view! {
                            // Edit mode footer.
                            //
                            // The sample datasource is intentionally read-only:
                            // its connection settings are managed by the server
                            // via `SAMPLE_CLICKHOUSE_*` env vars, so there is
                            // nothing for the user to save. Connect datasources
                            // are also read-only here — connection settings
                            // live on the remote agent, not in this modal. We
                            // surface a single "Close" button (no "Save") to
                            // make the read-only nature explicit, matching the
                            // React reference in `apps/frontend/src/components/
                            // settings/datasources/DatasourceModal.jsx` for
                            // both the sample and `connection_type === 'connect'`
                            // branches.
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| on_close.run(())
                            >
                                {if read_only { "Close" } else { "Cancel" }}
                            </Button>
                            <Show when=move || !is_sample.get() && !is_connect.get()>
                                <Button
                                    // KYO-408: `bq_kyomi_oauth_access_ok` folds in the
                                    // Save gate — a no-op for every non-BigQuery
                                    // provider and every BigQuery mode other than
                                    // kyomi_oauth (see its own doc comment above).
                                    disabled=Signal::derive(move || {
                                        is_saving || !bq_kyomi_oauth_access_ok.get()
                                    })
                                    on:click=move |_| do_save()
                                >
                                    {move || {
                                        if save_action.pending().get() {
                                            "Saving..."
                                        } else if is_admin.get() {
                                            "Save"
                                        } else {
                                            "Save Credentials"
                                        }
                                    }}
                                </Button>
                            </Show>
                        }.into_any()
                    }
                >
                    // Create mode footer.
                    //
                    // Three distinct sub-modes, each with its own footer:
                    //   1. Direct create: Cancel + Next/Create (existing flow).
                    //   2. Connect create, pre-submit: Cancel + "Create Connect
                    //      Datasource" which calls `do_create_connect`.
                    //   3. Connect create, post-submit (token in hand): a
                    //      single "Done" button which calls `on_saved` —
                    //      same callback the direct-create success path uses,
                    //      so the modal closes and the datasource list
                    //      refreshes.
                    {move || {
                        let do_save = do_save;
                        let do_create_connect = do_create_connect;
                        // Post-create view (Connect create finished) — single Done button.
                        if connect_create_complete.get() {
                            return view! {
                                <Button
                                    on:click=move |_| {
                                        on_saved.run(DatasourceResult {
                                            id: String::new(),
                                            slug: String::new(),
                                            name: connect_created_name.get_untracked(),
                                            datasource_type: connect_created_type.get_untracked(),
                                        });
                                    }
                                >
                                    "Done"
                                </Button>
                            }.into_any();
                        }

                        // Pre-submit Connect create — single "Create Connect
                        // Datasource" action; no tab navigation.
                        if is_connect.get() {
                            return view! {
                                <Button
                                    variant=ButtonVariant::Outline
                                    on:click=move |_| on_close.run(())
                                >
                                    "Cancel"
                                </Button>
                                <Button
                                    disabled=Signal::derive(move || creating_connect.get() || name.get().trim().is_empty())
                                    on:click=move |_| do_create_connect()
                                >
                                    {move || if creating_connect.get() {
                                        "Creating..."
                                    } else {
                                        "Create Connect Datasource"
                                    }}
                                </Button>
                            }.into_any();
                        }

                        // Direct create footer (original behavior).
                        let is_connection_tab = active_tab.get() == "connection";
                        // `connection_step_satisfied` (defined above, alongside
                        // `discovery_succeeded`) is the shared predicate — it
                        // folds in the narrow BigQuery enterprise_oauth
                        // precreate exception. `can_next` additionally
                        // requires a non-empty name; that check belongs only
                        // here, not in the shared predicate, since the
                        // Catalog tab pill below has no such requirement.
                        let can_next = connection_step_satisfied.get() && !name.get().is_empty();
                        let is_saving = save_action.pending().get();
                        view! {
                            <Button
                                variant=ButtonVariant::Outline
                                on:click=move |_| on_close.run(())
                            >
                                "Cancel"
                            </Button>
                            {if is_connection_tab {
                                view! {
                                    <Button
                                        disabled=!can_next
                                        on:click=move |_| set_active_tab.set("catalog".to_string())
                                    >
                                        "Next"
                                    </Button>
                                }.into_any()
                            } else {
                                view! {
                                    <Button
                                        // KYO-408: does not gate "Next" above, only the
                                        // final Create — matches the React reference
                                        // (`DatasourceModal.jsx`'s requiresBetaAccess
                                        // check lived on Create/Save only).
                                        disabled=Signal::derive(move || {
                                            is_saving || !bq_kyomi_oauth_access_ok.get()
                                        })
                                        on:click=move |_| do_save()
                                    >
                                        {move || if save_action.pending().get() { "Creating..." } else { "Create" }}
                                    </Button>
                                }.into_any()
                            }}
                        }.into_any()
                    }}
                </Show>
            }.into_any()
        });

    view! {
        <Modal
            show=open
            on_close=on_close
            title=modal_title
            size=ModalSize::Lg
            footer=footer.clone()
        >

            {move || {
                let do_test_and_discover = do_test_and_discover;
                view! {
                    // Error message
                    {move || error_msg.get().map(|msg| view! {
                        <Alert variant=AlertVariant::Error class="mb-4">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}

                    // Loading state (edit mode)
                    <Show when=move || settings_loading.get()>
                        <div class="flex items-center justify-center min-h-[400px]">
                            <span class="text-sm text-muted-foreground">"Loading settings..."</span>
                        </div>
                    </Show>

                    <Show when=move || !settings_loading.get()>
                        // ── Connection Method toggle (create mode only) ──
                        //
                        // Large two-column card selector. Follows the
                        // DESIGN.md "radio card" pattern used for selected /
                        // unselected choice cards: `rounded-lg border` with
                        // selected state switching to `border-primary
                        // bg-primary/5`. Hidden once the user has clicked
                        // "Create Connect Datasource" successfully — the
                        // post-create view below takes over the modal body.
                        <Show when=move || is_create_mode.get() && !connect_create_complete.get()>
                            <div class="mb-5">
                                <label class="block text-sm font-medium text-foreground mb-2">
                                    "Connection Method"
                                </label>
                                <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            set_connection_type.set("direct".to_string());
                                            set_error_msg.set(None);
                                        }
                                        class=move || {
                                            let selected = connection_type.get() == "direct";
                                            if selected {
                                                "p-4 rounded-lg border text-left transition-colors border-primary/20 bg-primary/10 text-primary"
                                            } else {
                                                "p-4 rounded-lg border text-left transition-colors border-border hover:border-muted-foreground/30 text-foreground"
                                            }
                                        }
                                    >
                                        <div class="font-medium text-sm">
                                            "Direct Connection"
                                        </div>
                                        <div class="text-xs text-muted-foreground mt-1">
                                            "Connect directly from Kyomi to your database."
                                        </div>
                                    </button>
                                    <button
                                        type="button"
                                        on:click=move |_| {
                                            set_connection_type.set("connect".to_string());
                                            set_error_msg.set(None);
                                            // If the user had picked a type
                                            // that isn't Connect-compatible
                                            // (e.g. BigQuery), snap to the
                                            // first Connect-compatible type
                                            // so the simplified form below
                                            // doesn't render with a disabled
                                            // placeholder value.
                                            let current = ds_type.get_untracked();
                                            let compatible = CONNECT_TYPES
                                                .iter()
                                                .any(|(v, _)| *v == current);
                                            if !compatible
                                                && let Some((v, _)) = CONNECT_TYPES.first()
                                            {
                                                set_ds_type.set((*v).to_string());
                                            }
                                        }
                                        class=move || {
                                            let selected = connection_type.get() == "connect";
                                            if selected {
                                                "p-4 rounded-lg border text-left transition-colors border-primary/20 bg-primary/10 text-primary"
                                            } else {
                                                "p-4 rounded-lg border text-left transition-colors border-border hover:border-muted-foreground/30 text-foreground"
                                            }
                                        }
                                    >
                                        <div class="font-medium text-sm">
                                            "Kyomi Connect"
                                        </div>
                                        <div class="text-xs text-muted-foreground mt-1">
                                            "Self-hosted agent — works through firewalls."
                                        </div>
                                    </button>
                                </div>
                            </div>
                        </Show>

                        // ── Post-create Connect view ──
                        //
                        // Rendered when the user has successfully created a
                        // Connect datasource in this session. Replaces the
                        // whole content area (tab bar + connection form) with
                        // a one-time token display + the four deployment
                        // command tabs. Footer provides a single "Done"
                        // button which closes the modal and refreshes the
                        // datasource list.
                        <Show when=move || connect_create_complete.get() && is_create_mode.get()>
                            <ConnectCreateSuccessView
                                // `connect_create_complete` (the Show gate) and this
                                // accessor are independent readers of `connect_token`
                                // with no guaranteed update ordering. On a Some→None
                                // transition the accessor can re-run before the Show
                                // tears the child down, so degrade to an empty token
                                // instead of panicking (KYO-121).
                                token=Signal::derive(move || connect_token.get().unwrap_or_default())
                                datasource_name=Signal::derive(move || connect_created_name.get())
                                datasource_type=Signal::derive(move || connect_created_type.get())
                                active_tab=active_deploy_tab
                                set_active_tab=set_active_deploy_tab
                            />
                        </Show>

                        // Edit-mode tab bar — Connection | Catalog.
                        // Hidden for sample datasources (read-only; no catalog management applies).
                        <Show when=move || {
                            !is_create_mode.get()
                                && !is_sample.get()
                        }>
                            <div class="flex border-b border-border mb-4">
                                <button
                                    class=move || if active_tab.get() == "connection" { TAB_ACTIVE } else { TAB_INACTIVE }
                                    on:click=move |_| set_active_tab.set("connection".to_string())
                                >
                                    "Connection"
                                </button>
                                // Catalog tab is workspace-admin-only (KYO-184): every field on
                                // it — scope selection, dedicated indexing credentials — persists
                                // through `update_datasource_settings`, and "Refresh Now" is
                                // separately gated by `refresh_catalog`'s admin check
                                // (`server_fns/sql_editor.rs:674-686`). Hiding the tab is what
                                // satisfies "Refresh Catalog is not rendered for non-admins" — do
                                // NOT also gate the Refresh Now button itself, that would be dead
                                // code a non-admin can never reach.
                                <Show when=move || is_admin.get()>
                                    <button
                                        class=move || if active_tab.get() == "catalog" { TAB_ACTIVE } else { TAB_INACTIVE }
                                        on:click=move |_| set_active_tab.set("catalog".to_string())
                                    >
                                        "Catalog"
                                    </button>
                                </Show>
                            </div>
                        </Show>

                        // Tab bar — hidden in Connect create-mode (the simplified
                        // Connect form has no separate Catalog step; the catalog
                        // is indexed automatically after agent registration) and
                        // hidden after a successful Connect create (the post-
                        // create view owns the whole body).
                        <Show when=move || {
                            is_create_mode.get()
                                && !is_connect.get()
                                && !connect_create_complete.get()
                        }>
                            <div class="flex border-b border-border mb-4">
                                <button
                                    class=move || if active_tab.get() == "connection" { TAB_ACTIVE } else { TAB_INACTIVE }
                                    on:click=move |_| set_active_tab.set("connection".to_string())
                                >
                                    "Connection"
                                </button>
                                <button
                                    class=move || {
                                        let can_go = connection_step_satisfied.get();
                                        if active_tab.get() == "catalog" { TAB_ACTIVE }
                                        else if can_go { TAB_INACTIVE }
                                        else { TAB_DISABLED }
                                    }
                                    disabled=move || !connection_step_satisfied.get()
                                    on:click=move |_| {
                                        if connection_step_satisfied.get_untracked() {
                                            set_active_tab.set("catalog".to_string());
                                        }
                                    }
                                >
                                    "Catalog"
                                </button>
                            </div>
                        </Show>

                        // ── Connect create-mode simplified form ──
                        //
                        // When the user picked "Kyomi Connect" above, we
                        // show a compact form: name + restricted type
                        // selector + a short info panel. No credentials,
                        // no test-connection, no catalog tab — the agent
                        // owns the connection. The submit button lives in
                        // the footer.
                        <Show when=move || {
                            is_create_mode.get()
                                && is_connect.get()
                                && !connect_create_complete.get()
                        }>
                            <ConnectCreateForm
                                name=name
                                set_name=set_name
                                slug=slug
                                set_slug=set_slug
                                slug_manually_edited=slug_manually_edited
                                set_slug_manually_edited=set_slug_manually_edited
                                ds_type=ds_type
                                set_ds_type=set_ds_type
                            />
                        </Show>

                        // Content area (min-height from React). Hidden in
                        // Connect create-mode (simplified form takes over)
                        // and post-create (deployment view takes over).
                        <Show when=move || {
                            !(connect_create_complete.get()
                                || is_create_mode.get() && is_connect.get())
                        }>
                        <div class="space-y-4 min-h-[400px]">

                            // ── CONNECTION TAB ──
                            <Show when=move || active_tab.get() == "connection">
                                <div class="space-y-4">

                                    // ── Sample datasource read-only view (edit mode) ──
                                    // Matches the React `isSampleDatasource` branch in
                                    // renderWorkspaceSettingsTab. Shows Name/Slug/Type as
                                    // non-editable text with an informational alert.
                                    <Show when=move || !is_create_mode.get() && is_sample.get()>
                                        <div class="space-y-4">
                                            <Alert>
                                                <AlertDescription>
                                                    "This is a sample datasource with pre-configured connection settings. Connection settings cannot be modified."
                                                </AlertDescription>
                                            </Alert>
                                            <div class="grid grid-cols-2 gap-4">
                                                <div>
                                                    <label class="block text-sm font-medium mb-1 text-muted-foreground">"Name"</label>
                                                    <p class="text-sm">{move || name.get()}</p>
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium mb-1 text-muted-foreground">"Slug"</label>
                                                    <p class="text-sm font-mono">{move || slug.get()}</p>
                                                </div>
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium mb-1 text-muted-foreground">"Type"</label>
                                                <div class="flex items-center gap-2">
                                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                                        <Icon icon=phosphor_leptos::DATABASE/>
                                                    </span>
                                                    <span class="text-sm">
                                                        {move || {
                                                            let t = ds_type.get();
                                                            PROVIDER_TYPES
                                                                .iter()
                                                                .find(|(v, _)| *v == t)
                                                                .map(|(_, l)| (*l).to_string())
                                                                .unwrap_or(t)
                                                        }}
                                                    </span>
                                                </div>
                                            </div>
                                        </div>
                                    </Show>

                                    // ── Non-admin edit-mode read-only view (KYO-184) ──
                                    // A non-admin cannot persist connection-config changes —
                                    // `update_datasource_settings` is workspace-admin-gated
                                    // (`server_fns/datasources.rs`) — so presenting the editable
                                    // connection-config inputs would let them type into fields
                                    // that silently never save. Mirrors the shape of the sample
                                    // read-only branch above; excludes samples since that branch
                                    // already covers them for every role. The credentials section
                                    // and provider OAuth connect/disconnect UI further down are
                                    // NOT part of this gate — personal credential entry still
                                    // works for non-admins.
                                    <Show when=move || !is_create_mode.get() && !is_admin.get() && !is_sample.get()>
                                        <div class="space-y-4">
                                            <Alert>
                                                <AlertDescription>
                                                    "Connection settings are managed by workspace admins. You can still connect your own credentials below."
                                                </AlertDescription>
                                            </Alert>
                                            <div class="grid grid-cols-2 gap-4">
                                                <div>
                                                    <label class="block text-sm font-medium mb-1 text-muted-foreground">"Name"</label>
                                                    <p class="text-sm">{move || name.get()}</p>
                                                </div>
                                                <div>
                                                    <label class="block text-sm font-medium mb-1 text-muted-foreground">"Slug"</label>
                                                    <p class="text-sm font-mono">{move || slug.get()}</p>
                                                </div>
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium mb-1 text-muted-foreground">"Type"</label>
                                                <div class="flex items-center gap-2">
                                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                                        <Icon icon=phosphor_leptos::DATABASE/>
                                                    </span>
                                                    <span class="text-sm">
                                                        {move || {
                                                            let t = ds_type.get();
                                                            PROVIDER_TYPES
                                                                .iter()
                                                                .find(|(v, _)| *v == t)
                                                                .map(|(_, l)| (*l).to_string())
                                                                .unwrap_or(t)
                                                        }}
                                                    </span>
                                                </div>
                                            </div>
                                        </div>
                                    </Show>

                                    // ── Sample quick-add tile (create mode only) ──
                                    // Matches the React sample quick-add block at the top of
                                    // the connection tab. Only visible for admins when the
                                    // sample ClickHouse is configured and not already added.
                                    <Show when=move || {
                                        is_create_mode.get()
                                            && sample_available.get()
                                            && !sample_already_added.get()
                                    }>
                                        <div class="flex items-center justify-between p-3 border border-border rounded-lg bg-muted/30">
                                            <div class="flex items-center gap-3">
                                                <span class="h-5 w-5 inline-flex items-center justify-center text-muted-foreground">
                                                    <Icon icon=phosphor_leptos::DATABASE/>
                                                </span>
                                                <div>
                                                    <p class="text-sm font-medium">"Acme Analytics (Sample)"</p>
                                                    <p class="text-xs text-muted-foreground">
                                                        "Try Kyomi with demo data — no setup required"
                                                    </p>
                                                </div>
                                            </div>
                                            <Button
                                                variant=ButtonVariant::Outline
                                                size=ButtonSize::Sm
                                                disabled=creating_sample
                                                on:click=move |_| do_create_sample()
                                            >
                                                {move || if creating_sample.get() { "Adding..." } else { "Add Sample" }}
                                            </Button>
                                        </div>
                                    </Show>

                                    // Rest of the connection form — hidden when viewing a sample datasource.
                                    <Show when=move || is_create_mode.get() || !is_sample.get()>
                                    <div class="space-y-4">

                                    // Type selector (create mode only)
                                    <Show when=move || is_create_mode.get()>
                                        <div>
                                            <label class="block text-sm font-medium mb-1">"Type"</label>
                                            <Select
                                                value=Signal::derive(move || ds_type.get())
                                                options=Signal::stored(PROVIDER_TYPES.iter().map(|(v, l)| (v.to_string(), l.to_string())).collect::<Vec<_>>())
                                                on_change=move |val: String| {
                                                    set_ds_type.set(val);
                                                    // Reset discovery when type changes
                set_discovery_status.set("idle".to_string());
                set_catalog_scope_touched.set(false);
                                                    set_test_result.set(None);
                                                    set_discovered_databases.set(vec![]);
                                                    set_discovered_schemas.set(vec![]);
                                                    set_discovered_warehouses.set(vec![]);
                                                    set_discovered_catalogs.set(vec![]);
                                                    set_catalog_discovery_denied.set(false);
                                                    // Invalidate create-mode catalog selections
                                                    // too — discovered items are for the old type.
                                                    set_create_catalog_selected.set(vec![]);
                                                    set_create_catalog_text.set(String::new());
                                                    set_create_include_public_datasets.set(false);
                                                    // KYO-468 — the discovered BigQuery
                                                    // project list belongs to whichever type
                                                    // was previously selected; switching away
                                                    // from bigquery (or between bigquery and
                                                    // itself via a different provider round
                                                    // trip) must not let a stale list survive
                                                    // to be misread as belonging to the new
                                                    // type once bigquery is reselected.
                                                    reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);
                                                }
                                            />
                                        </div>
                                    </Show>

                                    // Name & Slug (admin fields) — hidden for non-admins in edit
                                    // mode (KYO-184): `update_datasource_settings` is
                                    // workspace-admin-gated, so a non-admin's edits here would
                                    // silently fail to persist. The read-only summary branch
                                    // above already shows these values. Always shown in create
                                    // mode, which is admin-only-reachable anyway (header/empty-
                                    // state CTAs are admin-gated).
                                    <Show when=move || is_create_mode.get() || is_admin.get()>
                                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                        <div>
                                            <label class="block text-sm font-medium mb-1">
                                                "Name "
                                                <span class="text-error-foreground">"*"</span>
                                            </label>
                                            <input
                                                type="text"
                                                class=MODAL_INPUT_CLASS
                                                placeholder="Production Database"
                                                prop:value=move || name.get()
                                                on:input=move |ev| {
                                                    handle_name_change(event_target_value(&ev));
                                                }
                                            />
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium mb-1">"Slug"</label>
                                            <input
                                                type="text"
                                                class=format!("{} font-mono", MODAL_INPUT_CLASS)
                                                placeholder="production-db"
                                                prop:value=move || slug.get()
                                                on:input=move |ev| {
                                                    set_slug_manually_edited.set(true);
                                                    let val = event_target_value(&ev)
                                                        .to_lowercase()
                                                        .chars()
                                                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                                                        .collect::<String>();
                                                    set_slug.set(val);
                                                }
                                            />
                                            <p class="text-xs text-muted-foreground mt-1">
                                                {move || if is_create_mode.get() {
                                                    "Auto-generated from name if left empty"
                                                } else {
                                                    "Used in ChartML specs and API calls"
                                                }}
                                            </p>
                                        </div>
                                    </div>
                                    </Show>

                                    // ── Kyomi Connect status panel ──
                                    // For Connect datasources, the agent owns
                                    // the connection config (host, port,
                                    // credentials), not this modal. Show the
                                    // status / rotate-token / disconnect UI
                                    // instead of the per-provider form.
                                    // create-mode is gated out because Connect
                                    // datasources are created via the dedicated
                                    // Connect Setup page.
                                    <Show
                                        when=move || is_connect.get() && !is_create_mode.get()
                                        // Use a fallback-free closure so the
                                        // panel is freshly mounted (and its
                                        // polling interval freshly spawned)
                                        // each time the user opens a Connect
                                        // datasource in the modal.
                                    >
                                        {move || {
                                            let ds_id = datasource_id
                                                .get()
                                                .unwrap_or_default();
                                            let ds_type_val = ds_type.get();
                                            view! {
                                                <ConnectStatusPanel
                                                    datasource_id=ds_id
                                                    datasource_type=ds_type_val
                                                />
                                            }
                                        }}
                                    </Show>

                                    // Everything from auth-mode selectors onward
                                    // is hidden for Connect datasources — the
                                    // ConnectStatusPanel above replaces them.
                                    <Show when=move || !is_connect.get()>
                                    <div class="space-y-4">

                                    // Registry fetch failed — without this, the four
                                    // Authentication Mode selectors below would each
                                    // silently render with zero options and no
                                    // explanation (bigquery/snowflake/databricks/synapse
                                    // are all registry-defined, so a successful fetch
                                    // that simply omits the current type isn't an
                                    // expected case here — this narrows to the actual
                                    // fetch failure).
                                    <Show when=move || connection_auth_modes_unavailable.get()>
                                        <Alert variant=AlertVariant::Warning>
                                            <AlertDescription>
                                                "Couldn't load authentication mode options. Please refresh and try again."
                                            </AlertDescription>
                                        </Alert>
                                    </Show>

                                    <Show when=move || !connection_auth_modes_unavailable.get()>

                                    // BigQuery auth mode selector
                                    <Show when=move || ds_type.get() == "bigquery">
                                        <BigQueryAuthModeSection
                                            bq_auth_mode=bq_auth_mode
                                            set_bq_auth_mode=set_bq_auth_mode
                                            cfg_oauth_client_id=cfg_oauth_client_id
                                            set_cfg_oauth_client_id=set_cfg_oauth_client_id
                                            cfg_oauth_client_secret=cfg_oauth_client_secret
                                            set_cfg_oauth_client_secret=set_cfg_oauth_client_secret
                                            cfg_service_account_json=cfg_service_account_json
                                            set_cfg_service_account_json=set_cfg_service_account_json
                                            service_account_email=service_account_email
                                            set_service_account_email=set_service_account_email
                                            slug=slug
                                            cred_billing_project=cred_billing_project
                                            set_cred_billing_project=set_cred_billing_project
                                            oauth_connected=modal_oauth_connected
                                            set_oauth_connected=set_modal_oauth_connected
                                            oauth_email=modal_oauth_email
                                            set_oauth_email=set_modal_oauth_email
                                            oauth_expired=modal_oauth_expired
                                            set_oauth_expired=set_modal_oauth_expired
                                            oauth_connecting=modal_oauth_connecting
                                            set_oauth_connecting=set_modal_oauth_connecting
                                            google_disconnect_action=google_disconnect_action
                                            datasource_disconnect_action=datasource_disconnect_action
                                            is_create_mode=is_create_mode
                                            bq_projects=bq_projects
                                            set_bq_projects=set_bq_projects
                                            bq_projects_loading=bq_projects_loading
                                            bq_projects_error=bq_projects_error
                                            set_bq_projects_error=set_bq_projects_error
                                            set_bq_projects_attempted=set_bq_projects_attempted
                                            is_admin=is_admin
                                            auth_modes=connection_auth_modes
                                            test_result=test_result
                                            set_test_result=set_test_result
                                            set_discovery_status=set_discovery_status
                                            test_pending=bq_test_pending
                                            on_validate=on_bq_validate
                                            bq_access_confirmed=bq_access_confirmed
                                            set_bq_access_confirmed=set_bq_access_confirmed
                                            bq_kyomi_oauth_connect_ok=bq_kyomi_oauth_connect_ok
                                        />
                                    </Show>

                                    // Snowflake auth mode selector
                                    <Show when=move || ds_type.get() == "snowflake">
                                        <SnowflakeAuthModeSection
                                            sf_auth_mode=sf_auth_mode
                                            set_sf_auth_mode=set_sf_auth_mode
                                            slug=slug
                                            oauth_connected=modal_oauth_connected
                                            set_oauth_connected=set_modal_oauth_connected
                                            oauth_email=modal_oauth_email
                                            set_oauth_email=set_modal_oauth_email
                                            oauth_expired=modal_oauth_expired
                                            set_oauth_expired=set_modal_oauth_expired
                                            oauth_connecting=modal_oauth_connecting
                                            set_oauth_connecting=set_modal_oauth_connecting
                                            datasource_disconnect_action=datasource_disconnect_action
                                            is_create_mode=is_create_mode
                                            cfg_oauth_client_id=cfg_oauth_client_id
                                            cfg_oauth_client_secret=cfg_oauth_client_secret
                                            is_admin=is_admin
                                            auth_modes=connection_auth_modes
                                            set_test_result=set_test_result
                                            set_discovery_status=set_discovery_status
                                        />
                                    </Show>

                                    // Databricks auth mode selector
                                    <Show when=move || ds_type.get() == "databricks">
                                        <DatabricksAuthModeSection
                                            db_auth_mode=db_auth_mode
                                            set_db_auth_mode=set_db_auth_mode
                                            slug=slug
                                            oauth_connected=modal_oauth_connected
                                            set_oauth_connected=set_modal_oauth_connected
                                            oauth_email=modal_oauth_email
                                            set_oauth_email=set_modal_oauth_email
                                            oauth_expired=modal_oauth_expired
                                            set_oauth_expired=set_modal_oauth_expired
                                            oauth_connecting=modal_oauth_connecting
                                            set_oauth_connecting=set_modal_oauth_connecting
                                            datasource_disconnect_action=datasource_disconnect_action
                                            is_create_mode=is_create_mode
                                            cfg_oauth_client_id=cfg_oauth_client_id
                                            set_cfg_oauth_client_id=set_cfg_oauth_client_id
                                            cfg_oauth_client_secret=cfg_oauth_client_secret
                                            set_cfg_oauth_client_secret=set_cfg_oauth_client_secret
                                            is_admin=is_admin
                                            auth_modes=connection_auth_modes
                                            set_test_result=set_test_result
                                            set_discovery_status=set_discovery_status
                                        />
                                    </Show>

                                    // Synapse auth mode selector
                                    <Show when=move || ds_type.get() == "synapse">
                                        <SynapseAuthModeSection
                                            synapse_auth_mode=synapse_auth_mode
                                            set_synapse_auth_mode=set_synapse_auth_mode
                                            slug=slug
                                            cfg_oauth_client_id=cfg_oauth_client_id
                                            set_cfg_oauth_client_id=set_cfg_oauth_client_id
                                            cfg_oauth_client_secret=cfg_oauth_client_secret
                                            set_cfg_oauth_client_secret=set_cfg_oauth_client_secret
                                            oauth_connected=modal_oauth_connected
                                            set_oauth_connected=set_modal_oauth_connected
                                            oauth_email=modal_oauth_email
                                            set_oauth_email=set_modal_oauth_email
                                            oauth_expired=modal_oauth_expired
                                            set_oauth_expired=set_modal_oauth_expired
                                            oauth_connecting=modal_oauth_connecting
                                            set_oauth_connecting=set_modal_oauth_connecting
                                            datasource_disconnect_action=datasource_disconnect_action
                                            is_create_mode=is_create_mode
                                            is_admin=is_admin
                                            auth_modes=connection_auth_modes
                                            set_test_result=set_test_result
                                            set_discovery_status=set_discovery_status
                                        />
                                    </Show>

                                    </Show>

                                    // Connection fields (provider-specific) — workspace-admin-only
                                    // (KYO-184): these persist through `update_datasource_settings`,
                                    // which non-admins cannot call, so editing them would silently
                                    // do nothing on save.
                                    <Show when=move || is_admin.get()>
                                    <ProviderConnectionFields
                                        signals=ConnectionFieldsSignals {
                                            ds_type,
                                            sf_auth_mode,
                                            synapse_auth_mode,
                                            cfg_host,
                                            set_cfg_host,
                                            cfg_port,
                                            set_cfg_port,
                                            cfg_ssl_mode,
                                            set_cfg_ssl_mode,
                                            cfg_database,
                                            set_cfg_database,
                                            cfg_account,
                                            set_cfg_account,
                                            cfg_server_hostname,
                                            set_cfg_server_hostname,
                                            cfg_http_path,
                                            set_cfg_http_path,
                                            cfg_secure,
                                            set_cfg_secure,
                                            cfg_encrypt,
                                            set_cfg_encrypt,
                                            cfg_trust_cert,
                                            set_cfg_trust_cert,
                                            cfg_tenant_id,
                                            set_cfg_tenant_id,
                                            cfg_oauth_client_id,
                                            set_cfg_oauth_client_id,
                                            cfg_oauth_client_secret,
                                            set_cfg_oauth_client_secret,
                                        }
                                    />
                                    </Show>

                                    // SSH tunnel section — SSH-capable types, workspace admins only.
                                    <Show when=move || is_admin.get() && supports_ssh_tunnel(&ds_type.get())>
                                        <SshTunnelSection
                                            signals=SshTunnelSignals {
                                                cfg_ssh_enabled,
                                                set_cfg_ssh_enabled,
                                                cfg_ssh_host,
                                                set_cfg_ssh_host,
                                                cfg_ssh_port,
                                                set_cfg_ssh_port,
                                                cfg_ssh_username,
                                                set_cfg_ssh_username,
                                                ssh_public_key,
                                                set_ssh_public_key,
                                                set_ssh_private_key_generated,
                                                ssh_key_generating,
                                                ssh_key_action,
                                                cfg_ssh_host_fingerprint,
                                                set_cfg_ssh_host_fingerprint,
                                                cfg_ssh_key_mode,
                                                set_cfg_ssh_key_mode,
                                                cfg_ssh_private_key_input,
                                                set_cfg_ssh_private_key_input,
                                                cfg_ssh_passphrase,
                                                set_cfg_ssh_passphrase,
                                                is_edit_mode,
                                            }
                                        />
                                    </Show>

                                    // Credentials section (non-BigQuery / non-Snowflake-OAuth / non-Databricks-OAuth)
                                    <ProviderCredentialsFields
                                        signals=CredentialsFieldsSignals {
                                            ds_type,
                                            sf_auth_mode,
                                            bq_auth_mode,
                                            db_auth_mode,
                                            synapse_auth_mode,
                                            cred_username,
                                            set_cred_username,
                                            cred_password,
                                            set_cred_password,
                                            cred_password_stored,
                                            cred_access_token,
                                            set_cred_access_token,
                                            cred_private_key,
                                            set_cred_private_key,
                                            cred_sp_client_id,
                                            set_cred_sp_client_id,
                                            cred_sp_client_secret,
                                            set_cred_sp_client_secret,
                                            cfg_shared_credentials,
                                            set_cfg_shared_credentials,
                                            is_admin,
                                        }
                                    />

                                    // Test & Discover button — workspace-admin-only (KYO-184): it
                                    // probes the remote system using the connection-config fields
                                    // above, which non-admins can no longer edit.
                                    // Hidden for BigQuery (uses OAuth), Snowflake OAuth mode,
                                    // Databricks OAuth mode when not yet connected,
                                    // and Synapse Enterprise OAuth when not yet connected.
                                    <Show when=move || {
                                        let t = ds_type.get();
                                        let sf = sf_auth_mode.get();
                                        let db = db_auth_mode.get();
                                        let syn = synapse_auth_mode.get();
                                        let db_oauth_not_ready = t == "databricks" && db == "oauth" && !modal_oauth_connected.get();
                                        let synapse_eo_not_connected = t == "synapse"
                                            && syn == "enterprise_oauth"
                                            && !modal_oauth_connected.get();
                                        is_admin.get()
                                            && !(t == "bigquery"
                                                || (t == "snowflake" && sf == "oauth" && !modal_oauth_connected.get())
                                                || db_oauth_not_ready
                                                || synapse_eo_not_connected)
                                    }>
                                        <div class="border-t border-border pt-4 mt-4">
                                            <div class="flex items-center gap-3">
                                                <button
                                                    type="button"
                                                    class="inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring h-9 px-4 py-2 border border-input bg-background text-foreground shadow-sm hover:bg-secondary hover:text-accent-foreground disabled:pointer-events-none disabled:opacity-50"
                                                    disabled=move || test_action.pending().get()
                                                    on:click=move |_| do_test_and_discover()
                                                >
                                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                                        <Icon icon=phosphor_leptos::PLUG/>
                                                    </span>
                                                    {move || if test_action.pending().get() { "Discovering..." } else { "Test & Discover" }}
                                                </button>
                                                <ConnectionTestResultBadge test_result=test_result success_label="Connected" />
                                            </div>
                                            <p class="text-xs text-muted-foreground mt-2">
                                                "Validate connection and discover available resources"
                                            </p>
                                        </div>
                                    </Show>

                                    // Discovery fields (shown after successful Test & Discover, or
                                    // always in edit mode) — workspace-admin-only (KYO-184): these
                                    // set the catalog scope, which persists through
                                    // `update_datasource_settings`, non-admin-inaccessible.
                                    <Show when=move || {
                                        let t = ds_type.get();
                                        let is_create = is_create_mode.get();
                                        is_admin.get() && t != "bigquery" && (!is_create || discovery_succeeded.get())
                                    }>
                                        <DiscoveryFields
                                            signals=DiscoveryFieldsSignals {
                                                ds_type,
                                                discovery_succeeded,
                                                discovered_databases,
                                                discovered_schemas,
                                                discovered_warehouses,
                                                discovered_catalogs,
                                                cfg_database,
                                                set_cfg_database,
                                                cfg_schema,
                                                set_cfg_schema,
                                                cfg_warehouse,
                                                set_cfg_warehouse,
                                                cfg_catalog,
                                                set_cfg_catalog,
                                                cfg_role,
                                                set_cfg_role,
                                            }
                                        />
                                    </Show>

                                    </div>
                                    </Show>

                                    </div>
                                    </Show>

                                </div>
                            </Show>

                            // ── CATALOG TAB (create mode only) ──
                            <Show when=move || active_tab.get() == "catalog" && is_create_mode.get()>
                                <CreateModeCatalogPicker
                                    datasource_type=Signal::derive(move || ds_type.get())
                                    discovered_databases=discovered_databases
                                    discovered_schemas=discovered_schemas
                                    discovered_catalogs=discovered_catalogs
                                    catalog_selected=create_catalog_selected
                                    set_catalog_selected=set_create_catalog_selected
                                    catalog_text=create_catalog_text
                                    set_catalog_text=set_create_catalog_text
                                    include_public_datasets=create_include_public_datasets
                                    set_include_public_datasets=set_create_include_public_datasets
                                    catalog_discovery_denied=catalog_discovery_denied
                                    bq_projects=bq_projects
                                    bq_projects_loading=bq_projects_loading
                                    bq_projects_error=bq_projects_error
                                    bq_projects_attempted=bq_projects_attempted
                                    bq_auth_mode=bq_auth_mode
                                />
                            </Show>

                            // ── CATALOG TAB (edit mode only) ──
                            // `is_admin` here is defense in depth — the tab button that sets
                            // `active_tab` to "catalog" is itself admin-gated above, so a
                            // non-admin can't normally reach this state, but `active_tab` isn't
                            // reset if admin status changes while the modal is open.
                            <Show when=move || {
                                active_tab.get() == "catalog" && !is_create_mode.get() && is_admin.get()
                            }>
                                <EditModeCatalogTab
                                    datasource_id=Signal::derive(move || {
                                        datasource_id.get().unwrap_or_default()
                                    })
                                    datasource_slug=Signal::derive(move || slug.get())
                                    datasource_type=Signal::derive(move || ds_type.get())
                                    connection_config=Signal::derive(build_connection_config)
                                    credentials=Signal::derive(build_credentials)
                                    is_sample=is_sample
                                    is_connect=is_connect
                                    catalog_selected=catalog_selected
                                    set_catalog_selected=set_catalog_selected
                                    set_catalog_scope_touched=set_catalog_scope_touched
                                    bq_include_public=bq_include_public
                                    set_bq_include_public=set_bq_include_public
                                    use_indexing_credentials=use_indexing_credentials
                                    set_use_indexing_credentials=set_use_indexing_credentials
                                    indexing_creds_type=indexing_creds_type
                                    set_indexing_creds_type=set_indexing_creds_type
                                    indexing_creds_json=indexing_creds_json
                                    set_indexing_creds_json=set_indexing_creds_json
                                    indexing_username=indexing_username
                                    set_indexing_username=set_indexing_username
                                    indexing_password=indexing_password
                                    set_indexing_password=set_indexing_password
                                    indexing_token=indexing_token
                                    set_indexing_token=set_indexing_token
                                    indexing_client_id=indexing_client_id
                                    set_indexing_client_id=set_indexing_client_id
                                    indexing_client_secret=indexing_client_secret
                                    set_indexing_client_secret=set_indexing_client_secret
                                    indexing_tenant_id=indexing_tenant_id
                                    set_indexing_tenant_id=set_indexing_tenant_id
                                    set_indexing_creds_unchanged=set_indexing_creds_unchanged
                                />
                            </Show>

                        </div>
                        </Show>
                    </Show>
                }
            }}
        </Modal>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connect Create Form (create-mode, simplified)
// ─────────────────────────────────────────────────────────────────────────────

/// Simplified form rendered when the create-mode user selects "Kyomi Connect".
///
/// Shows only the fields needed to provision a Connect datasource: name +
/// slug + type (restricted to [`CONNECT_TYPES`]). Submission happens via
/// the modal footer (`do_create_connect`), not via an in-form button, so
/// the user's save gesture is consistent across Direct and Connect modes.
#[component]
fn ConnectCreateForm(
    name: ReadSignal<String>,
    set_name: WriteSignal<String>,
    slug: ReadSignal<String>,
    set_slug: WriteSignal<String>,
    slug_manually_edited: ReadSignal<bool>,
    set_slug_manually_edited: WriteSignal<bool>,
    ds_type: ReadSignal<String>,
    set_ds_type: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <div class="space-y-4 min-h-[400px]">
            // Info panel — matches the React "How Kyomi Connect works" box
            // (same three-step sequence, same copy).
            <div class="rounded-lg border border-border bg-muted/30 p-4">
                <p class="text-sm text-foreground font-medium mb-2">
                    "How Kyomi Connect works"
                </p>
                <ol class="text-sm text-muted-foreground space-y-1 list-decimal list-inside">
                    <li>"Save this datasource to generate a secure token"</li>
                    <li>"Deploy the Kyomi Connect agent in your network"</li>
                    <li>"The agent connects outbound to Kyomi — no inbound access needed"</li>
                </ol>
            </div>

            // Name + Slug
            <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                <div>
                    <label class="block text-sm font-medium mb-1">
                        "Name "
                        <span class="text-error-foreground">"*"</span>
                    </label>
                    <input
                        type="text"
                        class=MODAL_INPUT_CLASS
                        placeholder="Production Database"
                        prop:value=move || name.get()
                        on:input=move |ev| {
                            let new_name = event_target_value(&ev);
                            // Auto-generate slug until the user edits it
                            // manually — same rule the Direct form uses.
                            if !slug_manually_edited.get_untracked() {
                                set_slug.set(generate_slug(&new_name));
                            }
                            set_name.set(new_name);
                        }
                    />
                </div>
                <div>
                    <label class="block text-sm font-medium mb-1">"Slug"</label>
                    <input
                        type="text"
                        class=format!("{} font-mono", MODAL_INPUT_CLASS)
                        placeholder="production-db"
                        prop:value=move || slug.get()
                        on:input=move |ev| {
                            set_slug_manually_edited.set(true);
                            let val = event_target_value(&ev)
                                .to_lowercase()
                                .chars()
                                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                                .collect::<String>();
                            set_slug.set(val);
                        }
                    />
                    <p class="text-xs text-muted-foreground mt-1">
                        "Auto-generated from name if left empty"
                    </p>
                </div>
            </div>

            // Type selector — restricted to Connect-compatible types.
            // Drives the default port baked into the deployment commands
            // on the post-create view.
            <div>
                <label class="block text-sm font-medium mb-1">"Type"</label>
                <Select
                    value=Signal::derive(move || ds_type.get())
                    options=Signal::stored(
                        CONNECT_TYPES
                            .iter()
                            .map(|(v, l)| ((*v).to_string(), (*l).to_string()))
                            .collect::<Vec<_>>()
                    )
                    on_change=move |val: String| set_ds_type.set(val)
                />
                <p class="text-xs text-muted-foreground mt-1">
                    "BigQuery, Snowflake, and Databricks use OAuth and don't need Kyomi Connect."
                </p>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Connect Create Success View (post-create token display + deployment tabs)
// ─────────────────────────────────────────────────────────────────────────────

/// Post-create view rendered after `create_connect_datasource` succeeds.
///
/// Shows a one-time-display warning Alert, the new Connect token in a
/// copy-ready mono block, and the four deployment-command tabs with the
/// real token substituted in. The "Done" button lives in the modal footer
/// and fires `on_saved` to close the modal + refresh the list.
///
/// Reuses [`build_deployment_commands`] (same function powering the
/// edit-mode `ConnectStatusPanel`) so command text stays byte-equivalent
/// across both flows.
#[component]
fn ConnectCreateSuccessView(
    token: Signal<String>,
    datasource_name: Signal<String>,
    datasource_type: Signal<String>,
    active_tab: ReadSignal<String>,
    set_active_tab: WriteSignal<String>,
) -> impl IntoView {
    // Derived: the four deployment commands, rebuilt whenever the token or
    // type changes (both are stable for the lifetime of this view, but the
    // memo keeps us honest if that ever stops being true).
    let deployment_commands: Memo<DeploymentCommands> = Memo::new(move |_| {
        let tok = token.get();
        let ty = datasource_type.get();
        build_deployment_commands(&ty, Some(&tok), None)
    });

    // Displayed port reflects the default for the selected type — matches
    // what the commands embed, so the user can correlate them.
    let displayed_port = Signal::derive(move || default_port(&datasource_type.get()));

    let active_command = move || -> String {
        let tab = active_tab.get();
        deployment_commands.with(|cmds| cmds.for_tab(&tab).to_string())
    };

    view! {
        <div class="space-y-5 min-h-[400px]">
            // Header — datasource name + short framing text.
            <div class="space-y-1">
                <h3 class="text-lg font-semibold text-foreground">
                    "Deploy Kyomi Connect"
                </h3>
                <p class="text-sm text-muted-foreground">
                    "Install the Connect agent to bridge "
                    <span class="font-medium text-foreground">
                        {move || datasource_name.get()}
                    </span>
                    " to Kyomi."
                </p>
            </div>

            // One-time-display warning — same copy as ConnectStatusPanel's
            // post-rotation alert so the "save this token" message reads
            // identically across the creation + rotation flows.
            <Alert variant=AlertVariant::Warning>
                <AlertTitle>"Save this token now"</AlertTitle>
                <AlertDescription>
                    "It will not be shown again. Store it somewhere safe — \
                     you'll need it to deploy the Kyomi Connect agent."
                </AlertDescription>
            </Alert>

            // Token display with copy-to-clipboard.
            <div class="space-y-1.5">
                <label class="block text-sm font-medium text-foreground">
                    "Connect Token"
                </label>
                <div class="flex items-center gap-2 rounded-md border border-border bg-muted/30 px-3 py-2">
                    <code class="flex-1 text-xs font-mono text-foreground break-all select-all">
                        {move || token.get()}
                    </code>
                    <CopyButton text=Signal::derive(move || token.get())/>
                </div>
            </div>

            // Deployment tabs — same module powers ConnectStatusPanel.
            <div class="space-y-2">
                <div class="flex items-center justify-between">
                    <h4 class="text-sm font-medium text-foreground">"Deployment Instructions"</h4>
                    <span class="text-xs text-muted-foreground font-mono">
                        {move || format!("default port: {}", displayed_port.get())}
                    </span>
                </div>
                <DeploymentTabStrip
                    active_tab=active_tab.into()
                    set_active_tab=set_active_tab
                />
                <div class="relative rounded-md border border-border bg-muted/30">
                    <pre class="p-4 pr-12 text-xs font-mono text-foreground overflow-x-auto whitespace-pre">
                        {active_command}
                    </pre>
                    <div class="absolute top-2 right-2">
                        <CopyButton text=Signal::derive(move || {
                            let tab = active_tab.get();
                            deployment_commands.with(|cmds| cmds.for_tab(&tab).to_string())
                        })/>
                    </div>
                </div>
            </div>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Modal OAuth Status Panel (shared 4-state UI)
// ─────────────────────────────────────────────────────────────────────────────

/// The popup-monitor cleanup slot `ModalOAuthStatusPanel` stashes its
/// in-flight `monitor_oauth_popup` cleanup in (KYO-437). Named alias for
/// `clippy::type_complexity`, not a functional requirement.
#[cfg(target_arch = "wasm32")]
type PopupMonitorCleanupSlot = StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>>;

/// The four OAuth states rendered as a reactive panel inside the edit modal.
///
/// | State          | Condition                                      |
/// |----------------|------------------------------------------------|
/// | Not configured | `cfg_missing` is true (admin has not set up OAuth) |
/// | Connected      | `oauth_connected` is true                       |
/// | Expired        | `oauth_expired` is true                         |
/// | Not connected  | none of the above                               |
#[component]
fn ModalOAuthStatusPanel(
    /// Whether the OAuth credential is currently connected.
    oauth_connected: ReadSignal<bool>,
    /// Authenticated account email, if connected.
    oauth_email: ReadSignal<Option<String>>,
    /// Whether the token has expired (connected but needs re-auth).
    oauth_expired: ReadSignal<bool>,
    /// Whether a popup window is currently open (connecting…).
    oauth_connecting: ReadSignal<bool>,
    /// Setter for the connecting state (popup opener writes this).
    set_oauth_connecting: WriteSignal<bool>,
    /// Human-readable provider name (e.g. "Google", "Snowflake").
    provider_name: &'static str,
    /// URL to open as an OAuth popup window.
    connect_url: Signal<String>,
    /// Whether OAuth credentials are missing at the admin level.
    /// When true, shows the "not configured" warning instead of the
    /// connect button. Always false for `kyomi_oauth` (global OAuth,
    /// not per-datasource).
    cfg_missing: Signal<bool>,
    /// Whether the Connect/Reconnect action is blocked by an unmet
    /// precondition (KYO-427, corrected KYO-477). Defaults to `false` —
    /// never blocked — so Snowflake, Databricks, Microsoft, and BigQuery
    /// enterprise_oauth (the other four callers of this shared panel) are
    /// unaffected. The one real caller, BigQuery kyomi_oauth, passes
    /// `bq_kyomi_oauth_connect_ok` — the Connect-only gate, deliberately
    /// NOT the same signal the footer's Save/Create gate reads
    /// (`bq_kyomi_oauth_access_ok`). Folding Save/Create's account-level
    /// `oauth_connected` allowance into this prop is the exact KYO-477
    /// defect: see `bq_kyomi_oauth_connect_allowed`'s doc comment
    /// (`pages/settings/datasources.rs`) for why the two must stay
    /// separate predicates rather than a KYO-423-style shared copy.
    #[prop(default = false.into())]
    connect_blocked: Signal<bool>,
    /// Called when the user clicks "Disconnect".  Callers use an
    /// `Action` and pass a typed callback — this is a simple `Fn`
    /// because `Action` dispatch is synchronous and non-blocking.
    on_disconnect: Callback<()>,
    /// Whether the disconnect action is currently pending.
    disconnect_pending: Signal<bool>,
    /// Called when the popup-monitor (KYO-437) detects a connect attempt
    /// resolved without an OAuth `postMessage` ever arriving — the popup
    /// was closed, or the attempt timed out. Built by
    /// `build_oauth_recovery_callback` in the owning `*AuthModeSection`,
    /// which knows which `OAuthStatusSource` and slug to re-check status
    /// against for this provider/mode before reporting anything as failed.
    on_recover: Callback<PopupMonitorOutcome>,
) -> impl IntoView {
    let provider = provider_name;

    // On native (non-WASM) targets, connect_url is only referenced inside
    // #[cfg(target_arch = "wasm32")] blocks so the compiler considers it
    // unused. Consume it here to suppress the warning without a lint annotation.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = connect_url;

    // `on_recover` is only ever invoked from inside the WASM-only popup
    // monitor below — same reasoning as `connect_url` above.
    #[cfg(not(target_arch = "wasm32"))]
    let _ = on_recover;

    // Holds the cleanup returned by `monitor_oauth_popup` for whichever
    // connect attempt is currently in flight (KYO-437), so this component's
    // teardown can stop the popup-closed poll and connect timeout
    // immediately rather than waiting for the next tick — mirrors the
    // `StoredValue<Option<SendWrapper<_>>>` pattern used for other
    // `gloo_timers` handles in this codebase (`agent_thinking.rs`,
    // `search_sort_bar.rs`), except the payload here is the monitor's own
    // `FnOnce` cleanup rather than a raw timer handle, so it must be
    // *called*, not merely dropped — see the `on_cleanup` below.
    #[cfg(target_arch = "wasm32")]
    let popup_monitor: PopupMonitorCleanupSlot = StoredValue::new(None);
    #[cfg(target_arch = "wasm32")]
    on_cleanup(move || {
        popup_monitor.update_value(|slot| {
            if let Some(cleanup) = slot.take() {
                cleanup.take()();
            }
        });
    });

    // Starts (or restarts) a connect attempt: opens the popup and, on
    // success, arms the popup-closed poll + connect timeout (KYO-437).
    // Shared by both the "Connect" and "Reconnect" buttons below — they are
    // otherwise byte-for-byte the same click handler, differing only in
    // which state they're rendered from.
    let start_connect = move |_: leptos::ev::MouseEvent| {
        if oauth_connecting.get_untracked() || connect_blocked.get_untracked() {
            return;
        }
        set_oauth_connecting.set(true);
        #[cfg(target_arch = "wasm32")]
        {
            let connect_url_val = connect_url.get_untracked();
            use crate::utils::oauth_popup::{monitor_oauth_popup, open_oauth_popup};
            match open_oauth_popup(&connect_url_val, provider) {
                Some(popup) => {
                    let cleanup = monitor_oauth_popup(
                        popup,
                        // `try_get_untracked` — this runs inside a deferred
                        // `gloo_timers` callback, not a reactive scope; the
                        // signal may already be disposed if this panel
                        // unmounted (see the disposal-safety standard).
                        move || oauth_connecting.try_get_untracked().unwrap_or(false),
                        move |outcome| {
                            on_recover.try_run(outcome);
                        },
                    );
                    popup_monitor.update_value(|slot| {
                        *slot = Some(send_wrapper::SendWrapper::new(
                            Box::new(cleanup) as Box<dyn FnOnce()>,
                        ));
                    });
                }
                None => {
                    set_oauth_connecting.set(false);
                    toast_error("Popup was blocked. Please allow popups for this site.");
                }
            }
        }
    };

    view! {
        {move || {
            if cfg_missing.get() {
                // Not configured: admin has not set up OAuth credentials.
                return view! {
                    <div class="flex items-start gap-2 p-3 rounded-lg border border-warning-border bg-warning">
                        <span class="shrink-0 mt-0.5 text-warning-foreground">
                            <Icon icon=phosphor_leptos::WARNING size="16px"/>
                        </span>
                        <p class="text-sm text-warning-foreground">
                            "OAuth credentials not configured. Ask your admin to configure OAuth Client ID and Secret."
                        </p>
                    </div>
                }.into_any();
            }

            if oauth_connected.get() && !oauth_expired.get() {
                // Connected state.
                let email_text = oauth_email.get()
                    .unwrap_or_else(|| format!("{} account", provider));
                return view! {
                    <div class="flex items-center justify-between p-3 bg-success border border-success-border rounded-lg">
                        <div class="flex items-center gap-2">
                            <span class="shrink-0 text-success-foreground">
                                <Icon icon=phosphor_leptos::CHECK_CIRCLE size="16px"/>
                            </span>
                            <span class="text-sm text-success-foreground">{email_text}</span>
                        </div>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=Signal::derive(move || disconnect_pending.get())
                            on:click=move |_| {
                                if !disconnect_pending.get_untracked() {
                                    on_disconnect.run(());
                                }
                            }
                        >
                            {move || if disconnect_pending.get() {
                                view! {
                                    <span class="flex items-center gap-1.5">
                                        <Spinner size="h-3 w-3"/>
                                        "Disconnecting..."
                                    </span>
                                }.into_any()
                            } else {
                                view! { <span>"Disconnect"</span> }.into_any()
                            }}
                        </Button>
                    </div>
                }.into_any();
            }

            if oauth_expired.get() {
                // Expired state.
                let provider_name = provider;
                return view! {
                    <div class="space-y-2">
                        <div class="flex items-start gap-2 p-3 rounded-lg border border-warning-border bg-warning">
                            <span class="shrink-0 mt-0.5 text-warning-foreground">
                                <Icon icon=phosphor_leptos::WARNING size="16px"/>
                            </span>
                            <p class="text-sm text-warning-foreground">
                                {format!("Your {} connection has expired. Please reconnect.", provider_name)}
                            </p>
                        </div>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=connect_blocked
                            on:click=start_connect
                        >
                            {move || if oauth_connecting.get() {
                                view! {
                                    <span class="flex items-center gap-1.5">
                                        <Spinner size="h-3 w-3"/>
                                        "Connecting..."
                                    </span>
                                }.into_any()
                            } else {
                                view! {
                                    <span class="flex items-center gap-1.5">
                                        <Icon icon=phosphor_leptos::PLUG size="14px"/>
                                        {format!("Reconnect {}", provider_name)}
                                    </span>
                                }.into_any()
                            }}
                        </Button>
                    </div>
                }.into_any();
            }

            // Not connected state.
            let provider_name = provider;
            view! {
                <div class="space-y-2">
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        disabled=connect_blocked
                        on:click=start_connect
                    >
                        {move || if oauth_connecting.get() {
                            view! {
                                <span class="flex items-center gap-1.5">
                                    <Spinner size="h-3 w-3"/>
                                    "Connecting..."
                                </span>
                            }.into_any()
                        } else {
                            view! {
                                <span class="flex items-center gap-1.5">
                                    <Icon icon=phosphor_leptos::PLUG size="14px"/>
                                    {format!("Connect {}", provider_name)}
                                </span>
                            }.into_any()
                        }}
                    </Button>
                </div>
            }.into_any()
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OAuth Status Re-fetch Hook
// ─────────────────────────────────────────────────────────────────────────────

/// Which OAuth status endpoint backs a given auth mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OAuthStatusSource {
    /// Account-level Google OAuth — `get_google_oauth_status()`.
    GoogleAccount,
    /// Per-datasource OAuth for this provider key — `get_datasource_oauth_status(key, slug)`.
    Datasource(&'static str),
}

/// The three OAuth status setters an `*AuthModeSection` component owns,
/// packed as a single struct so [`use_oauth_status_refetch`] doesn't take a
/// pile of positional `WriteSignal` arguments.
#[derive(Clone, Copy)]
struct OAuthStatusSetters {
    connected: WriteSignal<bool>,
    email: WriteSignal<Option<String>>,
    expired: WriteSignal<bool>,
}

/// Resolves which OAuth status source (if any) should be (re)fetched for
/// the current auth mode, together with the slug it should be fetched
/// with. This is the single implementation of the KYO-411/KYO-426/KYO-443
/// guard logic, shared by both callers that need it:
/// `use_oauth_status_refetch`'s mode-change `Memo` and
/// `build_oauth_recovery_callback`'s post-popup recheck. Neither inlines
/// its own copy of these rules — an inlined copy is exactly how KYO-13,
/// KYO-17 and KYO-197 recurred (see `docs/CODING_STANDARDS.md` §Leptos).
///
/// `read_slug` is a lazy accessor rather than an already-read `&str` so
/// that going through one shared function doesn't cost either caller its
/// own reactive-tracking property: this predicate only calls `read_slug`
/// on the edit-mode `Datasource(_)` branch below, so
/// `use_oauth_status_refetch`'s `Memo` — which passes `move || slug.get()`
/// — subscribes to `slug` only when that branch is actually taken, which is
/// what stops it re-running (and flashing the panel to disconnected) on
/// every keystroke in the Name field while some other source is selected
/// (KYO-443; see the two `..._never_calls_read_slug` tests below, which
/// pin this directly against this function using a closure that panics if
/// called, rather than by scraping source text). `build_oauth_recovery_callback`
/// passes `move || slug.get_untracked()` — already untracked, so routing
/// through this function doesn't change when it reads `slug`.
///
/// `GoogleAccount` is an account-level fetch (`get_google_oauth_status()`)
/// that takes no slug at all and exists independently of any particular
/// datasource, so it must run even in create mode and even when `slug` is
/// empty — notably BigQuery kyomi_oauth, where otherwise an already-linked
/// Google account is never detected while creating a new datasource
/// (KYO-411). Its arm never calls `read_slug`.
///
/// `Datasource(_)` sources need TWO things `GoogleAccount` doesn't:
///   - `!is_create_mode`: `get_datasource_oauth_status(provider_key, slug)`
///     looks up a datasource that must already exist server-side. In create
///     mode it never does yet, so calling this fetch 500s (KYO-426) — the
///     empty-slug check below does NOT catch this, because `slug`
///     auto-generates from the Name field the moment the user types
///     anything, so it stops being empty long before the datasource is
///     actually created. This is checked in the match guard, before
///     `read_slug` is called, so create mode never reads the slug either.
///   - a non-empty resolved slug: a genuine precondition of the fetch call
///     itself (`get_datasource_oauth_status` needs a real slug to look up),
///     independent of create/edit mode.
///
/// Returns the resolved slug alongside the source so a caller that read it
/// lazily above doesn't have to read it a second time to perform the fetch.
fn oauth_status_source_to_fetch(
    current_mode: &str,
    is_create_mode: bool,
    read_slug: impl FnOnce() -> String,
    source_for_mode: fn(&str) -> Option<OAuthStatusSource>,
) -> Option<(OAuthStatusSource, String)> {
    match source_for_mode(current_mode) {
        Some(OAuthStatusSource::GoogleAccount) => {
            Some((OAuthStatusSource::GoogleAccount, String::new()))
        }
        Some(OAuthStatusSource::Datasource(key)) if !is_create_mode => {
            let slug_val = read_slug();
            (!slug_val.is_empty()).then_some((OAuthStatusSource::Datasource(key), slug_val))
        }
        _ => None,
    }
}

/// Result of a one-shot OAuth status fetch, normalized across the two
/// underlying endpoints (account-level Google OAuth vs. per-datasource
/// provider OAuth) so callers don't need to know which one answered.
struct OAuthStatusFetchResult {
    connected: bool,
    email: Option<String>,
    expired: bool,
}

/// Fetches OAuth status for `source` — the single place that knows which
/// server fn backs [`OAuthStatusSource::GoogleAccount`] vs.
/// [`OAuthStatusSource::Datasource`]. Shared by [`use_oauth_status_refetch`]
/// (the mode-change refetch) and the popup-recovery callback built by
/// [`build_oauth_recovery_callback`] (KYO-437) — without this extraction the
/// recovery path would need its own copy of this match, which is exactly
/// the kind of duplication that turned one Effect body into three separate
/// copies before `use_oauth_status_refetch` itself existed (KYO-13 /
/// KYO-17 / KYO-197, see `docs/CODING_STANDARDS.md`).
///
/// Returns `None` on a fetch error — logged via `warn!`, not silently
/// discarded — so callers know to leave the previous status alone rather
/// than treat a network error as "disconnected".
async fn fetch_oauth_status_once(
    source: OAuthStatusSource,
    slug_val: String,
) -> Option<OAuthStatusFetchResult> {
    match source {
        OAuthStatusSource::GoogleAccount => get_google_oauth_status()
            .await
            .map_err(|e| leptos::logging::warn!("Google OAuth status fetch failed: {e}"))
            .ok()
            .map(|s| OAuthStatusFetchResult {
                connected: s.connected,
                email: s.google_email,
                expired: s.token_expired,
            }),
        OAuthStatusSource::Datasource(provider_key) => {
            get_datasource_oauth_status(provider_key.to_string(), slug_val)
                .await
                .map_err(|e| leptos::logging::warn!("{provider_key} OAuth status fetch failed: {e}"))
                .ok()
                .map(|s| OAuthStatusFetchResult {
                    connected: s.connected,
                    email: s.provider_email,
                    expired: s.token_expired,
                })
        }
    }
}

/// Re-fetches OAuth connection status whenever the auth mode changes in an
/// open datasource modal.
///
/// Without this, the status panel is fetched once when the modal opens
/// (using whichever auth mode was loaded from the server) and never again —
/// so switching the mode selector leaves the panel showing the previous
/// mode's stale connected/email/expired state. This exact defect was
/// independently flagged in KYO-13 (BigQuery) and KYO-17 (Databricks) and
/// documented in `docs/CODING_STANDARDS.md` §Leptos as a required
/// `Effect::new` re-fetch — and it recurred a third time in Synapse anyway,
/// because each fix copy-pasted the Effect instead of sharing it (KYO-197).
/// Any new provider's OAuth status panel must call this hook rather than
/// hand-rolling another copy of the Effect.
///
/// The `fetch_input` `Memo` below resolves its source and slug via the
/// shared `oauth_status_source_to_fetch` predicate, passing `slug` in as a
/// lazy `move || slug.get()` accessor rather than reading it upfront — a
/// reactive closure subscribes only to the signals it actually reads on a
/// given run, and `oauth_status_source_to_fetch` only calls the accessor on
/// its edit-mode `Datasource(_)` branch (see that function's doc comment
/// for the full branch-by-branch account). So a run that resolves
/// `GoogleAccount`, or a create-mode `Datasource(_)`, never calls the
/// accessor and this `Memo` is never subscribed to `slug` on that run.
/// Before this fix, `slug.get()` was read unconditionally at the top of
/// the `Effect` this `Memo` replaced, so BigQuery kyomi_oauth re-ran, and
/// therefore reset the panel to disconnected, on every keystroke in the
/// Name field (KYO-443) — even though that keystroke was driving `slug`'s
/// auto-generated value, not anything BigQuery kyomi_oauth's own fetch
/// (`get_google_oauth_status`, which takes no slug) needed. `Memo` also
/// dedupes by `PartialEq`, so even a run that resolves the same branch
/// again only notifies the `Effect` below when the resolved `(source,
/// slug)` pair actually changed — so `setters.connected.set(false)` no
/// longer fires on every keystroke either.
fn use_oauth_status_refetch(
    auth_mode: ReadSignal<String>,
    slug: ReadSignal<String>,
    is_create_mode: Signal<bool>,
    setters: OAuthStatusSetters,
    source_for_mode: fn(&str) -> Option<OAuthStatusSource>,
) {
    // See the doc comment above, and oauth_status_source_to_fetch's own
    // doc comment, for why `slug` is passed in as a lazy accessor rather
    // than read upfront (KYO-443).
    let fetch_input = Memo::new(move |_| {
        let current_mode = auth_mode.get(); // subscribe to mode changes
        let create_mode = is_create_mode.get();
        oauth_status_source_to_fetch(&current_mode, create_mode, move || slug.get(), source_for_mode)
    });

    Effect::new(move |_| {
        let Some((source, slug_val)) = fetch_input.get() else {
            return;
        };
        // Reset to disconnected state while the fetch is in flight.
        setters.connected.set(false);
        setters.email.set(None);
        setters.expired.set(false);

        leptos::task::spawn_local(async move {
            if let Some(status) = fetch_oauth_status_once(source, slug_val).await {
                setters.connected.try_set(status.connected);
                setters.email.try_set(status.email);
                setters.expired.try_set(status.expired);
            }
        });
    });
}

/// Builds the `on_recover` callback every `ModalOAuthStatusPanel` in a given
/// `*AuthModeSection` is wired to (KYO-437).
///
/// Fired by the shared popup monitor (`oauth_popup::monitor_oauth_popup`)
/// when a connect attempt resolves *without* an OAuth `postMessage` ever
/// arriving — the popup was closed, or the attempt timed out. Before
/// reporting anything as failed, this re-runs the exact status fetch
/// `use_oauth_status_refetch` uses (via `fetch_oauth_status_once`): the
/// account may have been linked server-side even though the notification
/// was lost (KYO-436) — the whole point of this ticket is turning that dead
/// end into a working recovery instead of a permanent spinner. If the
/// recheck finds a connection, the recovered state is adopted exactly as
/// the `postMessage` success handler would have set it and nothing is
/// shown; otherwise `oauth_connecting` is cleared and a toast explains what
/// happened, with wording (`popup_monitor_outcome_message`) that
/// distinguishes an intentional cancel from a timeout.
///
/// Built once per section — not once per `ModalOAuthStatusPanel` instance —
/// because it re-resolves the *current* mode's `OAuthStatusSource`
/// dynamically via `oauth_status_source_to_fetch` when it fires, the same
/// way `use_oauth_status_refetch` does on every mode change. That lets one
/// callback correctly serve both of BigQuery's panels (`kyomi_oauth` and
/// `enterprise_oauth`) without needing to know at build time which one is
/// currently visible.
///
/// Takes `is_create_mode` for the same reason `use_oauth_status_refetch`
/// does (KYO-426): a `Datasource(_)` source's recheck calls
/// `get_datasource_oauth_status(provider_key, slug)`, which 500s against a
/// datasource that doesn't exist yet. This callback fires after a connect
/// attempt, and a connect attempt is reachable in create mode (e.g.
/// BigQuery `enterprise_oauth`), so without this guard a popup that closed
/// early in create mode would trigger the exact same fetch-against-a
/// -nonexistent-datasource error `oauth_status_source_to_fetch` guards
/// against in the mode-change path too. `auth_mode`/`is_create_mode`/`slug`
/// are all read with `get_untracked()` here — this callback runs once, on a
/// discrete event, not inside a reactive scope — including inside the
/// `move || slug.get_untracked()` accessor passed to
/// `oauth_status_source_to_fetch`, so routing the slug read through that
/// shared function changes nothing about when this callback reads `slug`.
fn build_oauth_recovery_callback(
    auth_mode: ReadSignal<String>,
    slug: ReadSignal<String>,
    is_create_mode: Signal<bool>,
    setters: OAuthStatusSetters,
    set_oauth_connecting: WriteSignal<bool>,
    source_for_mode: fn(&str) -> Option<OAuthStatusSource>,
    provider_name: &'static str,
) -> Callback<PopupMonitorOutcome> {
    Callback::new(move |outcome: PopupMonitorOutcome| {
        let current_mode = auth_mode.get_untracked();
        let create_mode = is_create_mode.get_untracked();
        let resolved = oauth_status_source_to_fetch(
            &current_mode,
            create_mode,
            move || slug.get_untracked(),
            source_for_mode,
        );

        leptos::task::spawn_local(async move {
            let recovered = match resolved {
                Some((source, slug_val)) => fetch_oauth_status_once(source, slug_val).await,
                None => None,
            };
            match recovered {
                Some(status) if status.connected => {
                    setters.connected.try_set(true);
                    setters.email.try_set(status.email);
                    setters.expired.try_set(status.expired);
                }
                _ => {
                    toast_error(popup_monitor_outcome_message(provider_name, outcome));
                }
            }
            set_oauth_connecting.try_set(false);
        });
    })
}

/// Maps BigQuery's auth mode to its OAuth status source. `service_account`
/// mode has no OAuth status to fetch.
fn bigquery_oauth_source(mode: &str) -> Option<OAuthStatusSource> {
    match mode {
        "kyomi_oauth" => Some(OAuthStatusSource::GoogleAccount),
        "enterprise_oauth" => Some(OAuthStatusSource::Datasource("bigquery-enterprise")),
        _ => None,
    }
}

/// Maps Snowflake's auth mode to its OAuth status source. `password` and
/// `keypair` modes have no OAuth status to fetch.
///
/// Deliberate behaviour change from the hand-rolled Effect this replaces:
/// the old guard was negative (`mode == "password" || mode == "keypair"`
/// bails), so an *unrecognised* mode fell through and fetched anyway. This
/// allow-list makes an unrecognised mode a no-op instead, matching what
/// Databricks already does. The selector only ever offers
/// `password`/`oauth`/`keypair`, so this is unreachable in practice.
fn snowflake_oauth_source(mode: &str) -> Option<OAuthStatusSource> {
    match mode {
        "oauth" => Some(OAuthStatusSource::Datasource("snowflake")),
        _ => None,
    }
}

/// Maps Databricks's auth mode to its OAuth status source. `token` mode has
/// no OAuth status to fetch.
fn databricks_oauth_source(mode: &str) -> Option<OAuthStatusSource> {
    match mode {
        "oauth" => Some(OAuthStatusSource::Datasource("databricks")),
        _ => None,
    }
}

/// Maps Synapse's auth mode to its OAuth status source. `sql` and
/// `service_principal` modes have no OAuth status to fetch.
fn synapse_oauth_source(mode: &str) -> Option<OAuthStatusSource> {
    match mode {
        "enterprise_oauth" => Some(OAuthStatusSource::Datasource("microsoft-enterprise")),
        _ => None,
    }
}

/// Dispatches to the provider-specific `*_oauth_source` function for
/// `ds_type`, so any caller that needs "does this (ds_type, auth_mode)
/// pair have an OAuth status behind it, and which kind" reads the same
/// mapping every `*AuthModeSection` already wires into
/// `use_oauth_status_refetch`, rather than hand-rolling a second,
/// independent (ds_type, auth_mode) → [`OAuthStatusSource`] mapping that
/// could silently drift from it. Added for KYO-517 —
/// [`connection_step_satisfied_from`] is its only caller today.
fn oauth_source_for_ds_type(ds_type: &str, auth_mode: &str) -> Option<OAuthStatusSource> {
    match ds_type {
        "bigquery" => bigquery_oauth_source(auth_mode),
        "snowflake" => snowflake_oauth_source(auth_mode),
        "databricks" => databricks_oauth_source(auth_mode),
        "synapse" => synapse_oauth_source(auth_mode),
        _ => None,
    }
}

/// Whether an admin-configured OAuth Client ID/Secret pair is missing —
/// i.e. an admin has not (yet, or not fully) configured OAuth for this
/// provider, and a member's "Connect" button would start a flow that
/// cannot succeed.
///
/// True when *either* field is empty, not only when both are. A
/// half-filled config — exactly what an interrupted admin leaves behind —
/// must still trip this, because the React predicate this ports
/// (`OAuthConnect.jsx`'s `configFields.every(f => !!connectionConfig[f.name])`,
/// negated) requires ALL fields present to consider OAuth configured, so
/// "missing" is true the moment ANY one is empty. Every config-bearing
/// OAuth surface (BigQuery `enterprise_oauth`, Snowflake, Databricks,
/// Synapse) must derive its `cfg_missing` signal from this function
/// rather than re-deriving the emptiness check inline — see
/// `docs/standards/code-organization/propagate-predicate-changes-to-every-copy.md`.
///
/// The one deliberate exception is BigQuery's `kyomi_oauth` mode, which
/// is account-level, globally-hosted Kyomi OAuth with no `configFields`
/// at all — there is nothing for this function to check — so it keeps
/// `cfg_missing=Signal::stored(false)` at its call site instead of
/// calling this function. (KYO-519)
fn oauth_config_missing(client_id: &str, client_secret: &str) -> bool {
    client_id.is_empty() || client_secret.is_empty()
}

// ─────────────────────────────────────────────────────────────────────────────
// Connection Test Result Badge
// ─────────────────────────────────────────────────────────────────────────────

/// Renders the outcome of the last Test & Discover / Validate call.
///
/// Success renders a check icon + `success_label` — unchanged from what both
/// call sites rendered inline before this extraction. Failure renders an X
/// icon, "Failed", and the server's sanitized failure reason
/// (`TestConnectionResult::message`) beneath it, so the specific,
/// user-fixable cause (malformed JSON, wrong project_id, disabled key,
/// BigQuery API not enabled, missing IAM role, revoked key, ...) is visible
/// instead of discarded.
///
/// Shared by the generic Test & Discover button (`success_label = "Connected"`)
/// and the BigQuery service_account mode's "Validate & Discover Projects"
/// button (`success_label = "Valid"`) — before this component existed, both
/// sites hardcoded an identical "Failed" arm that read only
/// `TestConnectionResult::success`, never `::message`, which is why a
/// BigQuery service-account failure rendered the two-syllable word "Failed"
/// no matter which of six distinct, user-fixable causes produced it
/// (KYO-469).
#[component]
fn ConnectionTestResultBadge(
    test_result: ReadSignal<Option<TestConnectionResult>>,
    success_label: &'static str,
) -> impl IntoView {
    move || {
        test_result.get().map(|r| {
            if r.success {
                view! {
                    <div class="flex items-center gap-2 text-sm text-success-foreground">
                        <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4"/>
                        {success_label}
                    </div>
                }.into_any()
            } else {
                view! {
                    <div>
                        <div class="flex items-center gap-2 text-sm text-error-foreground">
                            <Icon icon=phosphor_leptos::X attr:class="h-4 w-4"/>
                            "Failed"
                        </div>
                        <p class="text-xs text-error-foreground mt-2">{r.message.clone()}</p>
                    </div>
                }.into_any()
            }
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// BigQuery Auth Mode Section
// ─────────────────────────────────────────────────────────────────────────────

/// Sentinel option value that swaps a [`BqProjectField`] into custom-entry
/// mode. Never written into the field's `value` signal — the `on_change`
/// handler below intercepts it before that can happen.
const BQ_CUSTOM_PROJECT_OPTION: &str = "__custom__";

/// Appends the "Enter custom project ID..." sentinel option
/// (`BQ_CUSTOM_PROJECT_OPTION`) to a discovered project list, producing the
/// options `[Select]` renders in [`BqProjectField`]'s dropdown branch.
///
/// Pure and free of the view tree so KYO-406's actual bug — the dropdown
/// offering no way to enter a project the discovery API didn't return, once
/// *any* project was discovered — can be asserted by value (the sentinel is
/// really present in the option list `Select` receives) rather than merely
/// asserting that `BQ_CUSTOM_PROJECT_OPTION` appears somewhere in the
/// source.
fn bq_project_select_options(projects: Vec<(String, String)>) -> Vec<(String, String)> {
    let mut opts = projects;
    opts.push((
        BQ_CUSTOM_PROJECT_OPTION.to_string(),
        "Enter custom project ID...".to_string(),
    ));
    opts
}

/// A single BigQuery project field with three mutually exclusive render
/// states:
///
/// 1. **No projects discovered** (and not loading): a plain text input —
///    there is nothing to pick from, so there is no dropdown to offer.
/// 2. **Projects discovered, dropdown mode**: a [`Select`] whose option list
///    carries a trailing "Enter custom project ID..." sentinel
///    (`BQ_CUSTOM_PROJECT_OPTION`). `Select` has no separator/heterogeneous-
///    item support, so the sentinel renders as an ordinary trailing option
///    rather than forking a new shared primitive for one caller.
/// 3. **Projects discovered, custom-entry mode**: a text input plus a "Back
///    to dropdown" affordance. Reached by picking the sentinel; left by the
///    button. `is_custom` is a signal local to this component instance, so
///    each `BqProjectField` toggles independently of any other rendered
///    elsewhere (KYO-406 AC).
///
/// Across both dropdown and custom-entry states the field shares the same
/// `value`/`set_value` signal, so switching modes never loses or resets
/// whatever project id was already entered.
///
/// Using `<Show>` here (instead of `{move || ...}` branching) keeps a stable
/// component tree and avoids the disposal panic that occurs when an `Effect`
/// inside `Select` fires after the surrounding closure's reactive scope is
/// torn down during a branch swap. The three states above are still only two
/// levels of `<Show>` nesting — outer (has-projects vs not), inner
/// (dropdown vs custom-entry) — never a `{move || match ... }` branch.
#[component]
fn BqProjectField(
    label: &'static str,
    value: ReadSignal<String>,
    set_value: WriteSignal<String>,
    bq_projects: ReadSignal<Vec<(String, String)>>,
    bq_projects_loading: ReadSignal<bool>,
) -> impl IntoView {
    let (is_custom, set_is_custom) = signal(false);

    let select_options = Signal::derive(move || bq_project_select_options(bq_projects.get()));

    view! {
        <div>
            <label class="block text-sm font-medium mb-1">{label}</label>
            <Show
                when=move || bq_projects_loading.get() || !bq_projects.get().is_empty()
                fallback=move || view! {
                    <input
                        type="text"
                        class=MODAL_INPUT_CLASS
                        placeholder="my-gcp-project"
                        prop:value=move || value.get()
                        on:input=move |ev| set_value.set(event_target_value(&ev))
                    />
                    <p class="text-xs text-muted-foreground mt-1">
                        "Connect to discover projects, or enter project ID manually"
                    </p>
                }
            >
                <p class="text-xs text-muted-foreground mb-1">
                    "Select from discovered projects or enter a custom ID"
                </p>
                <Show
                    when=move || is_custom.get()
                    fallback=move || view! {
                        <Select
                            value=Signal::derive(move || value.get())
                            options=select_options
                            on_change=move |val| {
                                if val == BQ_CUSTOM_PROJECT_OPTION {
                                    set_is_custom.set(true);
                                } else {
                                    set_value.set(val);
                                }
                            }
                            placeholder="Select a project".to_string()
                            disabled=Signal::derive(move || bq_projects_loading.get())
                        />
                    }
                >
                    <input
                        type="text"
                        class=MODAL_INPUT_CLASS
                        placeholder="my-gcp-project"
                        prop:value=move || value.get()
                        on:input=move |ev| set_value.set(event_target_value(&ev))
                    />
                    <button
                        type="button"
                        class="text-xs text-primary hover:underline mt-1"
                        on:click=move |_| set_is_custom.set(false)
                    >
                        "Back to dropdown"
                    </button>
                </Show>
            </Show>
        </div>
    }
}

#[component]
fn BigQueryAuthModeSection(
    bq_auth_mode: ReadSignal<String>,
    set_bq_auth_mode: WriteSignal<String>,
    cfg_oauth_client_id: ReadSignal<String>,
    set_cfg_oauth_client_id: WriteSignal<String>,
    cfg_oauth_client_secret: ReadSignal<String>,
    set_cfg_oauth_client_secret: WriteSignal<String>,
    cfg_service_account_json: ReadSignal<String>,
    set_cfg_service_account_json: WriteSignal<String>,
    service_account_email: ReadSignal<String>,
    set_service_account_email: WriteSignal<String>,
    slug: ReadSignal<String>,
    cred_billing_project: ReadSignal<String>,
    set_cred_billing_project: WriteSignal<String>,
    /// Whether the OAuth account is currently connected.
    oauth_connected: ReadSignal<bool>,
    /// Setter for the connected state (used by re-fetch Effect on mode change).
    set_oauth_connected: WriteSignal<bool>,
    /// The connected account email, if any.
    oauth_email: ReadSignal<Option<String>>,
    /// Setter for the email state (used by re-fetch Effect on mode change).
    set_oauth_email: WriteSignal<Option<String>>,
    /// Whether the OAuth token has expired.
    oauth_expired: ReadSignal<bool>,
    /// Setter for the expired state (used by re-fetch Effect on mode change).
    set_oauth_expired: WriteSignal<bool>,
    /// Whether an OAuth popup is currently in progress.
    oauth_connecting: ReadSignal<bool>,
    /// Setter for the connecting state.
    set_oauth_connecting: WriteSignal<bool>,
    /// Action to disconnect the Kyomi/Google OAuth account.
    google_disconnect_action: Action<(), Result<crate::server_fns::datasource_oauth::GoogleOAuthDisconnectResult, ServerFnError>>,
    /// Action to disconnect a per-datasource OAuth account.
    datasource_disconnect_action: Action<(String, String), Result<crate::server_fns::datasource_oauth::DatasourceOAuthDisconnectResult, ServerFnError>>,
    /// True in create mode — OAuth status panel is hidden in create mode.
    is_create_mode: Signal<bool>,
    /// GCP project list fetched after OAuth connects.  Empty until OAuth is
    /// connected; drives the billing project Select dropdown.
    bq_projects: ReadSignal<Vec<(String, String)>>,
    /// Setter for `bq_projects`. The service-account "Remove" chip and the
    /// Authentication Mode selector's `on_change` are teardown routes that
    /// aren't `Action`s (see `set_test_result` above), so they need their
    /// own way to clear the discovered project list — the same list
    /// `google_disconnect_action`'s and `datasource_disconnect_action`'s
    /// Effects clear in the parent, and that `do_test_and_discover` clears
    /// before every fresh validate. Passed through to
    /// `try_reset_bq_projects_signals` (KYO-468) alongside
    /// `set_bq_projects_error`/`set_bq_projects_attempted` below rather than
    /// written individually — `try_set` for the same parent/child boundary
    /// reason as `set_test_result` (KYO-413).
    set_bq_projects: WriteSignal<Vec<(String, String)>>,
    /// True while the project list is being fetched.
    bq_projects_loading: ReadSignal<bool>,
    /// Non-None when the project fetch returned an error or warning message.
    bq_projects_error: ReadSignal<Option<String>>,
    /// Setter for `bq_projects_error`. KYO-468: always reset alongside
    /// `set_bq_projects`/`set_bq_projects_attempted` via
    /// `try_reset_bq_projects_signals` — see that function's doc comment for
    /// why the three travel together — so switching away from a mode that
    /// failed a listing doesn't leave its error message attached to
    /// whatever mode is selected next.
    set_bq_projects_error: WriteSignal<Option<String>>,
    /// Setter for `bq_projects_attempted` (owned by the parent
    /// `DatasourceModal`). KYO-468: always reset alongside `set_bq_projects`/
    /// `set_bq_projects_error` via `try_reset_bq_projects_signals` — without
    /// it, `create_mode_catalog_uses_generic_picker` can keep routing to
    /// `BqCreateModeProjectPicker` with another mode's stale `bq_projects`
    /// after switching to `enterprise_oauth` (or any future non-populating
    /// mode), since `bq_projects_attempted` alone doesn't know which mode
    /// set it.
    set_bq_projects_attempted: WriteSignal<bool>,
    /// Gates the admin-only connection-config surfaces in this section: the
    /// Authentication Mode selector, the Enterprise OAuth Client ID/Secret
    /// fields, and the Service Account JSON textarea. All three persist
    /// through `update_datasource_settings`, which non-admins cannot call
    /// (KYO-184). Does NOT gate the personal OAuth connect/disconnect panel
    /// or the billing project field — those are per-user and must stay
    /// visible to every member (`docs/DATASOURCE_ARCHITECTURE.md` §1/§5.2).
    is_admin: Signal<bool>,
    /// Registry-provided auth modes for BigQuery (KYO-274) — ids, labels,
    /// and descriptions for the Authentication Mode selector below. Sourced
    /// from `get_datasource_types()` by the parent `DatasourceModal`.
    auth_modes: Signal<Vec<AuthModeOption>>,
    /// Result of the last Test & Discover / Validate call — shared with the
    /// generic Test & Discover button other providers use. Drives the
    /// service_account mode's Valid/Failed indicator (KYO-405).
    test_result: ReadSignal<Option<TestConnectionResult>>,
    /// Setter for `test_result`. The service-account "Remove" chip and the
    /// JSON textarea's clear/invalidate paths are the one teardown route in
    /// this component that isn't an `Action` (KYO-405's disconnects go
    /// through `google_disconnect_action`/`datasource_disconnect_action`,
    /// whose parent-side Effects already reset `test_result`) — so this
    /// child needs its own way to re-close the "Next" gate when the
    /// credentials `test_result` was validated against disappear (KYO-413).
    /// `try_set` because this write crosses the parent/child signal boundary
    /// from a plain `on:click`/`on:input` handler.
    set_test_result: WriteSignal<Option<TestConnectionResult>>,
    /// Setter for the parent's `discovery_status` ("idle"/"loading"/
    /// "success"/"error"), reset alongside `set_test_result` above for the
    /// same reason — the two track together everywhere else in this file.
    set_discovery_status: WriteSignal<String>,
    /// True while `test_action` (the parent's Test & Discover Action) is
    /// pending — drives the "Validate & Discover Projects" button's
    /// disabled/"Validating..." state (KYO-405).
    test_pending: Signal<bool>,
    /// Dispatches the parent's `do_test_and_discover` for the
    /// service_account mode's "Validate & Discover Projects" button
    /// (KYO-405). A `Callback` rather than the raw closure so this component
    /// doesn't need the parent's private `TestDiscoverInput` type.
    on_validate: Callback<()>,
    /// KYO-408 — whether the user has ticked "I have requested access and
    /// had it confirmed" for the kyomi_oauth Google-account allowlist.
    /// Owned by the parent `DatasourceModal` (not local state) because the
    /// footer's Save/Create gate (`bq_kyomi_oauth_access_ok`) reads it too.
    bq_access_confirmed: ReadSignal<bool>,
    /// Setter for the checkbox above.
    set_bq_access_confirmed: WriteSignal<bool>,
    /// KYO-477 — the Connect/Reconnect-only gate, deliberately NOT the
    /// same signal as the footer's Save/Create gate
    /// (`bq_kyomi_oauth_access_ok`, read directly by `DatasourceModal`'s
    /// own footer buttons and never threaded down here). See
    /// `bq_kyomi_oauth_connect_allowed`'s doc comment in
    /// `bq_kyomi_oauth_connect_ok`'s definition (`DatasourceModal`) for
    /// why these two must stay separate predicates rather than one
    /// shared signal — folding Save/Create's `oauth_connected` allowance
    /// in here is exactly the KYO-477 defect.
    bq_kyomi_oauth_connect_ok: Signal<bool>,
) -> impl IntoView {
    // Parse service account email from JSON
    let handle_service_account_json = move |json_text: String| {
        set_cfg_service_account_json.set(json_text.clone());
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_text) {
            if let Some(email) = parsed.get("client_email").and_then(|v| v.as_str()) {
                set_service_account_email.set(email.to_string());
            } else {
                set_service_account_email.set(String::new());
                // KYO-413 — the JSON no longer resolves to a usable service
                // account, so any prior `test_result` was validated against
                // credentials that are now gone.
                set_test_result.try_set(None);
                set_discovery_status.try_set("idle".to_string());
            }
        } else if json_text.is_empty() {
            set_service_account_email.set(String::new());
            set_test_result.try_set(None);
            set_discovery_status.try_set("idle".to_string());
        }
    };

    // ── Kyomi OAuth: connect URL is fixed (no slug).
    let kyomi_oauth_url = Signal::stored("/api/v1/auth/google-oauth/connect".to_string());

    // ── Enterprise OAuth: connect URL is slug-scoped.
    let enterprise_oauth_url = Signal::derive(move || {
        let s = slug.get();
        format!("/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug={s}")
    });

    // ── Enterprise OAuth: "not configured" when client ID or secret is
    // missing (KYO-519 — see oauth_config_missing's doc comment for why
    // this must route through the shared predicate rather than an inline
    // `&&`, which only catches a config left fully blank).
    let enterprise_cfg_missing = Signal::derive(move || {
        oauth_config_missing(&cfg_oauth_client_id.get(), &cfg_oauth_client_secret.get())
    });

    // ── Disconnect callbacks — wraps the Action dispatch in a Callback<()>.
    let on_google_disconnect = Callback::new(move |()| {
        if !google_disconnect_action.pending().get_untracked() {
            google_disconnect_action.dispatch(());
        }
    });

    let slug_for_disconnect = slug;
    let on_enterprise_disconnect = Callback::new(move |()| {
        if !datasource_disconnect_action.pending().get_untracked() {
            let slug_val = slug_for_disconnect.get_untracked();
            datasource_disconnect_action
                .dispatch(("bigquery-enterprise".to_string(), slug_val));
        }
    });

    let google_disconnect_pending =
        Signal::derive(move || google_disconnect_action.pending().get());
    let enterprise_disconnect_pending =
        Signal::derive(move || datasource_disconnect_action.pending().get());

    // Re-fetch OAuth status whenever bq_auth_mode changes so the status panel
    // reflects the correct account for the newly selected mode.
    use_oauth_status_refetch(
        bq_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        bigquery_oauth_source,
    );

    // Recovers a connect attempt that resolved without an OAuth
    // postMessage ever arriving — popup closed, or timed out (KYO-437).
    // Built once for the whole section (not once per panel below) since it
    // re-resolves the current mode's OAuthStatusSource dynamically, so this
    // one callback correctly serves both the kyomi_oauth and
    // enterprise_oauth panels.
    let on_oauth_recover = build_oauth_recovery_callback(
        bq_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        set_oauth_connecting,
        bigquery_oauth_source,
        "BigQuery",
    );

    // Redirect URL shown in the Enterprise OAuth configuration panel so users
    // know what to enter in the Google Cloud OAuth client settings.
    // Only computed on WASM — the component won't render server-side.
    #[cfg(target_arch = "wasm32")]
    let enterprise_redirect_url = {
        let origin = web_sys::window()
            .map(|w| w.location().origin().unwrap_or_default())
            .unwrap_or_default();
        format!("{}/auth/oauth/bigquery-enterprise/callback", origin)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let enterprise_redirect_url = String::new();

    let enterprise_redirect_url_signal = Signal::stored(enterprise_redirect_url.clone());

    view! {
        // Authentication Mode selector — admin-only (KYO-184): persists via
        // `update_datasource_settings`, which non-admins cannot call. The
        // per-mode panels below still render correctly for non-admins using
        // whatever mode was already loaded from the server (`bq_auth_mode`
        // is set by the settings-load effect regardless of who is viewing).
        <Show when=move || is_admin.get()>
            <div class="space-y-2 pb-4 border-b border-border">
                <label class="block text-sm font-medium">"Authentication Mode"</label>
                <Select
                    value=Signal::derive(move || bq_auth_mode.get())
                    options=Signal::derive(move || auth_mode_select_options(&auth_modes.get()))
                    on_change=move |val| {
                        set_bq_auth_mode.set(val);
                        // KYO-413 — switching auth mode invalidates any
                        // `test_result` validated against the previous mode's
                        // credentials; leaving it in place would let a stale
                        // success keep "Next" open for a mode that was never
                        // validated. `try_set` because this write crosses the
                        // parent/child signal boundary from a plain event
                        // handler, same as the Remove chip and JSON-clear
                        // paths below.
                        set_test_result.try_set(None);
                        set_discovery_status.try_set("idle".to_string());
                        // KYO-468 — same reasoning, for the discovered
                        // project list: a project list (and any error)
                        // fetched under the previous mode belongs to that
                        // mode's credentials, not whatever mode was just
                        // selected. Without this reset, switching from
                        // service_account (populated) to enterprise_oauth
                        // (never populates) leaves `bq_projects_attempted`
                        // true and the create-mode Catalog tab would render
                        // the previous mode's stale project checkboxes as
                        // if they belonged to the new one. `try_reset_...`
                        // for the same parent/child boundary reason as
                        // `set_test_result` above.
                        try_reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);
                    }
                />
                <p class="text-xs text-muted-foreground">
                    {move || auth_mode_description(&auth_modes.get(), &bq_auth_mode.get())}
                </p>
            </div>
        </Show>

        // BigQuery Credentials section
        <div class="space-y-4 border-t border-border pt-4 mt-4">
            <h4 class="text-sm font-medium">"BigQuery Credentials"</h4>

            // Kyomi OAuth mode
            <Show when=move || bq_auth_mode.get() == "kyomi_oauth">
                <div class="space-y-3">
                    <p class="text-sm text-muted-foreground">
                        "Connect your Google account to access BigQuery projects."
                    </p>
                    // KYO-408/KYO-477 — Kyomi has no programmatic access to
                    // the Google Cloud Console's test-user allowlist for its
                    // shared OAuth app; Google is the only thing that can let
                    // a given account through or refuse it. This notice +
                    // checkbox is NOT a security gate (there's nothing here
                    // for Kyomi to protect, and no dishonest tick bypasses
                    // anything Google wouldn't already stop) — it exists
                    // purely so the user requests access *before* burning a
                    // doomed OAuth round-trip. Hidden once connected: at that
                    // point `ModalOAuthStatusPanel` below renders its
                    // "Connected" branch (Disconnect only, no Connect button
                    // to gate), and Save/Create's own gate
                    // (`bq_kyomi_oauth_access_ok`'s doc comment above, in
                    // `DatasourceModal`) auto-satisfies from the same
                    // connected state — so there is nothing left for this
                    // notice to ask for. The Connect gate
                    // (`bq_kyomi_oauth_connect_ok`, read by the
                    // `connect_blocked` prop below) does NOT auto-satisfy
                    // from `oauth_connected` — see its own doc comment for
                    // why — but that is moot here specifically because the
                    // Connect button isn't reachable while connected either.
                    // KYO-499 — restores parity with the React original
                    // (`AuthModeSelector.jsx` at `ee16f48a^`): one sentence
                    // plus an inline beta-access request link, not a
                    // heading + two explanatory paragraphs + a standalone
                    // Button component (that shape shipped in KYO-435/
                    // KYO-477/KYO-478 without verifying against React and
                    // was rejected as "a monstrosity" — see KYO-499). The link
                    // goes to the shared mailto constant in `utils::beta_access`
                    // (see that module for the exact target) — the same one
                    // `pages/auth/login.rs`'s Google sign-in notice uses —
                    // rather than the `FeedbackAccessRequestHandle`/
                    // FeedbackModal path this used before, because the
                    // login page has no `Layout` context to reach that
                    // modal and the two surfaces must use one identical
                    // target (KYO-499's decision; React's own target,
                    // `/beta-signup`, was never ported and stays out of
                    // scope — see the ticket).
                    //
                    // This comment deliberately does not quote the exact
                    // copy strings below — `tests/oauth.rs`'s source-text
                    // assertions for this block scan for those literals,
                    // and an echo here would let a regression in the real
                    // markup pass unnoticed (verified by mutation during
                    // KYO-499 implementation).
                    <Show when=move || !oauth_connected.get()>
                        <Alert variant=AlertVariant::Warning>
                            <Icon icon=phosphor_leptos::WARNING_CIRCLE attr:class="h-4 w-4" />
                            <AlertDescription>
                                <p class="mb-3">
                                    "This authentication method requires beta access. "
                                    <a
                                        href=beta_access::BETA_ACCESS_REQUEST_HREF
                                        class="text-primary hover:underline font-medium"
                                    >
                                        "Request beta access"
                                    </a>
                                </p>
                                <label class="flex items-center gap-2 cursor-pointer">
                                    <Checkbox
                                        checked=Signal::derive(move || bq_access_confirmed.get())
                                        on_change=Callback::new(move |v: bool| {
                                            // KYO-499 — persist to
                                            // localStorage["hasBetaAccess"]
                                            // alongside the in-memory signal;
                                            // see `bq_access_confirmed`'s doc
                                            // comment.
                                            beta_access::write_beta_access(v);
                                            set_bq_access_confirmed.set(v)
                                        })
                                    />
                                    <span class="text-sm">
                                        "I have beta access"
                                    </span>
                                </label>
                            </AlertDescription>
                        </Alert>
                    </Show>
                    // 4-state OAuth status panel — shown in create mode too:
                    // `kyomi_oauth_url` (/api/v1/auth/google-oauth/connect) is
                    // not slug-scoped, so it connects identically before or
                    // after the datasource is saved.
                    <ModalOAuthStatusPanel
                        oauth_connected=oauth_connected
                        oauth_email=oauth_email
                        oauth_expired=oauth_expired
                        oauth_connecting=oauth_connecting
                        set_oauth_connecting=set_oauth_connecting
                        provider_name="BigQuery"
                        connect_url=kyomi_oauth_url
                        // KYO-519 — deliberately NOT oauth_config_missing(...).
                        // kyomi_oauth is account-level, globally-hosted Kyomi
                        // OAuth: it has no configFields (no per-datasource
                        // Client ID/Secret) at all, so there is nothing for
                        // that predicate to check. Leave this hardcoded
                        // false rather than "fixing" it into a call — doing
                        // so would read cfg_oauth_client_id/secret, which
                        // belong to a different auth mode (enterprise_oauth)
                        // entirely and are unrelated to this panel.
                        cfg_missing=Signal::stored(false)
                        connect_blocked=Signal::derive(move || {
                            !bq_kyomi_oauth_connect_ok.get()
                        })
                        on_disconnect=on_google_disconnect
                        disconnect_pending=google_disconnect_pending
                        on_recover=on_oauth_recover
                    />
                    <Show when=move || !oauth_connected.get()>
                        <p class="text-xs text-muted-foreground">
                            "After connecting, you can set the billing project."
                        </p>
                    </Show>
                    <BqProjectField
                        label="Billing Project"
                        value=cred_billing_project
                        set_value=set_cred_billing_project
                        bq_projects=bq_projects
                        bq_projects_loading=bq_projects_loading
                    />
                    {move || bq_projects_error.get().filter(|_| bq_projects.get().is_empty()).map(|err| view! {
                        <Alert variant=AlertVariant::Warning class="mt-2">
                            <AlertDescription>
                                {err} " You can still enter project IDs manually below."
                            </AlertDescription>
                        </Alert>
                    })}
                </div>
            </Show>

            // Enterprise OAuth mode
            <Show when=move || bq_auth_mode.get() == "enterprise_oauth">
                <div class="space-y-4">
                    // Admin OAuth configuration — admin-only (KYO-184): the
                    // client ID/secret persist through `update_datasource_settings`.
                    <Show when=move || is_admin.get()>
                        <div class="space-y-3 pb-4 border-b border-border">
                            <h4 class="text-sm font-medium">"OAuth Configuration"</h4>
                            <p class="text-xs text-muted-foreground">
                                "Configure your organization's Google Cloud OAuth app."
                            </p>
                            <div class="p-3 bg-muted/30 rounded-lg space-y-1">
                                <label class="block text-xs font-medium text-muted-foreground">
                                    "Redirect URL (use when creating Google Cloud OAuth client)"
                                </label>
                                <div class="flex items-center gap-2">
                                    <code class="text-xs font-mono break-all flex-1">{move || enterprise_redirect_url_signal.get()}</code>
                                    <CopyButton text=enterprise_redirect_url_signal/>
                                </div>
                            </div>
                            <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                <div>
                                    <label class="block text-sm font-medium mb-1">"OAuth Client ID"</label>
                                    <input
                                        type="text"
                                        class=MODAL_INPUT_CLASS
                                        placeholder="From Google Cloud Console"
                                        prop:value=move || cfg_oauth_client_id.get()
                                        on:input=move |ev| set_cfg_oauth_client_id.set(event_target_value(&ev))
                                    />
                                </div>
                                <div>
                                    <label class="block text-sm font-medium mb-1">"OAuth Client Secret"</label>
                                    <input
                                        type="password"
                                        class=MODAL_INPUT_CLASS
                                        placeholder="OAuth client secret"
                                        prop:value=move || cfg_oauth_client_secret.get()
                                        on:input=move |ev| set_cfg_oauth_client_secret.set(event_target_value(&ev))
                                    />
                                </div>
                            </div>
                        </div>
                    </Show>
                    // User connection — 4-state status panel.
                    // Hidden in create mode (no slug yet for the enterprise endpoint).
                    <Show when=move || !is_create_mode.get()>
                        <div class="space-y-3">
                            <h4 class="text-sm font-medium">"Your Connection"</h4>
                            <ModalOAuthStatusPanel
                                oauth_connected=oauth_connected
                                oauth_email=oauth_email
                                oauth_expired=oauth_expired
                                oauth_connecting=oauth_connecting
                                set_oauth_connecting=set_oauth_connecting
                                provider_name="BigQuery"
                                connect_url=enterprise_oauth_url
                                cfg_missing=enterprise_cfg_missing
                                on_disconnect=on_enterprise_disconnect
                                disconnect_pending=enterprise_disconnect_pending
                                on_recover=on_oauth_recover
                            />
                        </div>
                    </Show>
                    <Show when=move || is_create_mode.get()>
                        <p class="text-xs text-muted-foreground">
                            "After saving, connect your BigQuery account from this settings panel."
                        </p>
                    </Show>
                    // Billing project field — same conditional Select pattern
                    // as kyomi_oauth mode.
                    <Show when=move || !oauth_connected.get()>
                        <p class="text-xs text-muted-foreground">
                            "After connecting, you can set the billing project."
                        </p>
                    </Show>
                    <BqProjectField
                        label="Billing Project"
                        value=cred_billing_project
                        set_value=set_cred_billing_project
                        bq_projects=bq_projects
                        bq_projects_loading=bq_projects_loading
                    />
                    {move || bq_projects_error.get().filter(|_| bq_projects.get().is_empty()).map(|err| view! {
                        <Alert variant=AlertVariant::Warning class="mt-2">
                            <AlertDescription>
                                {err} " You can still enter project IDs manually below."
                            </AlertDescription>
                        </Alert>
                    })}
                </div>
            </Show>

            // Service Account mode — entirely admin-config (KYO-184): there is
            // no personal credential in this mode, so a non-admin sees an
            // explanatory note instead of the JSON textarea.
            <Show when=move || bq_auth_mode.get() == "service_account">
                <Show
                    when=move || is_admin.get()
                    fallback=move || view! {
                        <p class="text-xs text-muted-foreground">
                            "This datasource uses a shared service account configured by a workspace admin."
                        </p>
                    }
                >
                    <div class="space-y-4">
                        <p class="text-xs text-muted-foreground">
                            "Upload or paste your Google Cloud service account credentials JSON."
                        </p>
                        {move || service_account_email.get().is_empty().then(|| view! {
                            <div class="space-y-3">
                                <div>
                                    <label class="block text-sm font-medium mb-1">"Service Account JSON"</label>
                                    <textarea
                                        rows="6"
                                        class="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono focus:outline-none focus:ring-1 focus:ring-ring"
                                        placeholder=r#"{"type": "service_account", "client_email": "...", ...}"#
                                        prop:value=move || cfg_service_account_json.get()
                                        on:input=move |ev| {
                                            handle_service_account_json(event_target_value(&ev));
                                        }
                                    />
                                    <p class="text-xs text-muted-foreground mt-1">
                                        "Paste the contents of your service account JSON file"
                                    </p>
                                </div>
                            </div>
                        })}
                        {move || {
                            let email = service_account_email.get();
                            (!email.is_empty()).then(move || view! {
                                <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                                    <div class="flex items-center gap-2">
                                        <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4 text-success-foreground"/>
                                        <span class="text-sm text-foreground">
                                            {format!("Service Account: {email}")}
                                        </span>
                                    </div>
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        on:click=move |_| {
                                            set_service_account_email.set(String::new());
                                            set_cfg_service_account_json.set(String::new());
                                            // KYO-413 — removing the service account
                                            // credentials must re-close the "Next" gate;
                                            // otherwise a stale `test_result` from before
                                            // the removal keeps it enabled.
                                            set_test_result.try_set(None);
                                            set_discovery_status.try_set("idle".to_string());
                                            // Clear the discovered project list too — it
                                            // was populated by validating the credentials
                                            // just removed, so BqProjectField's dropdowns
                                            // must not keep offering them (KYO-413). KYO-468
                                            // — and the error/attempted flags that travel
                                            // with it: without this, removing the
                                            // credentials after a failed Validate leaves
                                            // `bq_projects_attempted` true and
                                            // `bq_projects_error` set, so the create-mode
                                            // Catalog tab renders the stale failure message
                                            // for a service account the user just removed
                                            // instead of falling back to "not yet
                                            // validated".
                                            try_reset_bq_projects_signals(set_bq_projects, set_bq_projects_error, set_bq_projects_attempted);
                                        }
                                    >
                                        "Remove"
                                    </Button>
                                </div>
                            })
                        }}
                        // "Validate & Discover Projects" — only once a service
                        // account is uploaded (matches React's
                        // `{serviceAccountEmail && (...)}` gate). Wrapped in
                        // its own `<Show>` rather than the `{move || ...}.then()`
                        // pattern used for the chip above, because
                        // `BqProjectField` mounts `Select`, which owns an
                        // internal `Effect` — branching that inside a plain
                        // reactive closure would destroy/recreate it on every
                        // service_account_email change (docs/CODING_STANDARDS.md
                        // "Use <Show> for conditional component rendering").
                        <Show when=move || !service_account_email.get().is_empty()>
                            <div class="space-y-4">
                                <div class="flex items-center gap-3">
                                    <Button
                                        variant=ButtonVariant::Outline
                                        disabled=Signal::derive(move || test_pending.get())
                                        on:click=move |_| on_validate.run(())
                                    >
                                        {move || if test_pending.get() {
                                            view! {
                                                <span class="flex items-center gap-1.5">
                                                    <Spinner size="h-3 w-3"/>
                                                    "Validating..."
                                                </span>
                                            }.into_any()
                                        } else {
                                            view! {
                                                <span class="flex items-center gap-1.5">
                                                    <Icon icon=phosphor_leptos::PLUG size="14px"/>
                                                    "Validate & Discover Projects"
                                                </span>
                                            }.into_any()
                                        }}
                                    </Button>
                                    <ConnectionTestResultBadge test_result=test_result success_label="Valid" />
                                </div>
                                <BqProjectField
                                    label="Billing Project"
                                    value=cred_billing_project
                                    set_value=set_cred_billing_project
                                    bq_projects=bq_projects
                                    bq_projects_loading=bq_projects_loading
                                />
                                {move || bq_projects_error.get().filter(|_| bq_projects.get().is_empty()).map(|err| view! {
                                    <Alert variant=AlertVariant::Warning class="mt-2">
                                        <AlertDescription>
                                            {err} " You can still enter project IDs manually below."
                                        </AlertDescription>
                                    </Alert>
                                })}
                            </div>
                        </Show>
                    </div>
                </Show>
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Snowflake Auth Mode Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SnowflakeAuthModeSection(
    sf_auth_mode: ReadSignal<String>,
    set_sf_auth_mode: WriteSignal<String>,
    /// Datasource slug — used to build the Snowflake OAuth connect URL.
    slug: ReadSignal<String>,
    /// Whether the OAuth account is currently connected.
    oauth_connected: ReadSignal<bool>,
    /// Setter for the connected state (used by re-fetch Effect on mode change).
    set_oauth_connected: WriteSignal<bool>,
    /// The connected account email, if any.
    oauth_email: ReadSignal<Option<String>>,
    /// Setter for the email state (used by re-fetch Effect on mode change).
    set_oauth_email: WriteSignal<Option<String>>,
    /// Whether the OAuth token has expired.
    oauth_expired: ReadSignal<bool>,
    /// Setter for the expired state (used by re-fetch Effect on mode change).
    set_oauth_expired: WriteSignal<bool>,
    /// Whether an OAuth popup is currently in progress.
    oauth_connecting: ReadSignal<bool>,
    /// Setter for the connecting state.
    set_oauth_connecting: WriteSignal<bool>,
    /// Action to disconnect the per-datasource OAuth account.
    datasource_disconnect_action: Action<(String, String), Result<crate::server_fns::datasource_oauth::DatasourceOAuthDisconnectResult, ServerFnError>>,
    /// True in create mode — OAuth status panel is hidden in create mode.
    is_create_mode: Signal<bool>,
    /// Admin-configured OAuth Client ID (KYO-519). Read-only here — unlike
    /// `DatabricksAuthModeSection`, this component doesn't render the
    /// Client ID/Secret input fields itself (those live in
    /// `ProviderConnectionFields`'s `"snowflake"` arm, reading/writing the
    /// same signal); it only needs the value to compute `cfg_missing`
    /// below, so no `set_cfg_oauth_client_id` is threaded through.
    cfg_oauth_client_id: ReadSignal<String>,
    /// Admin-configured OAuth Client Secret (KYO-519). Same rationale as
    /// `cfg_oauth_client_id` above — read-only, computed into
    /// `cfg_missing`.
    cfg_oauth_client_secret: ReadSignal<String>,
    /// Gates the Authentication Mode selector — admin-only (KYO-184), it
    /// persists through `update_datasource_settings`. Does NOT gate the
    /// personal OAuth connect/disconnect panel below, which stays visible to
    /// every member for whatever mode is already loaded.
    is_admin: Signal<bool>,
    /// Registry-provided auth modes for Snowflake (KYO-274) — ids, labels,
    /// and descriptions for the Authentication Mode selector below. Sourced
    /// from `get_datasource_types()` by the parent `DatasourceModal`.
    auth_modes: Signal<Vec<AuthModeOption>>,
    /// Setter for the parent's `test_result`, reset when the Authentication
    /// Mode selector changes — a `test_result` validated against the
    /// previous mode's credentials must not keep "Next" open for a mode
    /// that was never validated (KYO-413). `try_set` because this write
    /// crosses the parent/child signal boundary from a plain event handler.
    set_test_result: WriteSignal<Option<TestConnectionResult>>,
    /// Setter for the parent's `discovery_status`, reset alongside
    /// `set_test_result` above for the same reason.
    set_discovery_status: WriteSignal<String>,
) -> impl IntoView {
    // Snowflake connect URL is slug-scoped.
    let sf_connect_url = Signal::derive(move || {
        let s = slug.get();
        format!("/api/v1/auth/oauth/snowflake/connect?datasource_slug={s}")
    });

    // cfg_missing: true when admin has not configured OAuth Client ID/Secret
    // (KYO-519 — previously hardcoded Signal::stored(false), so a member
    // saw a normal Connect button and a doomed connect attempt no matter
    // what the admin had entered. Routed through the shared
    // oauth_config_missing predicate, same as the other three providers).
    let sf_cfg_missing = Signal::derive(move || {
        oauth_config_missing(&cfg_oauth_client_id.get(), &cfg_oauth_client_secret.get())
    });

    // Redirect URL for display in OAuth mode.
    // On native (non-WASM) targets, we have no window.location.origin — use a placeholder.
    #[cfg(target_arch = "wasm32")]
    let redirect_url = {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        format!("{origin}/auth/oauth/snowflake/callback")
    };
    #[cfg(not(target_arch = "wasm32"))]
    let redirect_url = "/auth/oauth/snowflake/callback".to_string();

    let redirect_url_signal = Signal::stored(redirect_url);

    // Re-fetch OAuth status whenever sf_auth_mode changes so the status panel
    // reflects the correct account for the newly selected mode.
    use_oauth_status_refetch(
        sf_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        snowflake_oauth_source,
    );

    // Recovers a connect attempt that resolved without an OAuth
    // postMessage ever arriving — popup closed, or timed out (KYO-437).
    let on_oauth_recover = build_oauth_recovery_callback(
        sf_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        set_oauth_connecting,
        snowflake_oauth_source,
        "Snowflake",
    );

    let slug_for_disconnect = slug;
    let on_sf_disconnect = Callback::new(move |()| {
        if !datasource_disconnect_action.pending().get_untracked() {
            let slug_val = slug_for_disconnect.get_untracked();
            datasource_disconnect_action.dispatch(("snowflake".to_string(), slug_val));
        }
    });

    let sf_disconnect_pending =
        Signal::derive(move || datasource_disconnect_action.pending().get());

    view! {
        // Authentication Mode selector — admin-only (KYO-184). The OAuth
        // status panel below still reflects whichever mode was already
        // loaded from the server, so it renders correctly for non-admins too.
        <Show when=move || is_admin.get()>
            <div class="space-y-2 pb-4 border-b border-border">
                <label class="block text-sm font-medium">"Authentication Mode"</label>
                <Select
                    value=Signal::derive(move || sf_auth_mode.get())
                    options=Signal::derive(move || auth_mode_select_options(&auth_modes.get()))
                    on_change=move |val| {
                        set_sf_auth_mode.set(val);
                        // KYO-413 — see BigQueryAuthModeSection's Authentication
                        // Mode on_change for why this reset is needed.
                        set_test_result.try_set(None);
                        set_discovery_status.try_set("idle".to_string());
                    }
                />
                <p class="text-xs text-muted-foreground">
                    {move || auth_mode_description(&auth_modes.get(), &sf_auth_mode.get())}
                </p>
            </div>
        </Show>

        // OAuth connection status — shown only when OAuth mode is selected.
        <Show when=move || sf_auth_mode.get() == "oauth">
            <div class="space-y-3 border-t border-border pt-4 mt-4">
                <h4 class="text-sm font-medium">"Your Snowflake Connection"</h4>
                // Redirect URL copy block — shown so admins can register the callback URI.
                <div class="p-3 bg-muted/30 rounded-lg space-y-1">
                    <label class="block text-xs font-medium text-muted-foreground">
                        "Redirect URL (add as a redirect URI in your Snowflake OAuth app)"
                    </label>
                    <div class="flex items-center gap-2 mt-1">
                        <code class="flex-1 text-xs font-mono text-foreground break-all">
                            {move || redirect_url_signal.get()}
                        </code>
                        <CopyButton text=redirect_url_signal/>
                    </div>
                </div>
                // 4-state status panel — hidden in create mode (no slug yet).
                <Show when=move || !is_create_mode.get()>
                    <ModalOAuthStatusPanel
                        oauth_connected=oauth_connected
                        oauth_email=oauth_email
                        oauth_expired=oauth_expired
                        oauth_connecting=oauth_connecting
                        set_oauth_connecting=set_oauth_connecting
                        provider_name="Snowflake"
                        connect_url=sf_connect_url
                        cfg_missing=sf_cfg_missing
                        on_disconnect=on_sf_disconnect
                        disconnect_pending=sf_disconnect_pending
                        on_recover=on_oauth_recover
                    />
                </Show>
                <Show when=move || is_create_mode.get()>
                    <p class="text-xs text-muted-foreground">
                        "After saving, connect your Snowflake account from this settings panel."
                    </p>
                </Show>
            </div>
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Databricks Auth Mode Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn DatabricksAuthModeSection(
    db_auth_mode: ReadSignal<String>,
    set_db_auth_mode: WriteSignal<String>,
    /// Datasource slug — used to build the Databricks OAuth connect URL.
    slug: ReadSignal<String>,
    /// Whether the OAuth account is currently connected.
    oauth_connected: ReadSignal<bool>,
    /// Setter for the connected state (used by re-fetch Effect on mode change).
    set_oauth_connected: WriteSignal<bool>,
    /// The connected account email, if any.
    oauth_email: ReadSignal<Option<String>>,
    /// Setter for the email state (used by re-fetch Effect on mode change).
    set_oauth_email: WriteSignal<Option<String>>,
    /// Whether the OAuth token has expired.
    oauth_expired: ReadSignal<bool>,
    /// Setter for the expired state (used by re-fetch Effect on mode change).
    set_oauth_expired: WriteSignal<bool>,
    /// Whether an OAuth popup is currently in progress.
    oauth_connecting: ReadSignal<bool>,
    /// Setter for the connecting state.
    set_oauth_connecting: WriteSignal<bool>,
    /// Action to disconnect the per-datasource OAuth account.
    datasource_disconnect_action: Action<(String, String), Result<crate::server_fns::datasource_oauth::DatasourceOAuthDisconnectResult, ServerFnError>>,
    /// True in create mode — OAuth status panel is hidden in create mode.
    is_create_mode: Signal<bool>,
    /// OAuth Client ID (admin-configured).
    cfg_oauth_client_id: ReadSignal<String>,
    set_cfg_oauth_client_id: WriteSignal<String>,
    /// OAuth Client Secret (admin-configured).
    cfg_oauth_client_secret: ReadSignal<String>,
    set_cfg_oauth_client_secret: WriteSignal<String>,
    /// Gates the Authentication Mode selector and the admin-configured OAuth
    /// Client ID/Secret fields — both persist through
    /// `update_datasource_settings` (KYO-184). Does NOT gate "Your Databricks
    /// Connection" below, which is per-user and stays visible to every member.
    is_admin: Signal<bool>,
    /// Registry-provided auth modes for Databricks (KYO-274) — ids, labels,
    /// and descriptions for the Authentication Mode selector below. Sourced
    /// from `get_datasource_types()` by the parent `DatasourceModal`.
    auth_modes: Signal<Vec<AuthModeOption>>,
    /// Setter for the parent's `test_result`, reset when the Authentication
    /// Mode selector changes — a `test_result` validated against the
    /// previous mode's credentials must not keep "Next" open for a mode
    /// that was never validated (KYO-413). `try_set` because this write
    /// crosses the parent/child signal boundary from a plain event handler.
    set_test_result: WriteSignal<Option<TestConnectionResult>>,
    /// Setter for the parent's `discovery_status`, reset alongside
    /// `set_test_result` above for the same reason.
    set_discovery_status: WriteSignal<String>,
) -> impl IntoView {
    // Databricks connect URL is slug-scoped.
    let db_connect_url = Signal::derive(move || {
        let s = slug.get();
        format!("/api/v1/auth/oauth/databricks/connect?datasource_slug={s}")
    });

    let slug_for_disconnect = slug;
    let on_db_disconnect = Callback::new(move |()| {
        if !datasource_disconnect_action.pending().get_untracked() {
            let slug_val = slug_for_disconnect.get_untracked();
            datasource_disconnect_action.dispatch(("databricks".to_string(), slug_val));
        }
    });

    let db_disconnect_pending =
        Signal::derive(move || datasource_disconnect_action.pending().get());

    // cfg_missing: true when admin has not configured OAuth Client ID/Secret
    // (KYO-519 — routed through the shared oauth_config_missing predicate
    // so this and its three siblings can't drift from each other again).
    let db_cfg_missing = Signal::derive(move || {
        oauth_config_missing(&cfg_oauth_client_id.get(), &cfg_oauth_client_secret.get())
    });

    // Re-fetch OAuth status whenever db_auth_mode changes to "oauth" so the
    // status panel reflects the current account state.
    use_oauth_status_refetch(
        db_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        databricks_oauth_source,
    );

    // Recovers a connect attempt that resolved without an OAuth
    // postMessage ever arriving — popup closed, or timed out (KYO-437).
    let on_oauth_recover = build_oauth_recovery_callback(
        db_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        set_oauth_connecting,
        databricks_oauth_source,
        "Databricks",
    );

    // Redirect URL for the Databricks OAuth app registration.
    // Only computed on WASM — the component won't render server-side.
    #[cfg(target_arch = "wasm32")]
    let redirect_url_text = {
        let origin = web_sys::window()
            .map(|w| w.location().origin().unwrap_or_default())
            .unwrap_or_default();
        format!("{}/auth/oauth/databricks/callback", origin)
    };
    #[cfg(not(target_arch = "wasm32"))]
    let redirect_url_text = String::new();

    let redirect_url_signal = Signal::stored(redirect_url_text);

    view! {
        // Authentication Mode selector — admin-only (KYO-184).
        <Show when=move || is_admin.get()>
            <div class="space-y-2 pb-4 border-b border-border">
                <label class="block text-sm font-medium">"Authentication Mode"</label>
                <Select
                    value=Signal::derive(move || db_auth_mode.get())
                    options=Signal::derive(move || auth_mode_select_options(&auth_modes.get()))
                    on_change=move |val| {
                        set_db_auth_mode.set(val);
                        // KYO-413 — see BigQueryAuthModeSection's Authentication
                        // Mode on_change for why this reset is needed.
                        set_test_result.try_set(None);
                        set_discovery_status.try_set("idle".to_string());
                    }
                />
                <p class="text-xs text-muted-foreground">
                    {move || auth_mode_description(&auth_modes.get(), &db_auth_mode.get())}
                </p>
            </div>
        </Show>

        // OAuth configuration — shown only when OAuth mode is selected.
        <Show when=move || db_auth_mode.get() == "oauth">
            <div class="space-y-3 border-t border-border pt-4 mt-4">
                // Admin OAuth Client ID/Secret configuration — admin-only
                // (KYO-184): persists through `update_datasource_settings`.
                <Show when=move || is_admin.get()>
                    <div class="space-y-3 pb-4 border-b border-border">
                        <h4 class="text-sm font-medium">"OAuth Configuration"</h4>
                        <p class="text-xs text-muted-foreground">
                            "Configure your organization's Databricks OAuth app."
                        </p>
                        // Redirect URL helper
                        <div class="mt-3 p-3 rounded-md bg-muted">
                            <p class="text-xs text-muted-foreground mb-1">
                                "Redirect URL (use when creating Databricks OAuth app)"
                            </p>
                            <div class="flex items-center gap-2">
                                <code class="text-xs font-mono break-all flex-1">
                                    {move || redirect_url_signal.get()}
                                </code>
                                <CopyButton text=redirect_url_signal/>
                            </div>
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"OAuth Client ID"</label>
                                <input
                                    type="text"
                                    class=MODAL_INPUT_CLASS
                                    placeholder="From Databricks OAuth app"
                                    prop:value=move || cfg_oauth_client_id.get()
                                    on:input=move |ev| set_cfg_oauth_client_id.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"OAuth Client Secret"</label>
                                <input
                                    type="password"
                                    class=MODAL_INPUT_CLASS
                                    placeholder="OAuth client secret"
                                    prop:value=move || cfg_oauth_client_secret.get()
                                    on:input=move |ev| set_cfg_oauth_client_secret.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    </div>
                </Show>
                // User connection status
                <h4 class="text-sm font-medium">"Your Databricks Connection"</h4>
                // 4-state status panel — hidden in create mode (no slug yet).
                <Show when=move || !is_create_mode.get()>
                    <ModalOAuthStatusPanel
                        oauth_connected=oauth_connected
                        oauth_email=oauth_email
                        oauth_expired=oauth_expired
                        oauth_connecting=oauth_connecting
                        set_oauth_connecting=set_oauth_connecting
                        provider_name="Databricks"
                        connect_url=db_connect_url
                        cfg_missing=db_cfg_missing
                        on_disconnect=on_db_disconnect
                        disconnect_pending=db_disconnect_pending
                        on_recover=on_oauth_recover
                    />
                </Show>
                <Show when=move || is_create_mode.get()>
                    <p class="text-xs text-muted-foreground">
                        "After saving, connect your Databricks account from this settings panel."
                    </p>
                </Show>
            </div>
        </Show>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Synapse Auth Mode Section
// ─────────────────────────────────────────────────────────────────────────────

#[component]
fn SynapseAuthModeSection(
    synapse_auth_mode: ReadSignal<String>,
    set_synapse_auth_mode: WriteSignal<String>,
    /// Datasource slug — used to build the Microsoft Enterprise OAuth connect URL.
    slug: ReadSignal<String>,
    /// Admin-level OAuth client ID (enterprise_oauth mode, stored in connection_config).
    cfg_oauth_client_id: ReadSignal<String>,
    set_cfg_oauth_client_id: WriteSignal<String>,
    /// Admin-level OAuth client secret (enterprise_oauth mode, stored in connection_config).
    cfg_oauth_client_secret: ReadSignal<String>,
    set_cfg_oauth_client_secret: WriteSignal<String>,
    /// Whether the OAuth account is currently connected.
    oauth_connected: ReadSignal<bool>,
    /// Setter for the connected state (used by the re-fetch hook on mode change).
    set_oauth_connected: WriteSignal<bool>,
    /// The connected account email, if any.
    oauth_email: ReadSignal<Option<String>>,
    /// Setter for the email state (used by the re-fetch hook on mode change).
    set_oauth_email: WriteSignal<Option<String>>,
    /// Whether the OAuth token has expired.
    oauth_expired: ReadSignal<bool>,
    /// Setter for the expired state (used by the re-fetch hook on mode change).
    set_oauth_expired: WriteSignal<bool>,
    /// Whether an OAuth popup is currently in progress.
    oauth_connecting: ReadSignal<bool>,
    /// Setter for the connecting state.
    set_oauth_connecting: WriteSignal<bool>,
    /// Action to disconnect a per-datasource OAuth account.
    datasource_disconnect_action: Action<(String, String), Result<crate::server_fns::datasource_oauth::DatasourceOAuthDisconnectResult, ServerFnError>>,
    /// True in create mode — OAuth status panel is hidden in create mode.
    is_create_mode: Signal<bool>,
    /// Gates the Authentication Mode selector and the admin-configured OAuth
    /// Client ID/Secret fields — both persist through
    /// `update_datasource_settings` (KYO-184). Does NOT gate "Your
    /// Connection" below, which is per-user and stays visible to every member.
    is_admin: Signal<bool>,
    /// Registry-provided auth modes for Synapse (KYO-274) — ids, labels,
    /// and descriptions for the Authentication Mode selector below. Sourced
    /// from `get_datasource_types()` by the parent `DatasourceModal`.
    auth_modes: Signal<Vec<AuthModeOption>>,
    /// Setter for the parent's `test_result`, reset when the Authentication
    /// Mode selector changes — a `test_result` validated against the
    /// previous mode's credentials must not keep "Next" open for a mode
    /// that was never validated (KYO-413). `try_set` because this write
    /// crosses the parent/child signal boundary from a plain event handler.
    set_test_result: WriteSignal<Option<TestConnectionResult>>,
    /// Setter for the parent's `discovery_status`, reset alongside
    /// `set_test_result` above for the same reason.
    set_discovery_status: WriteSignal<String>,
) -> impl IntoView {
    // Re-fetch OAuth status whenever synapse_auth_mode changes so the status
    // panel reflects the correct account for the newly selected mode.
    use_oauth_status_refetch(
        synapse_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        synapse_oauth_source,
    );

    // Recovers a connect attempt that resolved without an OAuth
    // postMessage ever arriving — popup closed, or timed out (KYO-437).
    let on_oauth_recover = build_oauth_recovery_callback(
        synapse_auth_mode,
        slug,
        is_create_mode,
        OAuthStatusSetters {
            connected: set_oauth_connected,
            email: set_oauth_email,
            expired: set_oauth_expired,
        },
        set_oauth_connecting,
        synapse_oauth_source,
        "Microsoft",
    );

    // Microsoft Enterprise OAuth connect URL — slug-scoped
    let enterprise_oauth_url = Signal::derive(move || {
        let s = slug.get();
        format!(
            "/api/v1/auth/oauth/microsoft-enterprise/connect?datasource_slug={s}"
        )
    });

    // "not configured" when client ID or secret is missing (KYO-519 —
    // see oauth_config_missing's doc comment for why this must route
    // through the shared predicate rather than an inline `&&`, which
    // only catches a config left fully blank).
    let enterprise_cfg_missing = Signal::derive(move || {
        oauth_config_missing(&cfg_oauth_client_id.get(), &cfg_oauth_client_secret.get())
    });

    let slug_for_disconnect = slug;
    let on_enterprise_disconnect = Callback::new(move |()| {
        if !datasource_disconnect_action.pending().get_untracked() {
            let slug_val = slug_for_disconnect.get_untracked();
            datasource_disconnect_action
                .dispatch(("microsoft-enterprise".to_string(), slug_val));
        }
    });

    let enterprise_disconnect_pending =
        Signal::derive(move || datasource_disconnect_action.pending().get());

    // Redirect URL for display in enterprise OAuth mode
    // On native (non-WASM) targets, we have no window.location.origin — use a placeholder.
    #[cfg(target_arch = "wasm32")]
    let redirect_url = {
        let origin = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        format!("{origin}/auth/oauth/microsoft-enterprise/callback")
    };
    #[cfg(not(target_arch = "wasm32"))]
    let redirect_url = "/auth/oauth/microsoft-enterprise/callback".to_string();

    let redirect_url_signal = Signal::stored(redirect_url);

    view! {
        // Authentication Mode selector — admin-only (KYO-184).
        <Show when=move || is_admin.get()>
            <div class="space-y-2 pb-4 border-b border-border">
                <label class="block text-sm font-medium">"Authentication Mode"</label>
                <Select
                    value=Signal::derive(move || synapse_auth_mode.get())
                    options=Signal::derive(move || auth_mode_select_options(&auth_modes.get()))
                    on_change=move |val| {
                        set_synapse_auth_mode.set(val);
                        // KYO-413 — see BigQueryAuthModeSection's Authentication
                        // Mode on_change for why this reset is needed.
                        set_test_result.try_set(None);
                        set_discovery_status.try_set("idle".to_string());
                    }
                />
                <p class="text-xs text-muted-foreground">
                    {move || auth_mode_description(&auth_modes.get(), &synapse_auth_mode.get())}
                </p>
            </div>
        </Show>

        // Enterprise OAuth admin configuration + user connection panel
        <Show when=move || synapse_auth_mode.get() == "enterprise_oauth">
            <div class="space-y-4 border-t border-border pt-4 mt-4">
                // Admin OAuth configuration — admin-only (KYO-184): persists
                // through `update_datasource_settings`.
                <Show when=move || is_admin.get()>
                    <div class="space-y-3 pb-4 border-b border-border">
                        <h4 class="text-sm font-medium">"OAuth Configuration"</h4>
                        <p class="text-xs text-muted-foreground">
                            "Configure your organization's Azure AD app registration."
                        </p>
                        // Redirect URL copy block
                        <div class="p-3 bg-muted/30 rounded-lg space-y-1">
                            <label class="block text-xs font-medium text-muted-foreground">
                                "Redirect URL (add as a redirect URI in your Azure AD app)"
                            </label>
                            <div class="flex items-center gap-2 mt-1">
                                <code class="flex-1 text-xs font-mono text-foreground break-all">
                                    {move || redirect_url_signal.get()}
                                </code>
                                <CopyButton text=redirect_url_signal/>
                            </div>
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"OAuth Client ID"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="Application (client) ID"
                                    prop:value=move || cfg_oauth_client_id.get()
                                    on:input=move |ev| set_cfg_oauth_client_id.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"OAuth Client Secret"</label>
                                <input type="password" class=MODAL_INPUT_CLASS
                                    placeholder="Client secret value"
                                    prop:value=move || cfg_oauth_client_secret.get()
                                    on:input=move |ev| set_cfg_oauth_client_secret.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    </div>
                </Show>
                // User connection — 4-state status panel.
                // Hidden in create mode (no slug yet for the enterprise endpoint).
                <Show when=move || !is_create_mode.get()>
                    <div class="space-y-3">
                        <h4 class="text-sm font-medium">"Your Connection"</h4>
                        <ModalOAuthStatusPanel
                            oauth_connected=oauth_connected
                            oauth_email=oauth_email
                            oauth_expired=oauth_expired
                            oauth_connecting=oauth_connecting
                            set_oauth_connecting=set_oauth_connecting
                            provider_name="Microsoft"
                            connect_url=enterprise_oauth_url
                            cfg_missing=enterprise_cfg_missing
                            on_disconnect=on_enterprise_disconnect
                            disconnect_pending=enterprise_disconnect_pending
                            on_recover=on_oauth_recover
                        />
                    </div>
                </Show>
                <Show when=move || is_create_mode.get()>
                    <p class="text-xs text-muted-foreground">
                        "After saving, connect your Microsoft account from this settings panel."
                    </p>
                </Show>
            </div>
        </Show>

        // Service Principal credentials are rendered by ProviderCredentialsFields
        // when synapse_auth_mode == "service_principal".
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Connection Fields
// ─────────────────────────────────────────────────────────────────────────────

/// Bundle of every signal `ProviderConnectionFields` needs.
/// Packed as a single prop to keep the component signature small;
/// signals are `Copy`, so cloning the struct is cheap.
#[derive(Clone, Copy)]
struct ConnectionFieldsSignals {
    ds_type: ReadSignal<String>,
    sf_auth_mode: ReadSignal<String>,
    synapse_auth_mode: ReadSignal<String>,
    cfg_host: ReadSignal<String>,
    set_cfg_host: WriteSignal<String>,
    cfg_port: ReadSignal<String>,
    set_cfg_port: WriteSignal<String>,
    cfg_ssl_mode: ReadSignal<String>,
    set_cfg_ssl_mode: WriteSignal<String>,
    cfg_database: ReadSignal<String>,
    set_cfg_database: WriteSignal<String>,
    cfg_account: ReadSignal<String>,
    set_cfg_account: WriteSignal<String>,
    cfg_server_hostname: ReadSignal<String>,
    set_cfg_server_hostname: WriteSignal<String>,
    cfg_http_path: ReadSignal<String>,
    set_cfg_http_path: WriteSignal<String>,
    cfg_secure: ReadSignal<bool>,
    set_cfg_secure: WriteSignal<bool>,
    cfg_encrypt: ReadSignal<bool>,
    set_cfg_encrypt: WriteSignal<bool>,
    cfg_trust_cert: ReadSignal<bool>,
    set_cfg_trust_cert: WriteSignal<bool>,
    cfg_tenant_id: ReadSignal<String>,
    set_cfg_tenant_id: WriteSignal<String>,
    cfg_oauth_client_id: ReadSignal<String>,
    set_cfg_oauth_client_id: WriteSignal<String>,
    cfg_oauth_client_secret: ReadSignal<String>,
    set_cfg_oauth_client_secret: WriteSignal<String>,
}

/// Renders the connection fields (host/port/ssl_mode/account/etc.) for the active provider.
/// Also renders the database text input so users can type a database name before Test & Discover.
/// After Test & Discover, DiscoveryFields replaces the text input with a dropdown of real db names.
#[component]
fn ProviderConnectionFields(signals: ConnectionFieldsSignals) -> impl IntoView {
    let ConnectionFieldsSignals {
        ds_type,
        sf_auth_mode,
        synapse_auth_mode,
        cfg_host,
        set_cfg_host,
        cfg_port,
        set_cfg_port,
        cfg_ssl_mode,
        set_cfg_ssl_mode,
        cfg_database,
        set_cfg_database,
        cfg_account,
        set_cfg_account,
        cfg_server_hostname,
        set_cfg_server_hostname,
        cfg_http_path,
        set_cfg_http_path,
        cfg_secure,
        set_cfg_secure,
        cfg_encrypt,
        set_cfg_encrypt,
        cfg_trust_cert,
        set_cfg_trust_cert,
        cfg_tenant_id,
        set_cfg_tenant_id,
        cfg_oauth_client_id,
        set_cfg_oauth_client_id,
        cfg_oauth_client_secret,
        set_cfg_oauth_client_secret,
    } = signals;
    view! {
        {move || {
            let t = ds_type.get();
            match t.as_str() {
                "postgres" | "mysql" | "redshift" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Host " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="db.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder=if t == "mysql" { "3306" } else { "5432" }
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"SSL Mode"</label>
                            <Select
                                value=Signal::derive(move || cfg_ssl_mode.get())
                                options=Signal::stored(vec![
                                    ("disable".to_string(), "Disable".to_string()),
                                    ("require".to_string(), "Require".to_string()),
                                    ("verify-ca".to_string(), "Verify CA".to_string()),
                                    ("verify-full".to_string(), "Verify Full".to_string()),
                                ])
                                on_change=move |val| set_cfg_ssl_mode.set(val)
                            />
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="mydb"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                }.into_any(),

                "clickhouse" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Host " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="clickhouse.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder="8123"
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <label class="flex items-center gap-2 cursor-pointer">
                            <input
                                type="checkbox"
                                class="h-4 w-4 rounded-md border-input"
                                prop:checked=move || cfg_secure.get()
                                on:change=move |ev| {
                                    let checked = event_target_checked(&ev);
                                    set_cfg_secure.set(checked);
                                }
                            />
                            <span class="text-sm">"Secure (HTTPS)"</span>
                        </label>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="default"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                }.into_any(),

                "snowflake" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Account " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="xy12345.us-east-1 or myorg-myaccount"
                                prop:value=move || cfg_account.get()
                                on:input=move |ev| set_cfg_account.set(event_target_value(&ev))
                            />
                            <p class="text-xs text-muted-foreground mt-1">
                                "Your Snowflake account identifier (found in your Snowflake URL)"
                            </p>
                        </div>
                        // Snowflake OAuth config (admin only, when oauth mode)
                        <Show when=move || sf_auth_mode.get() == "oauth">
                            <div class="space-y-3 pt-2 border-t border-border">
                                <h4 class="text-sm font-medium">"OAuth Configuration"</h4>
                                <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium mb-1">"OAuth Client ID"</label>
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="From Snowflake OAuth integration"
                                            prop:value=move || cfg_oauth_client_id.get()
                                            on:input=move |ev| set_cfg_oauth_client_id.set(event_target_value(&ev))
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium mb-1">"OAuth Client Secret"</label>
                                        <input type="password" class=MODAL_INPUT_CLASS
                                            placeholder="OAuth client secret"
                                            prop:value=move || cfg_oauth_client_secret.get()
                                            on:input=move |ev| set_cfg_oauth_client_secret.set(event_target_value(&ev))
                                        />
                                    </div>
                                </div>
                            </div>
                        </Show>
                    </div>
                }.into_any(),

                "databricks" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Server Hostname " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="dbc-xxxxxxxx-xxxx.cloud.databricks.com"
                                prop:value=move || cfg_server_hostname.get()
                                on:input=move |ev| set_cfg_server_hostname.set(event_target_value(&ev))
                            />
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "HTTP Path " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="/sql/1.0/warehouses/xxxx"
                                prop:value=move || cfg_http_path.get()
                                on:input=move |ev| set_cfg_http_path.set(event_target_value(&ev))
                            />
                        </div>
                    </div>
                }.into_any(),

                "sqlserver" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Host " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="sqlserver.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder="1433"
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="master"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <label class="flex items-center gap-2 cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4 rounded-md border-input"
                                    prop:checked=move || cfg_encrypt.get()
                                    on:change=move |ev| {
                                        set_cfg_encrypt.set(event_target_checked(&ev));
                                    }
                                />
                                <span class="text-sm">"Encrypt Connection"</span>
                            </label>
                            <label class="flex items-center gap-2 cursor-pointer">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4 rounded-md border-input"
                                    prop:checked=move || cfg_trust_cert.get()
                                    on:change=move |ev| {
                                        set_cfg_trust_cert.set(event_target_checked(&ev));
                                    }
                                />
                                <span class="text-sm">"Trust Server Certificate"</span>
                            </label>
                        </div>
                    </div>
                }.into_any(),

                "synapse" => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Server " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="my-workspace.sql.azuresynapse.net"
                                prop:value=move || cfg_host.get()
                                on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                            />
                            <p class="text-xs text-muted-foreground mt-1">
                                "Synapse workspace SQL endpoint"
                            </p>
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"Database"</label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="master"
                                prop:value=move || cfg_database.get()
                                on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                            />
                        </div>
                        // Tenant ID — required for Service Principal and Enterprise OAuth
                        <Show when=move || {
                            let syn = synapse_auth_mode.get();
                            syn == "service_principal" || syn == "enterprise_oauth"
                        }>
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Azure AD Tenant ID"
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                                    prop:value=move || cfg_tenant_id.get()
                                    on:input=move |ev| set_cfg_tenant_id.set(event_target_value(&ev))
                                />
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Required for Microsoft OAuth. Find in Azure Portal → Directory ID."
                                </p>
                            </div>
                        </Show>
                    </div>
                }.into_any(),

                "bigquery" => view! {
                    // BigQuery has no connection fields — handled in BigQueryAuthModeSection
                    <div></div>
                }.into_any(),

                _ => view! {
                    <div class="space-y-4">
                        <h4 class="text-sm font-medium">"Connection Settings"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Host"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="host.example.com"
                                    prop:value=move || cfg_host.get()
                                    on:input=move |ev| set_cfg_host.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Port"</label>
                                <input type="number" class=MODAL_INPUT_CLASS
                                    placeholder="5432"
                                    prop:value=move || cfg_port.get()
                                    on:input=move |ev| set_cfg_port.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    </div>
                }.into_any(),
            }
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SSH Tunnel Section
// ─────────────────────────────────────────────────────────────────────────────

/// Bundle of every signal `SshTunnelSection` needs. Follows the
/// `ConnectionFieldsSignals` convention — signals (and `Action`, which is
/// also `Copy`) packed as a single prop.
#[derive(Clone, Copy)]
struct SshTunnelSignals {
    cfg_ssh_enabled: ReadSignal<bool>,
    set_cfg_ssh_enabled: WriteSignal<bool>,
    cfg_ssh_host: ReadSignal<String>,
    set_cfg_ssh_host: WriteSignal<String>,
    cfg_ssh_port: ReadSignal<String>,
    set_cfg_ssh_port: WriteSignal<String>,
    cfg_ssh_username: ReadSignal<String>,
    set_cfg_ssh_username: WriteSignal<String>,
    ssh_public_key: ReadSignal<Option<String>>,
    set_ssh_public_key: WriteSignal<Option<String>>,
    set_ssh_private_key_generated: WriteSignal<Option<String>>,
    ssh_key_generating: ReadSignal<bool>,
    ssh_key_action: Action<(), Result<GeneratedSshKey, ServerFnError>>,
    cfg_ssh_host_fingerprint: ReadSignal<String>,
    set_cfg_ssh_host_fingerprint: WriteSignal<String>,
    cfg_ssh_key_mode: ReadSignal<String>,
    set_cfg_ssh_key_mode: WriteSignal<String>,
    cfg_ssh_private_key_input: ReadSignal<String>,
    set_cfg_ssh_private_key_input: WriteSignal<String>,
    cfg_ssh_passphrase: ReadSignal<String>,
    set_cfg_ssh_passphrase: WriteSignal<String>,
    /// Whether the modal is editing an existing datasource (vs. creating a
    /// new one) — drives the BYOK fields' "leave blank to keep the existing
    /// key" placeholder, which only makes sense once a key already exists.
    is_edit_mode: Signal<bool>,
}

/// Renders the "Connect via SSH Tunnel" checkbox, host/port/username fields,
/// and keypair generation/display. Ports the removed React
/// `renderSSHTunnelSection` (`DatasourceModal.jsx`) plus the keygen UX added
/// for KYO-125. Caller gates this on `is_admin && supports_ssh_tunnel(...)`.
#[component]
fn SshTunnelSection(signals: SshTunnelSignals) -> impl IntoView {
    let SshTunnelSignals {
        cfg_ssh_enabled,
        set_cfg_ssh_enabled,
        cfg_ssh_host,
        set_cfg_ssh_host,
        cfg_ssh_port,
        set_cfg_ssh_port,
        cfg_ssh_username,
        set_cfg_ssh_username,
        ssh_public_key,
        set_ssh_public_key,
        set_ssh_private_key_generated,
        ssh_key_generating,
        ssh_key_action,
        cfg_ssh_host_fingerprint,
        set_cfg_ssh_host_fingerprint,
        cfg_ssh_key_mode,
        set_cfg_ssh_key_mode,
        cfg_ssh_private_key_input,
        set_cfg_ssh_private_key_input,
        cfg_ssh_passphrase,
        set_cfg_ssh_passphrase,
        is_edit_mode,
    } = signals;

    // Signal::derive created outside the <Show> below so the child
    // component it's passed to (`SshPublicKeyDisplay`) is created once and
    // reactively updates, rather than being read (and thus destroyed/
    // recreated) from inside the `<Show>` children closure — see
    // CODING_STANDARDS.md "Never call .get() on signals inside <Show>
    // children".
    let public_key_display = Signal::derive(move || ssh_public_key.get().unwrap_or_default());

    let handle_toggle = move |ev: leptos::ev::Event| {
        let checked = event_target_checked(&ev);
        set_cfg_ssh_enabled.set(checked);
        if !checked {
            set_cfg_ssh_host.set(String::new());
            set_cfg_ssh_port.set("22".to_string());
            set_cfg_ssh_username.set(String::new());
            set_ssh_public_key.set(None);
            set_ssh_private_key_generated.set(None);
            set_cfg_ssh_host_fingerprint.set(String::new());
            set_cfg_ssh_key_mode.set("generate".to_string());
            set_cfg_ssh_private_key_input.set(String::new());
            set_cfg_ssh_passphrase.set(String::new());
        }
    };

    // Manual fallback for the auto-generate Effect (e.g. if the automatic
    // dispatch raced a disabled→enabled toggle) — guarded the same way as
    // every other Action dispatch in this modal (double-dispatch guard).
    let handle_generate = move |_| {
        if !ssh_key_action.pending().get_untracked() {
            ssh_key_action.dispatch(());
        }
    };

    view! {
        <div class="border-t border-border pt-4 mt-4">
            <label class="flex items-start gap-2 cursor-pointer">
                <input
                    type="checkbox"
                    class="h-4 w-4 rounded-md border-input mt-0.5"
                    prop:checked=move || cfg_ssh_enabled.get()
                    on:change=handle_toggle
                />
                <span>
                    <span class="text-sm font-medium block">"Connect via SSH Tunnel"</span>
                    <span class="text-xs text-muted-foreground">
                        "Use a bastion host to reach the database behind a firewall"
                    </span>
                </span>
            </label>

            <Show when=move || cfg_ssh_enabled.get()>
                <div class="mt-4 space-y-4">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "SSH Host " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="bastion.example.com"
                                prop:value=move || cfg_ssh_host.get()
                                on:input=move |ev| set_cfg_ssh_host.set(event_target_value(&ev))
                            />
                        </div>
                        <div>
                            <label class="block text-sm font-medium mb-1">"SSH Port"</label>
                            <input type="number" class=MODAL_INPUT_CLASS
                                placeholder="22"
                                prop:value=move || cfg_ssh_port.get()
                                on:input=move |ev| set_cfg_ssh_port.set(event_target_value(&ev))
                            />
                        </div>
                        <div class="sm:col-span-2">
                            <label class="block text-sm font-medium mb-1">
                                "SSH Username " <span class="text-error-foreground">"*"</span>
                            </label>
                            <input type="text" class=MODAL_INPUT_CLASS
                                placeholder="ssh_user"
                                prop:value=move || cfg_ssh_username.get()
                                on:input=move |ev| set_cfg_ssh_username.set(event_target_value(&ev))
                            />
                        </div>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-1">
                            "SSH Host Fingerprint (optional)"
                        </label>
                        <input type="text" class=MODAL_INPUT_CLASS
                            placeholder="SHA256:..."
                            prop:value=move || cfg_ssh_host_fingerprint.get()
                            on:input=move |ev| set_cfg_ssh_host_fingerprint.set(event_target_value(&ev))
                        />
                        <p class="text-xs text-muted-foreground mt-1">
                            "Pin the bastion's host key to prevent man-in-the-middle. Get it with "
                            <code class="font-mono">"ssh-keygen -lf"</code>
                            "."
                        </p>
                    </div>

                    <div>
                        <label class="block text-sm font-medium mb-2">"SSH Key"</label>
                        <div class="inline-flex bg-muted p-1 rounded-lg gap-1 mb-3">
                            <ToggleButton
                                variant=Signal::derive(move || {
                                    if cfg_ssh_key_mode.get() == "generate" {
                                        ButtonVariant::PillActive
                                    } else {
                                        ButtonVariant::Pill
                                    }
                                })
                                size=ButtonSize::Pill
                                on:click=move |_| set_cfg_ssh_key_mode.set("generate".to_string())
                            >
                                "Generate a key for me"
                            </ToggleButton>
                            <ToggleButton
                                variant=Signal::derive(move || {
                                    if cfg_ssh_key_mode.get() == "byok" {
                                        ButtonVariant::PillActive
                                    } else {
                                        ButtonVariant::Pill
                                    }
                                })
                                size=ButtonSize::Pill
                                on:click=move |_| set_cfg_ssh_key_mode.set("byok".to_string())
                            >
                                "Use my own key"
                            </ToggleButton>
                        </div>

                        <Show when=move || cfg_ssh_key_mode.get() == "generate">
                            <Show
                                when=move || ssh_public_key.get().is_some()
                                fallback=move || view! {
                                    <div class="space-y-2">
                                        <p class="text-xs text-muted-foreground">
                                            "Kyomi needs an SSH keypair to authenticate with the bastion host."
                                        </p>
                                        <Button
                                            variant=ButtonVariant::Secondary
                                            size=ButtonSize::Sm
                                            disabled=Signal::derive(move || ssh_key_generating.get())
                                            on:click=handle_generate
                                        >
                                            {move || {
                                                if ssh_key_generating.get() {
                                                    "Generating..."
                                                } else {
                                                    "Generate SSH key"
                                                }
                                            }}
                                        </Button>
                                    </div>
                                }
                            >
                                <SshPublicKeyDisplay public_key=public_key_display/>
                            </Show>
                        </Show>

                        <Show when=move || cfg_ssh_key_mode.get() == "byok">
                            <div class="space-y-3">
                                <div>
                                    <label class="block text-sm font-medium mb-1">
                                        "Private Key " <span class="text-error-foreground">"*"</span>
                                    </label>
                                    <textarea
                                        rows="6"
                                        class="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono focus:outline-none focus:ring-1 focus:ring-ring"
                                        prop:placeholder=move || {
                                            if is_edit_mode.get() {
                                                "Leave blank to keep the existing key"
                                            } else {
                                                "-----BEGIN OPENSSH PRIVATE KEY-----"
                                            }
                                        }
                                        prop:value=move || cfg_ssh_private_key_input.get()
                                        on:input=move |ev| set_cfg_ssh_private_key_input.set(event_target_value(&ev))
                                    />
                                    <p class="text-xs text-muted-foreground mt-1">
                                        "Paste an unencrypted or passphrase-protected OpenSSH/PEM private key."
                                    </p>
                                </div>
                                <div>
                                    <label class="block text-sm font-medium mb-1">"Passphrase"</label>
                                    <input type="password" class=MODAL_INPUT_CLASS
                                        prop:placeholder=move || {
                                            if is_edit_mode.get() {
                                                "Leave blank to keep the existing key"
                                            } else {
                                                ""
                                            }
                                        }
                                        prop:value=move || cfg_ssh_passphrase.get()
                                        on:input=move |ev| set_cfg_ssh_passphrase.set(event_target_value(&ev))
                                    />
                                    <p class="text-xs text-muted-foreground mt-1">
                                        "Only if your private key is encrypted."
                                    </p>
                                </div>
                            </div>
                        </Show>
                    </div>
                </div>
            </Show>
        </div>
    }
}

/// Public-key display box for the SSH Tunnel section — a plain component
/// (not an inline `{move || ...}` branch) so `<Show>` can mount/unmount it
/// safely without the `.get()`-inside-children-closure anti-pattern (it owns
/// a `<CopyButton>`, which has its own internal reactive state).
#[component]
fn SshPublicKeyDisplay(public_key: Signal<String>) -> impl IntoView {
    view! {
        <div class="bg-muted/30 rounded-lg p-3 space-y-2">
            <div class="flex items-start justify-between gap-2">
                <code class="font-mono text-xs break-all flex-1">{move || public_key.get()}</code>
                <CopyButton text=public_key/>
            </div>
            <p class="text-xs text-muted-foreground">
                "Add this public key to the bastion server's "
                <code class="font-mono">"authorized_keys"</code>
                " file."
            </p>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Provider Credentials Fields
// ─────────────────────────────────────────────────────────────────────────────

/// Bundle of every signal `ProviderCredentialsFields` needs.
#[derive(Clone, Copy)]
struct CredentialsFieldsSignals {
    ds_type: ReadSignal<String>,
    sf_auth_mode: ReadSignal<String>,
    bq_auth_mode: ReadSignal<String>,
    db_auth_mode: ReadSignal<String>,
    synapse_auth_mode: ReadSignal<String>,
    cred_username: ReadSignal<String>,
    set_cred_username: WriteSignal<String>,
    cred_password: ReadSignal<String>,
    set_cred_password: WriteSignal<String>,
    cred_password_stored: ReadSignal<bool>,
    cred_access_token: ReadSignal<String>,
    set_cred_access_token: WriteSignal<String>,
    cred_private_key: ReadSignal<String>,
    set_cred_private_key: WriteSignal<String>,
    cred_sp_client_id: ReadSignal<String>,
    set_cred_sp_client_id: WriteSignal<String>,
    cred_sp_client_secret: ReadSignal<String>,
    set_cred_sp_client_secret: WriteSignal<String>,
    cfg_shared_credentials: ReadSignal<bool>,
    set_cfg_shared_credentials: WriteSignal<bool>,
    /// Gates the "Shared credentials (all users)" toggle — enabling/rotating
    /// shared credentials is workspace-admin-only (KYO-184; see
    /// `docs/DATASOURCE_ARCHITECTURE.md` §5.2). Does not affect the personal
    /// credential fields below it, which every member can use.
    is_admin: Signal<bool>,
}

/// Renders credentials fields for the active provider.
/// Skipped for BigQuery (handled in BigQueryAuthModeSection) and Snowflake OAuth.
#[component]
fn ProviderCredentialsFields(signals: CredentialsFieldsSignals) -> impl IntoView {
    let CredentialsFieldsSignals {
        ds_type,
        sf_auth_mode,
        bq_auth_mode,
        db_auth_mode,
        synapse_auth_mode,
        cred_username,
        set_cred_username,
        cred_password,
        set_cred_password,
        cred_password_stored,
        cred_access_token,
        set_cred_access_token,
        cred_private_key,
        set_cred_private_key,
        cred_sp_client_id,
        set_cred_sp_client_id,
        cred_sp_client_secret,
        set_cred_sp_client_secret,
        cfg_shared_credentials,
        set_cfg_shared_credentials,
        is_admin,
    } = signals;
    view! {
        {move || {
            let t = ds_type.get();
            let sf = sf_auth_mode.get();
            let db = db_auth_mode.get();
            let _bq = bq_auth_mode.get();
            let syn = synapse_auth_mode.get();

            // BigQuery is handled entirely in BigQueryAuthModeSection
            if t == "bigquery" {
                return view! { <div></div> }.into_any();
            }

            // Snowflake OAuth — no password fields shown
            if t == "snowflake" && sf == "oauth" {
                return view! { <div></div> }.into_any();
            }

            // Databricks OAuth — no access token fields shown
            if t == "databricks" && db == "oauth" {
                return view! { <div></div> }.into_any();
            }

            // Synapse Enterprise OAuth — credentials handled via OAuth popup
            if t == "synapse" && syn == "enterprise_oauth" {
                return view! { <div></div> }.into_any();
            }

            // Synapse Service Principal — show SP credentials section
            if t == "synapse" && syn == "service_principal" {
                return view! {
                    <div class="space-y-4 border-t border-border pt-4 mt-4">
                        <h4 class="text-sm font-medium">"Service Principal Credentials"</h4>
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Client ID " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="Application (client) ID"
                                    prop:value=move || cred_sp_client_id.get()
                                    on:input=move |ev| set_cred_sp_client_id.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Client Secret " <span class="text-error-foreground">"*"</span>
                                </label>
                                <input type="password" class=MODAL_INPUT_CLASS
                                    placeholder="Client secret value"
                                    prop:value=move || cred_sp_client_secret.get()
                                    on:input=move |ev| set_cred_sp_client_secret.set(event_target_value(&ev))
                                />
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Credentials are encrypted at rest"
                                </p>
                            </div>
                        </div>
                    </div>
                }.into_any();
            }

            view! {
                <div class="space-y-4 border-t border-border pt-4 mt-4">
                    <div class="flex items-center justify-between">
                        <h4 class="text-sm font-medium">"Credentials"</h4>
                        // Shared credentials toggle — workspace-admin-only
                        // (KYO-184): enabling/rotating shared credentials persists
                        // through `update_datasource_settings`, which non-admins
                        // cannot call, and grants every workspace member query
                        // access under this identity (DATASOURCE_ARCHITECTURE.md
                        // §5.2/§5.3) — not something a member should be able to
                        // toggle even cosmetically.
                        <Show when=move || is_admin.get()>
                            <label class="flex items-center gap-2 cursor-pointer text-xs text-muted-foreground">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4 rounded-md border-input"
                                    prop:checked=move || cfg_shared_credentials.get()
                                    on:change=move |ev| {
                                        set_cfg_shared_credentials.set(event_target_checked(&ev));
                                    }
                                />
                                "Shared credentials (all users)"
                            </label>
                        </Show>
                    </div>

                    <Show when=move || !cfg_shared_credentials.get()>
                        {move || {
                            let t2 = ds_type.get();
                            let sf2 = sf_auth_mode.get();

                            if t2 == "databricks" {
                                return view! {
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Personal Access Token " <span class="text-error-foreground">"*"</span>
                                        </label>
                                        <input type="password" class=MODAL_INPUT_CLASS
                                            placeholder="dapi..."
                                            prop:value=move || cred_access_token.get()
                                            on:input=move |ev| set_cred_access_token.set(event_target_value(&ev))
                                        />
                                        <p class="text-xs text-muted-foreground mt-1">
                                            "Credentials are encrypted at rest"
                                        </p>
                                    </div>
                                }.into_any();
                            }

                            if t2 == "snowflake" && sf2 == "keypair" {
                                return view! {
                                    <div class="space-y-3">
                                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                            <div>
                                                <label class="block text-sm font-medium mb-1">
                                                    "Username " <span class="text-error-foreground">"*"</span>
                                                </label>
                                                <input type="text" class=MODAL_INPUT_CLASS
                                                    placeholder="your_username"
                                                    prop:value=move || cred_username.get()
                                                    on:input=move |ev| set_cred_username.set(event_target_value(&ev))
                                                />
                                            </div>
                                            <div>
                                                <label class="block text-sm font-medium mb-1">"Private Key Passphrase"</label>
                                                <input type="password" class=MODAL_INPUT_CLASS
                                                    placeholder="If encrypted"
                                                    prop:value=move || cred_password.get()
                                                    on:input=move |ev| set_cred_password.set(event_target_value(&ev))
                                                />
                                            </div>
                                        </div>
                                        <div>
                                            <label class="block text-sm font-medium mb-1">
                                                "Private Key (PEM) " <span class="text-error-foreground">"*"</span>
                                            </label>
                                            <textarea
                                                rows="4"
                                                class="w-full px-3 py-2 border border-input rounded-md bg-background text-sm font-mono focus:outline-none focus:ring-1 focus:ring-ring"
                                                placeholder="-----BEGIN PRIVATE KEY-----\n...\n-----END PRIVATE KEY-----"
                                                prop:value=move || cred_private_key.get()
                                                on:input=move |ev| set_cred_private_key.set(event_target_value(&ev))
                                            />
                                            <p class="text-xs text-muted-foreground mt-1">
                                                "Credentials are encrypted at rest"
                                            </p>
                                        </div>
                                    </div>
                                }.into_any();
                            }

                            // Default: username + password
                            view! {
                                <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Username " <span class="text-error-foreground">"*"</span>
                                        </label>
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="Database username"
                                            prop:value=move || cred_username.get()
                                            on:input=move |ev| set_cred_username.set(event_target_value(&ev))
                                        />
                                    </div>
                                    <div>
                                        <label class="block text-sm font-medium mb-1">
                                            "Password " <span class="text-error-foreground">"*"</span>
                                        </label>
                                        <input type="password" class=MODAL_INPUT_CLASS
                                            prop:placeholder=move || {
                                                if cred_password_stored.get() && cred_password.get().is_empty() {
                                                    "•••••••• (stored)"
                                                } else {
                                                    "••••••••"
                                                }
                                            }
                                            prop:value=move || cred_password.get()
                                            on:input=move |ev| set_cred_password.set(event_target_value(&ev))
                                        />
                                        <p class="text-xs text-muted-foreground mt-1">
                                            "Credentials are encrypted at rest"
                                        </p>
                                    </div>
                                </div>
                            }.into_any()
                        }}
                    </Show>

                    <Show when=move || cfg_shared_credentials.get()>
                        <div class="flex items-center gap-2 p-3 bg-muted/50 rounded-lg">
                            <Icon icon=phosphor_leptos::LOCK attr:class="h-4 w-4 text-muted-foreground"/>
                            <span class="text-sm text-muted-foreground">
                                "Shared credentials — all users connect with the same account"
                            </span>
                        </div>
                    </Show>
                </div>
            }.into_any()
        }}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Discovery Fields
// ─────────────────────────────────────────────────────────────────────────────

/// Renders the discovery fields (database/schema/warehouse/catalog) for the active provider.
/// Bundle of every signal `DiscoveryFields` needs.
#[derive(Clone, Copy)]
struct DiscoveryFieldsSignals {
    ds_type: ReadSignal<String>,
    discovery_succeeded: Signal<bool>,
    discovered_databases: ReadSignal<Vec<String>>,
    discovered_schemas: ReadSignal<Vec<String>>,
    discovered_warehouses: ReadSignal<Vec<String>>,
    discovered_catalogs: ReadSignal<Vec<String>>,
    cfg_database: ReadSignal<String>,
    set_cfg_database: WriteSignal<String>,
    cfg_schema: ReadSignal<String>,
    set_cfg_schema: WriteSignal<String>,
    cfg_warehouse: ReadSignal<String>,
    set_cfg_warehouse: WriteSignal<String>,
    cfg_catalog: ReadSignal<String>,
    set_cfg_catalog: WriteSignal<String>,
    cfg_role: ReadSignal<String>,
    set_cfg_role: WriteSignal<String>,
}

/// In create mode after successful Test & Discover, shows dropdowns.
/// In edit mode, shows text inputs pre-filled from saved config.
#[component]
fn DiscoveryFields(signals: DiscoveryFieldsSignals) -> impl IntoView {
    let DiscoveryFieldsSignals {
        ds_type,
        discovery_succeeded,
        discovered_databases,
        discovered_schemas,
        discovered_warehouses,
        discovered_catalogs,
        cfg_database,
        set_cfg_database,
        cfg_schema,
        set_cfg_schema,
        cfg_warehouse,
        set_cfg_warehouse,
        cfg_catalog,
        set_cfg_catalog,
        cfg_role,
        set_cfg_role,
    } = signals;
    view! {
        <div class="border-t border-border pt-4 mt-4">
            <h4 class="text-sm font-medium mb-3">
                {move || if discovery_succeeded.get() { "Select Resources" } else { "Resource Configuration" }}
            </h4>

            {move || {
                let t = ds_type.get();
                let succeeded = discovery_succeeded.get();

                match t.as_str() {
                    "postgres" | "redshift" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Database " <span class="text-error-foreground">"*"</span>
                                </label>
                                {if succeeded && !discovered_databases.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_database.get())
                                            options=Signal::derive(move || {
                                                discovered_databases.get().into_iter()
                                                    .map(|db| (db.clone(), db))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_database.set(val)
                                            placeholder="Select database..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="mydb"
                                            prop:value=move || cfg_database.get()
                                            on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                {if succeeded && !discovered_schemas.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_schema.get())
                                            options=Signal::derive(move || {
                                                discovered_schemas.get().into_iter()
                                                    .map(|s| (s.clone(), s))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_schema.set(val)
                                            placeholder="Select schema..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="public"
                                            prop:value=move || cfg_schema.get()
                                            on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Default schema for queries (usually \"public\")"
                                </p>
                            </div>
                        </div>
                    }.into_any(),

                    "clickhouse" | "mysql" => view! {
                        <div>
                            <label class="block text-sm font-medium mb-1">
                                "Default Database " <span class="text-error-foreground">"*"</span>
                            </label>
                            {if succeeded && !discovered_databases.get().is_empty() {
                                view! {
                                    <Select
                                        value=Signal::derive(move || cfg_database.get())
                                        options=Signal::derive(move || {
                                            discovered_databases.get().into_iter()
                                                .map(|db| (db.clone(), db))
                                                .collect()
                                        })
                                        on_change=move |val| set_cfg_database.set(val)
                                        placeholder="Select database..."
                                    />
                                }.into_any()
                            } else {
                                view! {
                                    <input type="text" class=MODAL_INPUT_CLASS
                                        placeholder="default"
                                        prop:value=move || cfg_database.get()
                                        on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                    />
                                }.into_any()
                            }}
                            <p class="text-xs text-muted-foreground mt-1">"Default database for queries"</p>
                        </div>
                    }.into_any(),

                    "snowflake" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Warehouse"</label>
                                {if succeeded && !discovered_warehouses.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_warehouse.get())
                                            options=Signal::derive(move || {
                                                discovered_warehouses.get().into_iter()
                                                    .map(|w| (w.clone(), w))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_warehouse.set(val)
                                            placeholder="Select warehouse..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="COMPUTE_WH"
                                            prop:value=move || cfg_warehouse.get()
                                            on:input=move |ev| set_cfg_warehouse.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">"Default warehouse for compute"</p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Database"</label>
                                {if succeeded && !discovered_databases.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_database.get())
                                            options=Signal::derive(move || {
                                                discovered_databases.get().into_iter()
                                                    .map(|db| (db.clone(), db))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_database.set(val)
                                            placeholder="Select database..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="MY_DATABASE"
                                            prop:value=move || cfg_database.get()
                                            on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="PUBLIC"
                                    prop:value=move || cfg_schema.get()
                                    on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                />
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Role"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="ACCOUNTADMIN"
                                    prop:value=move || cfg_role.get()
                                    on:input=move |ev| set_cfg_role.set(event_target_value(&ev))
                                />
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Snowflake role to use (leave empty for default)"
                                </p>
                            </div>
                        </div>
                    }.into_any(),

                    "databricks" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">"Catalog"</label>
                                {if succeeded && !discovered_catalogs.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_catalog.get())
                                            options=Signal::derive(move || {
                                                discovered_catalogs.get().into_iter()
                                                    .map(|c| (c.clone(), c))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_catalog.set(val)
                                            placeholder="Select catalog..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="hive_metastore"
                                            prop:value=move || cfg_catalog.get()
                                            on:input=move |ev| set_cfg_catalog.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">"Unity Catalog or hive_metastore"</p>
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                <input type="text" class=MODAL_INPUT_CLASS
                                    placeholder="default"
                                    prop:value=move || cfg_schema.get()
                                    on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                />
                            </div>
                        </div>
                    }.into_any(),

                    "sqlserver" | "synapse" => view! {
                        <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                            <div>
                                <label class="block text-sm font-medium mb-1">
                                    "Database " <span class="text-error-foreground">"*"</span>
                                </label>
                                {if succeeded && !discovered_databases.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_database.get())
                                            options=Signal::derive(move || {
                                                discovered_databases.get().into_iter()
                                                    .map(|db| (db.clone(), db))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_database.set(val)
                                            placeholder="Select database..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="master"
                                            prop:value=move || cfg_database.get()
                                            on:input=move |ev| set_cfg_database.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                            </div>
                            <div>
                                <label class="block text-sm font-medium mb-1">"Default Schema"</label>
                                {if succeeded && !discovered_schemas.get().is_empty() {
                                    view! {
                                        <Select
                                            value=Signal::derive(move || cfg_schema.get())
                                            options=Signal::derive(move || {
                                                discovered_schemas.get().into_iter()
                                                    .map(|s| (s.clone(), s))
                                                    .collect()
                                            })
                                            on_change=move |val| set_cfg_schema.set(val)
                                            placeholder="Select schema..."
                                        />
                                    }.into_any()
                                } else {
                                    view! {
                                        <input type="text" class=MODAL_INPUT_CLASS
                                            placeholder="dbo"
                                            prop:value=move || cfg_schema.get()
                                            on:input=move |ev| set_cfg_schema.set(event_target_value(&ev))
                                        />
                                    }.into_any()
                                }}
                                <p class="text-xs text-muted-foreground mt-1">
                                    "Default schema for queries (usually \"dbo\")"
                                </p>
                            </div>
                        </div>
                    }.into_any(),

                    _ => view! { <div></div> }.into_any(),
                }
            }}
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Create-Mode Catalog Picker
// ─────────────────────────────────────────────────────────────────────────────

/// Returns the discovered items relevant to catalog scope for a given datasource
/// type, choosing from the three discovery buckets.
fn catalog_items_for_type<'a>(
    ds_type: &str,
    databases: &'a [String],
    schemas: &'a [String],
    catalogs: &'a [String],
) -> &'a [String] {
    match ds_type {
        "postgres" | "redshift" | "sqlserver" | "synapse" | "flaredb" => schemas,
        "databricks" => catalogs,
        _ => databases,
    }
}

/// True when `CreateModeCatalogPicker` should render its original,
/// `available_items`-driven branch (the generic checkbox-list-or-text-input
/// pair that predates KYO-468) rather than [`BqCreateModeProjectPicker`].
///
/// That's every non-BigQuery type unconditionally — `available_items`
/// already carries the right data for them — plus BigQuery itself unless
/// *both*:
///
/// * `bq_auth_mode` is one of the two modes that ever populate
///   `bq_projects` (`"kyomi_oauth"` or `"service_account"` — see the
///   writers of `set_bq_projects`/`set_bq_projects_attempted` above).
///   `"enterprise_oauth"`'s per-datasource organizational token can't list
///   personal GCP projects and has no discovery button of its own, so it
///   must never satisfy this regardless of `bq_projects_attempted`.
/// * `bq_projects_attempted` is `true` for that mode.
///
/// Checking `bq_auth_mode` here — not just at the signals' reset sites — is
/// the KYO-468 leak fix: `bq_projects_attempted`/`bq_projects` alone can't
/// tell "genuinely attempted under the *current* mode" apart from "still
/// true from a populating mode the user switched away from earlier in the
/// same session" (e.g. `service_account` validates, then the user switches
/// to `enterprise_oauth` — the auth-mode `on_change` resets the three
/// signals, but a predicate that ignored `bq_auth_mode` would still be one
/// missed future reset site away from rendering stale data). Once both
/// conditions hold, this returns `false` and `CreateModeCatalogPicker`
/// renders `BqCreateModeProjectPicker` instead, sourced from `bq_projects`
/// rather than `available_items` (which never carries BigQuery data — see
/// `catalog_items_for_type`'s `_ => databases` fallthrough).
///
/// Extracted as a plain function — rather than left as an inline `when=`
/// closure — so this routing decision, the crux of the KYO-468 fix, can be
/// asserted directly by value instead of only by source-text inspection
/// like the rest of this view-tree file.
fn create_mode_catalog_uses_generic_picker(
    ds_type: &str,
    bq_auth_mode: &str,
    bq_projects_attempted: bool,
) -> bool {
    if ds_type != "bigquery" {
        return true;
    }
    let mode_populates_bq_projects = matches!(bq_auth_mode, "kyomi_oauth" | "service_account");
    !mode_populates_bq_projects || !bq_projects_attempted
}

/// Shared checkbox-list body for catalog scope selection: Select All /
/// Clear controls, a live "N of M selected" count, and a scrollable list
/// of checkboxes over `(value, label)` pairs. `value` is what gets
/// written into `set_selected` (and ultimately persisted as catalog
/// scope); `label` is what's shown to the user.
///
/// Used both by the generic database/schema/catalog list — where
/// `value == label`, an ordinary discovered name — and by BigQuery's
/// discovered-project checkboxes (KYO-468), where `value` is the project
/// id (what catalog scope is persisted as) and `label` is `bq_projects`'s
/// own "name (project_id)" / bare-id convention, already built by its
/// producers (the kyomi_oauth post-connect fetch and the service_account
/// `test_action` Effect, both above). Extracted so BigQuery's picker
/// reuses the exact same affordances instead of a second, independently
/// maintained copy of this markup.
#[component]
fn CatalogItemCheckboxList(
    items: Signal<Vec<(String, String)>>,
    selected: ReadSignal<Vec<String>>,
    set_selected: WriteSignal<Vec<String>>,
) -> impl IntoView {
    view! {
        <div class="space-y-2">
            // Select all / Clear + count
            <div class="flex items-center gap-2">
                <button
                    type="button"
                    class="text-xs text-primary hover:underline"
                    on:click=move |_| {
                        set_selected.set(
                            items.get_untracked().into_iter().map(|(value, _)| value).collect(),
                        );
                    }
                >
                    "Select all"
                </button>
                <span class="text-xs text-muted-foreground">"·"</span>
                <button
                    type="button"
                    class="text-xs text-primary hover:underline"
                    on:click=move |_| {
                        set_selected.set(vec![]);
                    }
                >
                    "Clear"
                </button>
                <span class="text-xs text-muted-foreground ml-auto">
                    {move || {
                        let sel = selected.get().len();
                        let total = items.get().len();
                        if sel == 0 {
                            "all (leave unchecked to index everything)".to_string()
                        } else {
                            format!("{sel} of {total} selected")
                        }
                    }}
                </span>
            </div>
            // Scrollable checkbox list
            <div class="border border-border rounded-md divide-y divide-border max-h-60 overflow-y-auto">
                <For
                    each=move || items.get()
                    key=|(value, _)| value.clone()
                    let:item
                >
                    {
                        let (value, label) = item;
                        let value_for_change = value.clone();
                        let value_for_check = value.clone();
                        view! {
                            <label class="flex items-center gap-3 px-3 py-2 cursor-pointer hover:bg-muted/40 transition-colors">
                                <input
                                    type="checkbox"
                                    class="h-4 w-4 rounded border-input accent-primary"
                                    prop:checked=move || {
                                        selected.get().contains(&value_for_check)
                                    }
                                    on:change=move |ev| {
                                        let checked = event_target_checked(&ev);
                                        let val = value_for_change.clone();
                                        set_selected.update(|list| {
                                            if checked {
                                                if !list.contains(&val) {
                                                    list.push(val);
                                                }
                                            } else {
                                                list.retain(|i| i != &val);
                                            }
                                        });
                                    }
                                />
                                <span class="text-sm font-mono text-foreground">
                                    {label}
                                </span>
                            </label>
                        }
                    }
                </For>
            </div>
        </div>
    }
}

/// BigQuery's create-mode catalog-scope picker (KYO-468), rendered by
/// `CreateModeCatalogPicker` once a project-listing attempt has actually
/// been made (`bq_projects_attempted`) — the caller keeps rendering the
/// original `available_items`-driven fallback for the "never attempted"
/// state (enterprise_oauth), so this component only has to distinguish
/// the three states that follow an attempt:
///
/// * **in flight** (`bq_projects_loading`) — a "Discovering projects…"
///   indicator.
/// * **discovered N** — [`CatalogItemCheckboxList`], the same Select All
///   / Clear affordances as every other provider's checkbox list.
/// * **attempted, failed or genuinely empty** — a warning [`Alert`]
///   carrying `bq_projects_error`'s text when set, otherwise an explicit
///   "no projects found" message, either way alongside the manual-entry
///   input as fallback.
///
/// Structured as nested `<Show>`s rather than a `{move || match ... }`
/// branch, per this file's disposal-safety convention — see
/// `BqProjectField`'s doc comment above (KYO-500/KYO-429): a branch swap
/// inside a plain reactive closure destroys and recreates its subtree,
/// which panics if a child's `Effect` fires mid-teardown. `<Show>` mounts
/// and unmounts through the framework's own ownership tree instead.
#[component]
fn BqCreateModeProjectPicker(
    bq_projects: ReadSignal<Vec<(String, String)>>,
    bq_projects_loading: ReadSignal<bool>,
    bq_projects_error: ReadSignal<Option<String>>,
    catalog_selected: ReadSignal<Vec<String>>,
    set_catalog_selected: WriteSignal<Vec<String>>,
    catalog_text: ReadSignal<String>,
    set_catalog_text: WriteSignal<String>,
) -> impl IntoView {
    view! {
        <Show
            when=move || bq_projects_loading.get()
            fallback=move || view! {
                <Show
                    when=move || !bq_projects.get().is_empty()
                    fallback=move || view! {
                        <div class="space-y-1.5">
                            <Show
                                when=move || bq_projects_error.get().is_some()
                                fallback=move || view! {
                                    <p class="text-sm text-muted-foreground">
                                        "No projects found."
                                    </p>
                                }
                            >
                                <Alert variant=AlertVariant::Warning>
                                    <AlertDescription>
                                        {move || bq_projects_error.get().unwrap_or_default()}
                                        " You can still enter project IDs manually below."
                                    </AlertDescription>
                                </Alert>
                            </Show>
                            <input
                                type="text"
                                class=MODAL_INPUT_CLASS
                                placeholder="Enter project IDs, comma-separated"
                                prop:value=move || catalog_text.get()
                                on:input=move |ev| set_catalog_text.set(event_target_value(&ev))
                            />
                        </div>
                    }
                >
                    <CatalogItemCheckboxList
                        items=Signal::derive(move || bq_projects.get())
                        selected=catalog_selected
                        set_selected=set_catalog_selected
                    />
                </Show>
            }
        >
            <div class="flex items-center gap-2 text-sm text-muted-foreground py-2">
                <Spinner size="h-4 w-4"/>
                "Discovering projects…"
            </div>
        </Show>
    }
}

/// Create-mode catalog tab body.
///
/// The user has already run "Test & Discover" on the Connection tab, so the
/// three discovery signal buckets are already populated.  This component:
///
/// * When items are available — shows a checkbox list with Select All / Clear
///   controls so the user can narrow which schemas/databases/catalogs get
///   indexed on first run.
/// * When no items were discovered (pre-test fallback, or a denied/genuinely
///   empty listing) — shows a comma-separated text input as a manual
///   override.
/// * BigQuery only — shows the "Include Public Datasets" toggle, and (KYO-468)
///   drives its own item list from `bq_projects`/`bq_projects_loading`/
///   `bq_projects_error`/`bq_projects_attempted` via
///   [`BqCreateModeProjectPicker`] once a project-listing attempt has
///   actually been made — `available_items` below never carries BigQuery
///   data (see `catalog_items_for_type`'s `_ => databases` fallthrough,
///   the exact bug this ticket fixed: the Catalog tab was reachable, but
///   silently rendered only the manual-entry input regardless of what had
///   already been discovered). Before an attempt has been made
///   (`enterprise_oauth`, which deliberately never lists projects — see
///   the OAuth-connect Effect's comment), BigQuery still falls through to
///   the same `available_items`-driven text-input branch as every other
///   type, unchanged.
///
/// Header text uses `catalog_item_label_for_type` from KYO-300 (no duplication).
#[component]
fn CreateModeCatalogPicker(
    /// The datasource type string (e.g. `"bigquery"`, `"postgres"`).
    datasource_type: Signal<String>,
    /// Databases discovered during the Connection tab test.
    discovered_databases: ReadSignal<Vec<String>>,
    /// Schemas discovered during the Connection tab test.
    discovered_schemas: ReadSignal<Vec<String>>,
    /// Catalogs discovered during the Connection tab test.
    discovered_catalogs: ReadSignal<Vec<String>>,
    /// Currently selected catalog scope items.
    catalog_selected: ReadSignal<Vec<String>>,
    set_catalog_selected: WriteSignal<Vec<String>>,
    /// Comma-separated text fallback (used when no items were discovered).
    catalog_text: ReadSignal<String>,
    set_catalog_text: WriteSignal<String>,
    /// BigQuery only: include public datasets in catalog indexing.
    include_public_datasets: ReadSignal<bool>,
    set_include_public_datasets: WriteSignal<bool>,
    /// KYO-474: true when the last Test & Discover attempt succeeded but
    /// this type's catalog-scope key was denied (`resource_errors`,
    /// KYO-466) — read once by the caller and passed through here, never
    /// re-derived from `items.is_empty()` below, which is also true for
    /// "not attempted yet" and "succeeded, genuinely nothing there"
    /// (KYO-452 still owns the copy for both of those).
    catalog_discovery_denied: ReadSignal<bool>,
    /// BigQuery only (KYO-468): the account-level discovered project list
    /// — sourced from the modal's `bq_projects` signal (kyomi_oauth's
    /// post-connect fetch, or service_account's Test & Discover), never
    /// re-derived through `available_items`/`catalog_items_for_type`,
    /// which never carries BigQuery data.
    bq_projects: ReadSignal<Vec<(String, String)>>,
    /// True while the fetch above is in flight.
    bq_projects_loading: ReadSignal<bool>,
    /// Set when the fetch above failed or was denied.
    bq_projects_error: ReadSignal<Option<String>>,
    /// KYO-468: true once a BigQuery project-listing attempt has actually
    /// started/completed — never inferred from `bq_projects.is_empty()`,
    /// which is also true for "never attempted" (`enterprise_oauth`) and
    /// "attempted, genuinely empty". Computed once by the caller from the
    /// modal's own state, never re-derived here — same discipline as
    /// `catalog_discovery_denied` above.
    bq_projects_attempted: ReadSignal<bool>,
    /// BigQuery only: the currently selected Authentication Mode
    /// (`"kyomi_oauth"` / `"enterprise_oauth"` / `"service_account"`).
    /// KYO-468: `create_mode_catalog_uses_generic_picker` needs this
    /// alongside `bq_projects_attempted` — the auth-mode `on_change`
    /// handler resets `bq_projects_attempted` on every switch, but reading
    /// the current mode directly here means the routing decision itself
    /// stays correct even if some future teardown site misses that reset.
    bq_auth_mode: ReadSignal<String>,
) -> impl IntoView {
    // Derive the available items for the current type from the discovery
    // signals.  Recomputed reactively on type changes. Yields (value,
    // label) pairs — value == label for every type this derives items for
    // — so it can be handed directly to `CatalogItemCheckboxList` without
    // a second wrapping derive.
    let available_items = Signal::derive(move || {
        let ds_type = datasource_type.get();
        let dbs = discovered_databases.get();
        let schemas = discovered_schemas.get();
        let cats = discovered_catalogs.get();
        // We need owned Vecs — clone from whichever bucket is relevant.
        let items_ref = catalog_items_for_type(&ds_type, &dbs, &schemas, &cats);
        items_ref.iter().map(|v| (v.clone(), v.clone())).collect::<Vec<(String, String)>>()
    });

    view! {
        <div class="space-y-4">
            // Header
            {move || {
                let ds_type = datasource_type.get();
                let label = catalog_item_label_for_type(&ds_type);
                view! {
                    <div>
                        <h4 class="text-sm font-medium mb-1">"Catalog Scope"</h4>
                        <p class="text-sm text-muted-foreground">
                            "Select which "
                            {label}
                            " to include in the catalog."
                        </p>
                    </div>
                }
            }}

            // BigQuery: Include Public Datasets toggle
            <Show when=move || datasource_type.get() == "bigquery">
                <label class="flex items-center justify-between p-3 rounded-lg border border-border bg-muted/30 cursor-pointer">
                    <div>
                        <span class="text-sm font-medium text-foreground block">
                            "Include Public Datasets"
                        </span>
                        <span class="text-xs text-muted-foreground">
                            "Show BigQuery public datasets in search results"
                        </span>
                    </div>
                    <Switch
                        checked=Signal::from(include_public_datasets)
                        on_change=Callback::new(move |val: bool| {
                            set_include_public_datasets.set(val);
                        })
                    />
                </label>
            </Show>

            // Checkbox picker (when items were discovered) or text input
            // fallback. BigQuery only takes this branch before any
            // project-listing attempt has been made — once
            // `bq_projects_attempted` is true, it renders through
            // `BqCreateModeProjectPicker` below instead, sourced from
            // `bq_projects` rather than `available_items` (which never
            // carries BigQuery data — KYO-468).
            <Show
                when=move || {
                    create_mode_catalog_uses_generic_picker(
                        &datasource_type.get(),
                        &bq_auth_mode.get(),
                        bq_projects_attempted.get(),
                    )
                }
                fallback=move || view! {
                    <BqCreateModeProjectPicker
                        bq_projects=bq_projects
                        bq_projects_loading=bq_projects_loading
                        bq_projects_error=bq_projects_error
                        catalog_selected=catalog_selected
                        set_catalog_selected=set_catalog_selected
                        catalog_text=catalog_text
                        set_catalog_text=set_catalog_text
                    />
                }
            >
            {move || {
                let items = available_items.get();
                if items.is_empty() {
                    // No discovery results — render text input
                    let ds_type = datasource_type.get();
                    // Placeholder describes the field only — it must never promise
                    // an outcome discovery cannot guarantee (KYO-452). The
                    // qualified claim ("...this account can list") lives in the
                    // helper text below instead, driven by the same
                    // catalog_item_label_for_type noun used throughout this file.
                    let placeholder = match ds_type.as_str() {
                        "bigquery" => "Enter project IDs, comma-separated",
                        "clickhouse" | "mysql" | "snowflake" => "Enter database names, comma-separated",
                        "databricks" => "Enter catalog names, comma-separated",
                        _ => "Enter schema names, comma-separated",
                    };
                    let noun = catalog_item_label_for_type(&ds_type);
                    // KYO-474: a listing denial (`catalog_discovery_denied`,
                    // sourced from `resource_errors` — KYO-466) replaces the
                    // "leave blank" promise with a direct instruction, since
                    // that promise fails for a permission-limited account.
                    // "Not attempted yet" and "succeeded, genuinely empty"
                    // still fall through to the unchanged KYO-452 copy below
                    // — this must never be inferred from `items.is_empty()`.
                    let helper_text = if catalog_discovery_denied.get() {
                        format!("This account can't list {noun}. Enter the {noun} you want indexed.")
                    } else {
                        format!("Leave blank to index all {noun} this account can list.")
                    };
                    view! {
                        <div class="space-y-1.5">
                            <input
                                type="text"
                                class=MODAL_INPUT_CLASS
                                placeholder=placeholder
                                prop:value=move || catalog_text.get()
                                on:input=move |ev| set_catalog_text.set(event_target_value(&ev))
                            />
                            <p class="text-xs text-muted-foreground">
                                {helper_text}
                            </p>
                        </div>
                    }.into_any()
                } else {
                    // Discovery succeeded — checkbox list with Select All / Clear
                    view! {
                        <CatalogItemCheckboxList
                            items=available_items
                            selected=catalog_selected
                            set_selected=set_catalog_selected
                        />
                    }.into_any()
                }
            }}
            </Show>
        </div>
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Edit-Mode Catalog Tab
// ─────────────────────────────────────────────────────────────────────────────

fn view_service_account_form(
    json_signal: ReadSignal<String>,
    set_json_signal: WriteSignal<String>,
    set_unchanged: WriteSignal<bool>,
) -> leptos::prelude::AnyView {
    let has_json = Signal::derive(move || !json_signal.get().is_empty());

    let sa_email = Signal::derive(move || {
        let json_str = json_signal.get();
        if json_str.is_empty() {
            return None;
        }
        serde_json::from_str::<serde_json::Value>(&json_str)
            .ok()
            .and_then(|v| v.get("client_email")?.as_str().map(|s| s.to_string()))
    });

    view! {
        <div class="space-y-3">
            <Show
                when=move || has_json.get()
                fallback=move || view! {
                    <div class="space-y-3">
                        <div>
                            <label class="block text-sm font-medium text-foreground mb-1">"Service Account JSON"</label>
                            <textarea
                                class=format!("{} font-mono", MODAL_INPUT_CLASS)
                                rows=4
                                placeholder="{\"type\": \"service_account\", \"client_email\": \"...\"}"
                                prop:value=move || json_signal.get()
                                on:input=move |ev| {
                                    let text = event_target_value(&ev);
                                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
                                        && v.get("type").and_then(|t| t.as_str()) == Some("service_account")
                                        && v.get("client_email").and_then(|e| e.as_str()).is_some()
                                    {
                                        set_json_signal.set(text);
                                        set_unchanged.set(false);
                                    }
                                }
                            />
                        </div>
                        <p class="text-xs text-muted-foreground">
                            "Paste a Google Cloud service account JSON key. Must have type \"service_account\" and a client_email field."
                        </p>
                    </div>
                }
            >
                <div class="flex items-center justify-between p-3 bg-muted/50 rounded-lg">
                    <div class="flex items-center gap-2">
                        <Icon icon=phosphor_leptos::CHECK attr:class="h-4 w-4 text-success-foreground"/>
                        <span class="text-sm text-foreground">
                            {move || format!("Service Account: {}", sa_email.get().unwrap_or_else(|| "loaded".to_string()))}
                        </span>
                    </div>
                    <Button
                        variant=ButtonVariant::Outline
                        size=ButtonSize::Sm
                        on:click=move |_| {
                            set_json_signal.set(String::new());
                            set_unchanged.set(false);
                        }
                    >
                        "Remove"
                    </Button>
                </div>
            </Show>
        </div>
    }.into_any()
}

fn view_password_form(
    username: ReadSignal<String>,
    set_username: WriteSignal<String>,
    password: ReadSignal<String>,
    set_password: WriteSignal<String>,
    set_unchanged: WriteSignal<bool>,
) -> leptos::prelude::AnyView {
    view! {
        <div class="space-y-3">
            <div>
                <label class="block text-sm font-medium text-foreground mb-1">"Username"</label>
                <input
                    type="text"
                    class=MODAL_INPUT_CLASS
                    placeholder="e.g., readonly_indexer"
                    prop:value=move || username.get()
                    on:input=move |ev| {
                        set_username.set(event_target_value(&ev));
                        set_unchanged.set(false);
                    }
                />
            </div>
            <div>
                <label class="block text-sm font-medium text-foreground mb-1">"Password"</label>
                <input
                    type="password"
                    class=MODAL_INPUT_CLASS
                    placeholder="Enter password"
                    prop:value=move || password.get()
                    on:input=move |ev| {
                        set_password.set(event_target_value(&ev));
                        set_unchanged.set(false);
                    }
                />
            </div>
            <p class="text-xs text-muted-foreground">
                "These credentials will be used for catalog indexing only, not for user queries."
            </p>
        </div>
    }.into_any()
}

fn view_token_form(
    token: ReadSignal<String>,
    set_token: WriteSignal<String>,
    set_unchanged: WriteSignal<bool>,
) -> leptos::prelude::AnyView {
    view! {
        <div class="space-y-3">
            <div>
                <label class="block text-sm font-medium text-foreground mb-1">"Personal Access Token"</label>
                <input
                    type="password"
                    class=MODAL_INPUT_CLASS
                    placeholder="dapi..."
                    prop:value=move || token.get()
                    on:input=move |ev| {
                        set_token.set(event_target_value(&ev));
                        set_unchanged.set(false);
                    }
                />
            </div>
            <p class="text-xs text-muted-foreground">
                "Databricks Personal Access Token for catalog indexing."
            </p>
        </div>
    }.into_any()
}

fn view_service_principal_form(
    client_id: ReadSignal<String>,
    set_client_id: WriteSignal<String>,
    client_secret: ReadSignal<String>,
    set_client_secret: WriteSignal<String>,
    tenant_id: ReadSignal<String>,
    set_tenant_id: WriteSignal<String>,
    set_unchanged: WriteSignal<bool>,
) -> leptos::prelude::AnyView {
    view! {
        <div class="space-y-3">
            <div>
                <label class="block text-sm font-medium text-foreground mb-1">"Tenant ID"</label>
                <input
                    type="text"
                    class=MODAL_INPUT_CLASS
                    placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                    prop:value=move || tenant_id.get()
                    on:input=move |ev| {
                        set_tenant_id.set(event_target_value(&ev));
                        set_unchanged.set(false);
                    }
                />
            </div>
            <div>
                <label class="block text-sm font-medium text-foreground mb-1">"Client ID"</label>
                <input
                    type="text"
                    class=MODAL_INPUT_CLASS
                    placeholder="xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
                    prop:value=move || client_id.get()
                    on:input=move |ev| {
                        set_client_id.set(event_target_value(&ev));
                        set_unchanged.set(false);
                    }
                />
            </div>
            <div>
                <label class="block text-sm font-medium text-foreground mb-1">"Client Secret"</label>
                <input
                    type="password"
                    class=MODAL_INPUT_CLASS
                    placeholder="Enter client secret"
                    prop:value=move || client_secret.get()
                    on:input=move |ev| {
                        set_client_secret.set(event_target_value(&ev));
                        set_unchanged.set(false);
                    }
                />
            </div>
            <p class="text-xs text-muted-foreground">
                "Azure AD service principal credentials for catalog indexing."
            </p>
        </div>
    }.into_any()
}

/// Edit-mode catalog tab — stats card, Refresh Now button, and schema/database
/// picker.
///
/// Replaces `apps/frontend/src/components/settings/CatalogSection.jsx`.
#[component]
fn EditModeCatalogTab(
    datasource_id: Signal<String>,
    datasource_slug: Signal<String>,
    datasource_type: Signal<String>,
    connection_config: Signal<serde_json::Value>,
    credentials: Signal<serde_json::Value>,
    is_sample: ReadSignal<bool>,
    /// Whether this is a Connect datasource. The scope picker discovers
    /// containers through the live agent (`discover_connect_containers`) rather
    /// than dialing the database directly; the credentials section is hidden.
    is_connect: Signal<bool>,
    catalog_selected: ReadSignal<Vec<String>>,
    set_catalog_selected: WriteSignal<Vec<String>>,
    set_catalog_scope_touched: WriteSignal<bool>,
    bq_include_public: ReadSignal<bool>,
    set_bq_include_public: WriteSignal<bool>,
    use_indexing_credentials: ReadSignal<bool>,
    set_use_indexing_credentials: WriteSignal<bool>,
    indexing_creds_type: ReadSignal<String>,
    set_indexing_creds_type: WriteSignal<String>,
    indexing_creds_json: ReadSignal<String>,
    set_indexing_creds_json: WriteSignal<String>,
    indexing_username: ReadSignal<String>,
    set_indexing_username: WriteSignal<String>,
    indexing_password: ReadSignal<String>,
    set_indexing_password: WriteSignal<String>,
    indexing_token: ReadSignal<String>,
    set_indexing_token: WriteSignal<String>,
    indexing_client_id: ReadSignal<String>,
    set_indexing_client_id: WriteSignal<String>,
    indexing_client_secret: ReadSignal<String>,
    set_indexing_client_secret: WriteSignal<String>,
    indexing_tenant_id: ReadSignal<String>,
    set_indexing_tenant_id: WriteSignal<String>,
    set_indexing_creds_unchanged: WriteSignal<bool>,
) -> impl IntoView {
    // ── Datasource-type registry data (KYO-187) ────────────────────────────
    // Which auth modes the "Catalog Indexing Credentials" selector below
    // offers is registry-owned (`DatasourceTypeMetadata::indexing_auth_modes`),
    // not a client-side match on the type string — see
    // `get_datasource_types`. `use_query` is the shared list-query cache
    // (Layout-scoped, no deps), so this doesn't add a second ad-hoc fetch
    // mechanism: it's the same hook `DatasourcesPage` uses for the
    // datasource list itself.
    let datasource_types = use_query("datasource-types", || (), |_: ()| get_datasource_types());

    // ── Load catalog stats on mount ──────────────────────────────────────
    // Input: datasource_id
    let stats_action = Action::new(|ds_id: &String| {
        let ds_id = ds_id.clone();
        async move { get_catalog_stats(ds_id).await }
    });

    // Dispatch stats load once when the component first mounts.
    Effect::new(move |_| {
        let id = datasource_id.get();
        if !id.is_empty() {
            stats_action.dispatch(id);
        }
    });

    // Derived stats from the action result.
    let stats = Signal::derive(move || {
        stats_action.value().get().and_then(|r| r.ok())
    });

    // ── Refresh catalog ──────────────────────────────────────────────────
    // Input: datasource_slug — returned through result so the Effect can
    // distinguish which invocation finished (double-dispatch guard uses
    // `pending()`).
    let refresh_action = Action::new(|slug: &String| {
        let slug = slug.clone();
        async move {
            refresh_catalog(slug).await
        }
    });

    // ── Live refresh status (KYO-144) ─────────────────────────────────────
    // Catalog indexing now runs in the background (KYO-143) — the server
    // fn returns as soon as it has kicked off the refresh. `refresh_phase`
    // drives the "Refreshing..." indicator while a background run is (or
    // is believed to be) in flight. Declared unconditionally so the view
    // compiles on both targets; only ever written from the WASM poll loop.
    // `_set_refresh_phase` is only written from WASM poll code below; the
    // underscore prefix avoids an unused-variable warning on the native
    // (SSR) target, matching the `Ok(_msg)` convention already used in
    // this file for WASM-only-consumed Action results.
    let (refresh_phase, _set_refresh_phase) = signal::<Option<String>>(None);

    // Interval handle lives in a StoredValue so `on_cleanup` can drop it —
    // never `.forget()`. `seen_running` and `poll_count` guard the startup
    // race: a poll can observe the pre-existing "idle" status before the
    // background task has flipped it to "running". We only treat a
    // subsequent "idle"/"failed" as terminal once we've actually observed
    // "running", or after a few polls have passed with no "running"
    // sighting (the job finished faster than our poll interval could
    // catch it). `poll_count` doubles as an overall safety cap so a stuck
    // "running" status can't poll forever.
    #[cfg(target_arch = "wasm32")]
    let interval_handle: StoredValue<Option<send_wrapper::SendWrapper<gloo_timers::callback::Interval>>> =
        StoredValue::new(None);
    #[cfg(target_arch = "wasm32")]
    let seen_running: StoredValue<bool> = StoredValue::new(false);
    #[cfg(target_arch = "wasm32")]
    let poll_count: StoredValue<u32> = StoredValue::new(0);

    #[cfg(target_arch = "wasm32")]
    on_cleanup(move || {
        interval_handle.set_value(None);
    });

    // Poll interval — 4s. Starts a fresh run each time `refresh_action`
    // reports the background job was (re-)kicked off.
    #[cfg(target_arch = "wasm32")]
    const CATALOG_REFRESH_POLL_INTERVAL_MS: u32 = 4_000;
    // ~5 minutes at the poll interval above.
    #[cfg(target_arch = "wasm32")]
    const CATALOG_REFRESH_MAX_POLLS: u32 = 75;
    // Number of polls to tolerate a stale "idle" reading before treating
    // it as terminal, when "running" was never observed.
    #[cfg(target_arch = "wasm32")]
    const CATALOG_REFRESH_IDLE_GRACE_POLLS: u32 = 3;

    #[cfg(target_arch = "wasm32")]
    let start_refresh_polling = move || {
        use send_wrapper::SendWrapper;

        // Reset race-guard state for this run and optimistically show the
        // live indicator — the background task may not have flipped
        // status to "running" yet.
        seen_running.set_value(false);
        poll_count.set_value(0);
        _set_refresh_phase.try_set(Some("running".to_string()));
        // Cancel any previous interval before starting a new one (e.g. the
        // user clicked Refresh again after a prior run already finished).
        interval_handle.set_value(None);

        let poll = move || {
            // This callback fires from a detached JS interval — the
            // component may already be disposed if cleanup raced the
            // timer. `try_get_untracked` bails out gracefully instead of
            // panicking (bare `.get_untracked()` would panic).
            let Some(slug) = datasource_slug.try_get_untracked() else {
                return;
            };
            if slug.is_empty() {
                return;
            }
            leptos::task::spawn_local(async move {
                match get_catalog_refresh_status(slug).await {
                    Ok(resp) => {
                        // The response can arrive after the component was
                        // disposed (e.g. modal closed mid-request). Every
                        // StoredValue/signal touch below uses `try_`
                        // variants so a disposed scope short-circuits
                        // instead of panicking.
                        let Some(count) = poll_count.try_get_value().map(|c| c + 1) else {
                            return;
                        };
                        poll_count.try_set_value(count);

                        // Safety cap first, so a status that never reaches a
                        // terminal state (e.g. a job wedged at "running"
                        // forever — crashed worker, orphaned run) can't
                        // poll indefinitely. Checked before the match below
                        // so it bounds every arm uniformly, including
                        // "running" itself.
                        if count >= CATALOG_REFRESH_MAX_POLLS {
                            interval_handle.try_set_value(None);
                            _set_refresh_phase.try_set(None);
                            return;
                        }

                        match resp.status.as_str() {
                            "running" => {
                                seen_running.try_set_value(true);
                                _set_refresh_phase.try_set(Some("running".to_string()));
                            }
                            "idle" => {
                                let seen = seen_running.try_get_value().unwrap_or(false);
                                if seen || count >= CATALOG_REFRESH_IDLE_GRACE_POLLS {
                                    interval_handle.try_set_value(None);
                                    // `Action::dispatch` has no `try_` variant
                                    // and panics if its owning scope is
                                    // disposed. `try_set` returning `None`
                                    // confirms the component is still alive
                                    // before we touch `stats_action`.
                                    let still_mounted =
                                        _set_refresh_phase.try_set(None).is_none();
                                    if still_mounted
                                        && let Some(id) = datasource_id.try_get_untracked()
                                        && !id.is_empty()
                                    {
                                        stats_action.dispatch(id);
                                    }
                                    // KYO-327: an "idle" status can still mean
                                    // some containers/schemas were denied
                                    // during discovery — `resolve_final_status`
                                    // folds a partial run down to "idle" as
                                    // long as at least one table was found
                                    // elsewhere. `progress.warnings` (the same
                                    // structured array `get_catalog_stats`
                                    // reads into `CatalogStatsResult`) carries
                                    // those denials; read directly, never
                                    // parsed out of the "failed" arm's
                                    // collapsed `error` string below.
                                    let warning_count = resp
                                        .progress
                                        .as_ref()
                                        .and_then(|p| p.get("warnings"))
                                        .and_then(|w| w.as_array())
                                        .map(Vec::len)
                                        .unwrap_or(0);
                                    if warning_count > 0 {
                                        let container_word = if warning_count == 1 {
                                            "container"
                                        } else {
                                            "containers"
                                        };
                                        toast_info(format!(
                                            "Catalog refresh completed with warnings — \
                                             {warning_count} {container_word} could not be read"
                                        ));
                                    } else {
                                        toast_success("Catalog refresh complete".to_string());
                                    }
                                }
                            }
                            "failed" => {
                                interval_handle.try_set_value(None);
                                _set_refresh_phase.try_set(None);
                                let detail = resp
                                    .progress
                                    .as_ref()
                                    .and_then(|p| p.get("error"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| "Catalog refresh failed".to_string());
                                toast_error(detail);
                            }
                            _ => {}
                        }
                    }
                    Err(err) => {
                        // Transient network/auth error — keep polling
                        // rather than flashing a toast every 4s. The
                        // safety cap above still bounds the overall wait.
                        leptos::logging::warn!("Failed to poll catalog refresh status: {err}");
                    }
                }
            });
        };

        // Immediate poll so the UI doesn't wait a full interval for the
        // first status check.
        poll();

        let interval =
            gloo_timers::callback::Interval::new(CATALOG_REFRESH_POLL_INTERVAL_MS, poll);
        interval_handle.set_value(Some(SendWrapper::new(interval)));
    };

    Effect::new(move |_| {
        if let Some(result) = refresh_action.value().get() {
            match result {
                Ok(_msg) => {
                    #[cfg(target_arch = "wasm32")]
                    {
                        toast_success(_msg);
                        start_refresh_polling();
                    }
                }
                Err(e) => {
                    leptos::logging::error!("Catalog refresh failed: {e}");
                    #[cfg(target_arch = "wasm32")]
                    toast_error(format!("Catalog refresh failed: {e}"));
                }
            }
        }
    });

    let on_refresh_click = move |_: leptos::ev::MouseEvent| {
        if refresh_action.pending().get_untracked() {
            return;
        }
        let slug = datasource_slug.get_untracked();
        if !slug.is_empty() {
            refresh_action.dispatch(slug);
        }
    };

    // ── Discover resources ───────────────────────────────────────────────
    // Input: (ds_type, conn_cfg, creds, slug_opt)
    type DiscoverInput = (String, serde_json::Value, serde_json::Value, Option<String>);

    let discover_action = Action::new(|input: &DiscoverInput| {
        let (ds_type_val, conn_cfg, creds, slug_opt) = input.clone();
        async move {
            discover_datasource_resources(ds_type_val, conn_cfg, creds, slug_opt).await
        }
    });

    // "idle" | "loading" | "success" | "error"
    let (discover_status, set_discover_status) = signal("idle".to_string());
    let (discover_error, set_discover_error) = signal::<Option<String>>(None);
    let (discovered_items, set_discovered_items) = signal::<Vec<String>>(vec![]);
    // KYO-474: true only when the last Discover attempt succeeded at the
    // connection level but `resource_errors` (KYO-466) named a denial for
    // this type's catalog-scope key specifically — never inferred from
    // `discovered_items` being empty, which is also true for "not
    // attempted yet" and "succeeded, genuinely nothing there" (KYO-452
    // still owns the copy for both of those). Read in the same branch that
    // already reads `resource_errors` below, not re-derived elsewhere.
    let (discover_denied, set_discover_denied) = signal(false);

    Effect::new(move |_| {
        if let Some(result) = discover_action.value().get() {
            match result {
                Ok(r) if r.success => {
                    // Extract the items relevant to this datasource type.
                    // `catalog_denial_key_for_type` (not
                    // `discovery_resource_key_for_type` — KYO-544) is used
                    // for both the item lookup below and the
                    // `resource_errors` check, because they must always
                    // agree on which key means what: BigQuery emits its
                    // items and its denial reason under the same
                    // "projects" key, never "databases".
                    let ds_type_val = datasource_type.get_untracked();
                    let key = catalog_denial_key_for_type(&ds_type_val);
                    // KYO-466: `r.success` only means the connection itself
                    // worked — the specific list this scope cares about can
                    // still have failed independently (e.g. BigQuery's
                    // `list_projects()` denied). Check `resource_errors`
                    // for this key before treating an empty `resources`
                    // entry as "no items" — an empty vec and a listing
                    // failure must not render identically.
                    if let Some(reason) = r.resource_errors.get(key) {
                        set_discover_status.set("error".to_string());
                        // Human-readable noun (catalog_item_label_for_type),
                        // not the raw `resources`/`resource_errors` map key
                        // — a reader of this Alert shouldn't have to know
                        // the wire-format dictionary key to understand what
                        // failed to list.
                        let noun = catalog_item_label_for_type(&ds_type_val);
                        set_discover_error.set(Some(format!("Couldn't list {noun}: {reason}")));
                        set_discovered_items.set(vec![]);
                        set_discover_denied.set(true);
                    } else {
                        set_discover_status.set("success".to_string());
                        set_discover_error.set(None);
                        let items = r.resources.get(key).cloned().unwrap_or_default();
                        set_discovered_items.set(items);
                        set_discover_denied.set(false);
                    }
                }
                Ok(r) => {
                    set_discover_status.set("error".to_string());
                    set_discover_error.set(Some(r.message));
                    set_discovered_items.set(vec![]);
                    set_discover_denied.set(false);
                }
                Err(e) => {
                    set_discover_status.set("error".to_string());
                    set_discover_error.set(Some(e.to_string()));
                    set_discovered_items.set(vec![]);
                    set_discover_denied.set(false);
                }
            }
        }
    });

    // ── Discover resources — Connect path ──────────────────────────────────
    // Kyomi holds no direct DB credentials for Connect datasources; discovery
    // must round-trip through the live agent instead of dialing the database
    // directly. Reuses the same `discover_status` / `discover_error` /
    // `discovered_items` signals as the direct-discovery path above so the
    // rest of the picker UI works unchanged for Connect.
    let connect_discover_action = Action::new(|datasource_id: &String| {
        let datasource_id = datasource_id.clone();
        async move { discover_connect_containers(datasource_id).await }
    });

    Effect::new(move |_| {
        if let Some(result) = connect_discover_action.value().get() {
            match result {
                Ok(names) => {
                    set_discover_status.set("success".to_string());
                    set_discover_error.set(None);
                    set_discovered_items.set(names);
                    // Connect discovery has no `resource_errors` — the live
                    // agent either returns containers or the call errors
                    // out entirely (the `Err` arm below), so a denial in
                    // the KYO-466 sense can't happen on this path.
                    set_discover_denied.set(false);
                }
                Err(e) => {
                    set_discover_status.set("error".to_string());
                    set_discover_error.set(Some(e.to_string()));
                    set_discovered_items.set(vec![]);
                    set_discover_denied.set(false);
                }
            }
        }
    });

    let on_discover_click = move |_: leptos::ev::MouseEvent| {
        if discover_action.pending().get_untracked()
            || connect_discover_action.pending().get_untracked()
        {
            return;
        }
        set_discover_status.set("loading".to_string());
        set_discover_error.set(None);
        set_discovered_items.set(vec![]);
        set_discover_denied.set(false);

        if is_connect.get_untracked() {
            let ds_id = datasource_id.get_untracked();
            connect_discover_action.dispatch(ds_id);
        } else {
            let ds_type_val = datasource_type.get_untracked();
            let conn_cfg = connection_config.get_untracked();
            let creds = credentials.get_untracked();
            let slug = datasource_slug.get_untracked();
            let slug_opt = if slug.is_empty() { None } else { Some(slug) };

            discover_action.dispatch((ds_type_val, conn_cfg, creds, slug_opt));
        }
    };

    // ── Text input for manual item entry ──────────────────────────────────
    let (new_item_input, set_new_item_input) = signal(String::new());

    let on_add_item = move || {
        let val = new_item_input.get_untracked().trim().to_string();
        if val.is_empty() {
            return;
        }
        let mut selected = catalog_selected.get_untracked();
        if selected.contains(&val) {
            return;
        }
        selected.push(val);
        set_catalog_selected.set(selected);
        set_new_item_input.set(String::new());
    };

    // ── Relative time formatter ───────────────────────────────────────────
    let format_relative = |iso: &str| -> String {
        // Parse the RFC3339 timestamp and compute a relative description.
        use std::str::FromStr as _;
        let Ok(dt) = chrono::DateTime::<chrono::Utc>::from_str(iso) else {
            return iso.to_string();
        };
        let now = chrono::Utc::now();
        let diff = now.signed_duration_since(dt);
        let secs = diff.num_seconds();
        if secs < 60 {
            "just now".to_string()
        } else if secs < 3600 {
            let m = secs / 60;
            format!("{m} minute{} ago", if m == 1 { "" } else { "s" })
        } else if secs < 86400 {
            let h = secs / 3600;
            format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
        } else {
            let d = secs / 86400;
            format!("{d} day{} ago", if d == 1 { "" } else { "s" })
        }
    };

    view! {
        <div class="space-y-6">

            // ── Stats card ──────────────────────────────────────────────────
            <div class="rounded-lg border border-border bg-card">
                // Card header with title + refresh button
                <div class="flex items-center justify-between px-4 py-3 border-b border-border">
                    <div class="flex items-center gap-2">
                        <span class="h-4 w-4 text-muted-foreground inline-flex items-center justify-center">
                            <Icon icon=phosphor_leptos::STACK/>
                        </span>
                        <span class="text-sm font-medium text-foreground">"Data Catalog"</span>
                    </div>
                    <Show when=move || !is_sample.get()>
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=Signal::derive(move || {
                                refresh_action.pending().get()
                                    || refresh_phase.get().as_deref() == Some("running")
                            })
                            on:click=on_refresh_click
                        >
                            <span class="h-4 w-4 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::ARROWS_CLOCKWISE/>
                            </span>
                            {move || if refresh_action.pending().get()
                                || refresh_phase.get().as_deref() == Some("running")
                            {
                                "Refreshing..."
                            } else {
                                "Refresh Now"
                            }}
                        </Button>
                    </Show>
                </div>

                // Stats grid
                <div class="p-4">
                    {move || {
                        if stats_action.pending().get() && stats.get().is_none() {
                            view! {
                                <div class="grid grid-cols-3 gap-3">
                                    <Skeleton class="h-14 w-full"/>
                                    <Skeleton class="h-14 w-full"/>
                                    <Skeleton class="h-14 w-full"/>
                                </div>
                            }.into_any()
                        } else {
                            let table_count = stats.get().map(|s| s.table_count).unwrap_or(0);
                            let schema_count = stats.get().map(|s| s.schema_count).unwrap_or(0);
                            let last_indexed_str = stats
                                .get()
                                .and_then(|s| s.last_indexed)
                                .map(|iso| format_relative(&iso))
                                .unwrap_or_else(|| "Never".to_string());
                            let ds_type_val = datasource_type.get();
                            let schema_label = match ds_type_val.as_str() {
                                "bigquery" => "Datasets",
                                _ => "Schemas",
                            };
                            view! {
                                <div class="grid grid-cols-3 gap-3">
                                    <div class="flex flex-col gap-0.5 p-3 rounded-md bg-muted/40">
                                        <span class="text-lg font-semibold font-data text-foreground">
                                            {table_count.to_string()}
                                        </span>
                                        <span class="text-xs text-muted-foreground">"Tables indexed"</span>
                                    </div>
                                    <div class="flex flex-col gap-0.5 p-3 rounded-md bg-muted/40">
                                        <span class="text-lg font-semibold font-data text-foreground">
                                            {schema_count.to_string()}
                                        </span>
                                        <span class="text-xs text-muted-foreground">{schema_label}</span>
                                    </div>
                                    <div class="flex flex-col gap-0.5 p-3 rounded-md bg-muted/40">
                                        <span class="text-sm font-medium font-data text-foreground truncate">
                                            {last_indexed_str}
                                        </span>
                                        <span class="text-xs text-muted-foreground">"Last indexed"</span>
                                    </div>
                                </div>
                            }.into_any()
                        }
                    }}

                    // Refresh error
                    {move || refresh_action.value().get().and_then(|r| r.err()).map(|e| view! {
                        <Alert variant=AlertVariant::Error class="mt-3">
                            <AlertDescription>{e.to_string()}</AlertDescription>
                        </Alert>
                    })}

                    // Persistent last-refresh-failed notice (KYO-126). Unlike
                    // the transient toast the poller fires while a manual
                    // refresh is being watched, this renders from
                    // `CatalogStatsResult` any time the page loads — so a
                    // background/initial index failure (nobody was polling)
                    // still leaves a visible trace. `refresh_failed` reads
                    // `catalog_refresh_status` directly off this datasource's
                    // own row (KYO-267), so a failure on another datasource
                    // in the same workspace can never render here.
                    //
                    // Suppressed while the transient "Refresh error" Alert
                    // above is showing a result from the same manual refresh
                    // — otherwise a failed manual refresh stacks two error
                    // Alerts describing the same failure. Once the poller's
                    // action result clears (e.g. the user starts another
                    // refresh), this persistent notice reappears if the
                    // underlying `stats.refresh_failed` is still true.
                    {move || {
                        if refresh_action.value().get().and_then(|r| r.err()).is_some() {
                            return None;
                        }
                        stats
                            .get()
                            .filter(|s| s.refresh_failed)
                            .map(|s| {
                                let reason = s.refresh_failure_reason.unwrap_or_else(|| {
                                    "Catalog refresh failed — search and AI table discovery \
                                     may be unavailable until the next successful refresh."
                                        .to_string()
                                });
                                view! {
                                    <Alert variant=AlertVariant::Error class="mt-3">
                                        <AlertDescription>{reason}</AlertDescription>
                                    </Alert>
                                }
                            })
                    }}

                    // Persistent partial-refresh-warnings notice (KYO-327).
                    // Companion to the failed-refresh notice above: the last
                    // refresh completed (`catalog_refresh_status == "idle"`)
                    // but one or more containers/schemas could not be read —
                    // e.g. a permission-denied schema alongside otherwise-
                    // successful discovery. Renders from `CatalogStatsResult`
                    // any time the page loads, same as the failed-refresh
                    // notice, so a background/initial refresh with warnings
                    // (nobody was watching the poller) still leaves a visible
                    // trace. Mutually exclusive with the failed notice above:
                    // `get_catalog_stats` only populates `refresh_warnings`
                    // when `refresh_failed` is false, so the two can never
                    // both render for the same stats snapshot — no extra
                    // guard needed here beyond the same transient-error
                    // suppression the failed notice uses.
                    {move || {
                        if refresh_action.value().get().and_then(|r| r.err()).is_some() {
                            return None;
                        }
                        stats
                            .get()
                            .filter(|s| !s.refresh_failed && !s.refresh_warnings.is_empty())
                            .map(|s| {
                                let count = s.refresh_warnings.len();
                                let container_word = if count == 1 { "container" } else { "containers" };
                                let detail = s.refresh_warnings.join("; ");
                                view! {
                                    <Alert variant=AlertVariant::Warning class="mt-3">
                                        <AlertTitle>"Catalog refresh completed with warnings"</AlertTitle>
                                        <AlertDescription>
                                            {format!(
                                                "{count} {container_word} could not be read during the \
                                                 last refresh: {detail}"
                                            )}
                                        </AlertDescription>
                                    </Alert>
                                }
                            })
                    }}
                </div>
            </div>

            // ── Schema/catalog picker (admin only, not for sample) ──────────
            <Show when=move || !is_sample.get()>
                <div class="space-y-3">
                    {move || {
                        let ds_type_val = datasource_type.get();
                        let item_label = catalog_item_label_for_type(&ds_type_val);
                        let config_label = match ds_type_val.as_str() {
                            "bigquery" => "Projects to Index",
                            "clickhouse" | "mysql" | "snowflake" => "Databases to Index",
                            "databricks" => "Catalogs to Index",
                            _ => "Schemas to Index",
                        };
                        // KYO-474: a listing denial from the last Discover
                        // attempt (`discover_denied`, sourced from
                        // `resource_errors` — KYO-466, never inferred from
                        // `discovered_items` being empty) replaces the
                        // "leave empty" promise with a direct instruction to
                        // use the manual-entry field below, matching
                        // `CreateModeCatalogPicker`'s fallback copy for the
                        // same state.
                        let description = if discover_denied.get() {
                            format!(
                                "This account can't list {item_label}. Enter the {item_label} you want indexed."
                            )
                        } else {
                            format!(
                                "Select which {item_label} to include in catalog indexing. Leave empty to index all {item_label} this account can list."
                            )
                        };

                        view! {
                            <div>
                                <h4 class="text-sm font-medium text-foreground mb-1">
                                    {config_label}
                                </h4>
                                <p class="text-xs text-muted-foreground mb-3">
                                    {description}
                                </p>
                            </div>
                        }
                    }}

                    // BigQuery: Include Public Datasets toggle
                    <Show when=move || datasource_type.get() == "bigquery">
                        <label class="flex items-center justify-between p-3 rounded-lg border border-border bg-muted/30 cursor-pointer">
                            <div>
                                <span class="text-sm font-medium text-foreground block">
                                    "Include Public Datasets"
                                </span>
                                <span class="text-xs text-muted-foreground">
                                    "Show BigQuery public datasets in search results"
                                </span>
                            </div>
                            <Switch
                                checked=Signal::from(bq_include_public)
                                on_change=Callback::new(move |val: bool| set_bq_include_public.set(val))
                            />
                        </label>
                    </Show>

                    // Discover Available button
                    <div class="flex items-center gap-2">
                        <Button
                            variant=ButtonVariant::Outline
                            size=ButtonSize::Sm
                            disabled=Signal::derive(move || {
                                discover_action.pending().get()
                                    || connect_discover_action.pending().get()
                            })
                            on:click=on_discover_click
                        >
                            <span class="h-4 w-4 inline-flex items-center justify-center">
                                <Icon icon=phosphor_leptos::MAGNIFYING_GLASS/>
                            </span>
                            {move || if discover_action.pending().get()
                                || connect_discover_action.pending().get()
                            {
                                "Discovering..."
                            } else {
                                "Discover Available"
                            }}
                        </Button>
                        {move || if discover_status.get() == "success" {
                            let count = discovered_items.get().len();
                            if count > 0 {
                                Some(view! {
                                    <span class="text-xs text-muted-foreground">
                                        {format!("{count} found")}
                                    </span>
                                })
                            } else {
                                // KYO-466: a successful discovery that found
                                // nothing must say so — before this fix an
                                // empty (but real) result and a listing
                                // failure both rendered as silence. The
                                // failure case now renders via the
                                // "Discovery error" Alert below, driven by
                                // `discover_error`.
                                let noun = catalog_item_label_for_type(&datasource_type.get());
                                Some(view! {
                                    <span class="text-xs text-muted-foreground">
                                        {format!("No {noun} found")}
                                    </span>
                                })
                            }
                        } else {
                            None
                        }}
                    </div>

                    // Discovery error
                    {move || discover_error.get().filter(|_| discover_status.get() == "error").map(|msg| view! {
                        <Alert variant=AlertVariant::Warning class="mt-1">
                            <AlertDescription>{msg}</AlertDescription>
                        </Alert>
                    })}

                    // Discovered items: checkbox list
                    <Show when=move || {
                        discover_status.get() == "success" && !discovered_items.get().is_empty()
                    }>
                        <div class="space-y-2">
                            // Select all / Clear buttons + count
                            <div class="flex items-center gap-2">
                                <button
                                    type="button"
                                    class="text-xs text-primary hover:underline"
                                    on:click=move |_| {
                                        set_catalog_scope_touched.set(true);
                                        set_catalog_selected.set(discovered_items.get_untracked());
                                    }
                                >
                                    "Select all"
                                </button>
                                <span class="text-xs text-muted-foreground">"·"</span>
                                <button
                                    type="button"
                                    class="text-xs text-primary hover:underline"
                                    on:click=move |_| {
                                        set_catalog_scope_touched.set(true);
                                        set_catalog_selected.set(vec![]);
                                    }
                                >
                                    "Clear"
                                </button>
                                <span class="text-xs text-muted-foreground ml-auto">
                                    {move || {
                                        let sel = catalog_selected.get().len();
                                        let total = discovered_items.get().len();
                                        format!("{sel} of {total} selected")
                                    }}
                                </span>
                            </div>
                            // Checkbox list
                            <div class="border border-border rounded-md divide-y divide-border max-h-60 overflow-y-auto">
                                <For
                                    each=move || discovered_items.get()
                                    key=|item| item.clone()
                                    let:item
                                >
                                    {
                                        let item_for_change = item.clone();
                                        let item_for_check = item.clone();
                                        view! {
                                            <label class="flex items-center gap-3 px-3 py-2 cursor-pointer hover:bg-muted/40 transition-colors">
                                                <input
                                                    type="checkbox"
                                                    class="h-4 w-4 rounded border-input accent-primary"
                                                    prop:checked=move || catalog_selected.get().contains(&item_for_check)
                                                    on:change=move |ev| {
                                                        let checked = event_target_checked(&ev);
                                                        let val = item_for_change.clone();
                                                        set_catalog_scope_touched.set(true);
                                                        set_catalog_selected.update(|list| {
                                                            if checked {
                                                                if !list.contains(&val) {
                                                                    list.push(val);
                                                                }
                                                            } else {
                                                                list.retain(|i| i != &val);
                                                            }
                                                        });
                                                    }
                                                />
                                                <span class="text-sm font-mono text-foreground">
                                                    {item.clone()}
                                                </span>
                                            </label>
                                        }
                                    }
                                </For>
                            </div>
                        </div>
                    </Show>

                    // Currently selected items (chip list)
                    <Show when=move || !catalog_selected.get().is_empty()>
                        <div class="space-y-1">
                            <p class="text-xs text-muted-foreground font-medium">"Currently selected:"</p>
                            <div class="flex flex-wrap gap-1.5">
                                <For
                                    each=move || catalog_selected.get()
                                    key=|item| item.clone()
                                    let:item
                                >
                                    {
                                        let item_for_remove = item.clone();
                                        view! {
                                            <span class="inline-flex items-center gap-1 px-2 py-0.5 rounded-full border border-border bg-muted/40 text-xs font-mono text-foreground">
                                                {item.clone()}
                                                <button
                                                    type="button"
                                                    class="h-3.5 w-3.5 inline-flex items-center justify-center text-muted-foreground hover:text-foreground transition-colors"
                                                    on:click=move |_| {
                                                        let val = item_for_remove.clone();
                                                        set_catalog_scope_touched.set(true);
                                                        set_catalog_selected.update(|list| {
                                                            list.retain(|i| i != &val);
                                                        });
                                                    }
                                                >
                                                    <Icon icon=phosphor_leptos::X size="10px"/>
                                                </button>
                                            </span>
                                        }
                                    }
                                </For>
                            </div>
                        </div>
                    </Show>

                    // Manual text input (always shown as fallback / supplement)
                    <Show when=move || {
                        discover_status.get() != "success" || discovered_items.get().is_empty()
                    }>
                        <div class="space-y-1.5">
                            <p class="text-xs text-muted-foreground">
                                "Or add items manually:"
                            </p>
                            <div class="flex gap-2">
                                <input
                                    type="text"
                                    class=MODAL_INPUT_CLASS
                                    placeholder=move || {
                                        let ds_type_val = datasource_type.get();
                                        match ds_type_val.as_str() {
                                            "bigquery" => "Enter project ID",
                                            "clickhouse" | "mysql" | "snowflake" => "Enter database name",
                                            "databricks" => "Enter catalog name (e.g. main)",
                                            _ => "Enter schema name (e.g. public)",
                                        }
                                    }
                                    prop:value=move || new_item_input.get()
                                    on:input=move |ev| set_new_item_input.set(event_target_value(&ev))
                                    on:keydown=move |ev| {
                                        if ev.key() == "Enter" {
                                            ev.prevent_default();
                                            on_add_item();
                                        }
                                    }
                                />
                                <Button
                                    variant=ButtonVariant::Outline
                                    size=ButtonSize::Sm
                                    disabled=Signal::derive(move || new_item_input.get().trim().is_empty())
                                    on:click=move |_| on_add_item()
                                >
                                    <span class="h-4 w-4 inline-flex items-center justify-center">
                                        <Icon icon=phosphor_leptos::PLUS/>
                                    </span>
                                </Button>
                            </div>
                        </div>
                    </Show>

                    <p class="text-xs text-muted-foreground">
                        "Changes to the catalog scope take effect on the next refresh."
                    </p>
                </div>
            </Show>

            // ── INDEXING CREDENTIALS ──────────────────────────────
            <Show when=move || !is_sample.get() && !is_connect.get()>
                <div class="border-t border-border pt-6 space-y-4">
                    <div class="flex items-center justify-between">
                        <div>
                            <h4 class="text-sm font-medium text-foreground">"Catalog Indexing Credentials"</h4>
                            <p class="text-xs text-muted-foreground mt-0.5">
                                "By default, the workspace owner's credentials are used for catalog indexing."
                            </p>
                        </div>
                        <a
                            href="https://kyomi.ai/docs/datasources/indexing-credentials"
                            target="_blank"
                            rel="noopener noreferrer"
                            class="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors"
                        >
                            <Icon icon=phosphor_leptos::QUESTION/>
                            "Learn more"
                        </a>
                    </div>

                    <label class="flex items-center gap-3 cursor-pointer">
                        <input
                            type="checkbox"
                            class="h-4 w-4 rounded-md border-input accent-primary"
                            prop:checked=move || use_indexing_credentials.get()
                            on:change=move |ev| {
                                let checked = event_target_checked(&ev);
                                set_use_indexing_credentials.set(checked);
                                if !checked {
                                    set_indexing_creds_type.set(String::new());
                                    set_indexing_creds_json.set(String::new());
                                    set_indexing_username.set(String::new());
                                    set_indexing_password.set(String::new());
                                    set_indexing_token.set(String::new());
                                    set_indexing_client_id.set(String::new());
                                    set_indexing_client_secret.set(String::new());
                                    set_indexing_tenant_id.set(String::new());
                                }
                            }
                        />
                        <span class="text-sm text-foreground">"Use dedicated indexing credentials"</span>
                    </label>

                    <Show when=move || use_indexing_credentials.get()>
                        <div class="pl-7 space-y-4">
                            <Alert variant=AlertVariant::Info>
                                <AlertDescription>
                                    "OAuth credentials cannot be used for indexing (tokens expire and background jobs cannot refresh them). Use a service account or password-based credentials."
                                </AlertDescription>
                            </Alert>

                            {move || {
                                let ds_type_val = datasource_type.get();

                                // Which auth modes to offer comes from the registry (via
                                // `get_datasource_types`), not a client-side match — see
                                // `DatasourceTypeMetadata::indexing_auth_modes`. `None` is
                                // "still loading" (the `use_query` fetch hasn't resolved
                                // yet); render nothing rather than a false "not available"
                                // warning that would flash on every mount.
                                let types_result = match datasource_types.get() {
                                    Some(result) => result,
                                    None => return view! { <div></div> }.into_any(),
                                };
                                let all_types = match types_result {
                                    Ok(types) => types,
                                    Err(_) => {
                                        return view! {
                                            <Alert variant=AlertVariant::Warning>
                                                <AlertDescription>
                                                    "Failed to load indexing credential options. Please refresh and try again."
                                                </AlertDescription>
                                            </Alert>
                                        }.into_any();
                                    }
                                };
                                let auth_modes: Vec<AuthModeOption> = all_types
                                    .into_iter()
                                    .find(|t| t.type_id == ds_type_val)
                                    .map(|t| t.indexing_auth_modes)
                                    .unwrap_or_default();

                                if auth_modes.is_empty() {
                                    return view! {
                                        <Alert variant=AlertVariant::Warning>
                                            <AlertDescription>
                                                {format!("Indexing credentials configuration is not available for {} datasources.", ds_type_val)}
                                            </AlertDescription>
                                        </Alert>
                                    }.into_any();
                                }

                                let current_type = indexing_creds_type.get();
                                if current_type.is_empty() {
                                    set_indexing_creds_type.set(auth_modes[0].mode_id.clone());
                                }

                                view! {
                                    {if auth_modes.len() > 1 {
                                        view! {
                                            <div class="space-y-2">
                                                <label class="text-sm font-medium text-foreground">"Authentication Method"</label>
                                                <div class="flex flex-wrap gap-2">
                                                    {auth_modes.iter().map(|mode| {
                                                        let value = mode.mode_id.clone();
                                                        let label = mode.display_name.clone();
                                                        let value_for_click = value.clone();
                                                        view! {
                                                            <button
                                                                type="button"
                                                                class=format!("px-3 py-1.5 text-sm rounded-md border transition-colors {}",
                                                                    if indexing_creds_type.get() == value {
                                                                        "border-primary bg-primary/10 text-primary font-medium"
                                                                    } else {
                                                                        "border-input text-muted-foreground hover:text-foreground hover:border-foreground/30"
                                                                    }
                                                                )
                                                                on:click=move |_| set_indexing_creds_type.set(value_for_click.clone())
                                                            >
                                                                {label.clone()}
                                                            </button>
                                                        }
                                                    }).collect::<Vec<_>>()}
                                                </div>
                                            </div>
                                        }.into_any()
                                    } else {
                                        view! { <div></div> }.into_any()
                                    }}

                                    {move || {
                                        match indexing_creds_type.get().as_str() {
                                            "service_account" => view_service_account_form(
                                                indexing_creds_json,
                                                set_indexing_creds_json,
                                                set_indexing_creds_unchanged,
                                            ),
                                            "password" | "sql" => view_password_form(
                                                indexing_username,
                                                set_indexing_username,
                                                indexing_password,
                                                set_indexing_password,
                                                set_indexing_creds_unchanged,
                                            ),
                                            "token" => view_token_form(
                                                indexing_token,
                                                set_indexing_token,
                                                set_indexing_creds_unchanged,
                                            ),
                                            "service_principal" => view_service_principal_form(
                                                indexing_client_id,
                                                set_indexing_client_id,
                                                indexing_client_secret,
                                                set_indexing_client_secret,
                                                indexing_tenant_id,
                                                set_indexing_tenant_id,
                                                set_indexing_creds_unchanged,
                                            ),
                                            _ => view! {
                                                <Alert variant=AlertVariant::Warning>
                                                    <AlertDescription>
                                                        {format!("Unknown auth mode: {}", indexing_creds_type.get())}
                                                    </AlertDescription>
                                                </Alert>
                                            }.into_any(),
                                        }
                                    }}
                                }.into_any()
                            }}
                        </div>
                    </Show>
                </div>
            </Show>
        </div>
    }
}

#[cfg(all(test, feature = "ssr"))]
mod tests;
