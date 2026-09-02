//! Per-provider Auth Mode Section gating and registry-driven options
//! (MAJOR 1, KYO-274): which fields are admin-only, and the
//! connection-auth-modes-unavailable warning when the registry fetch fails.

use super::{appears_shortly_before, extract_between, SRC};

// ── MAJOR 1: auth-mode section gating ───────────────────────────────
//
// Each of the four provider Auth Mode Sections mixes admin-only
// connection-config fields (Authentication Mode selector, OAuth Client
// ID/Secret, Service Account JSON) with per-user OAuth connect UI. Only
// the admin-only fields must be gated on `is_admin` — gating the whole
// component would hide the personal OAuth panel from every member.

// `needle` deliberately omits the `<Show ` prefix and trailing `>` — the
// gate around "Service Account JSON" is a multi-line `<Show>` tag
// (`when=` on its own line), while the others are single-line. Matching
// on `when=move || is_admin.get()` catches both forms.
const IS_ADMIN_GATE: &str = "when=move || is_admin.get()";

// KYO-234: each provider's own `<Show when=is_admin.get()>` around the
// "Authentication Mode" label used to be checked right here, once per
// provider. That markup was extracted into the shared `AuthModeSelector`
// component below, so the is_admin gate on the selector itself is now
// checked once, against AuthModeSelector's own body
// (`auth_mode_selector_gates_the_selector_on_is_admin`), and
// `all_four_provider_sections_use_the_shared_auth_mode_selector` confirms
// every provider actually calls into it rather than rolling its own. The
// four tests below keep checking the gating this ticket did NOT touch —
// the per-mode credential panels (OAuth Client ID/Secret, Service Account
// JSON) — which KYO-197 and this ticket both left alone on purpose.

#[test]
fn bigquery_auth_mode_admin_fields_are_gated() {
    let f = extract_between(SRC, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection(");
    assert!(
        f.contains("is_admin: Signal<bool>"),
        "BigQueryAuthModeSection must accept an is_admin prop"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"OAuth Client ID\"", 1500),
        "BigQuery Enterprise OAuth Client ID field must be is_admin-gated"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"Service Account JSON\"", 1000),
        "BigQuery Service Account JSON field must be is_admin-gated"
    );
}

#[test]
fn snowflake_auth_mode_section_declares_is_admin() {
    let f = extract_between(SRC, "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection(");
    assert!(
        f.contains("is_admin: Signal<bool>"),
        "SnowflakeAuthModeSection must accept an is_admin prop — it has no admin-only \
         fields of its own, but must still thread is_admin through to AuthModeSelector"
    );
}

#[test]
fn databricks_auth_mode_admin_fields_are_gated() {
    let f = extract_between(SRC, "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection(");
    assert!(
        f.contains("is_admin: Signal<bool>"),
        "DatabricksAuthModeSection must accept an is_admin prop"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"OAuth Client ID\"", 1500),
        "Databricks OAuth Client ID field must be is_admin-gated"
    );
}

#[test]
fn synapse_auth_mode_admin_fields_are_gated() {
    let f = extract_between(SRC, "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals");
    assert!(
        f.contains("is_admin: Signal<bool>"),
        "SynapseAuthModeSection must accept an is_admin prop"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"OAuth Client ID\"", 1500),
        "Synapse OAuth Client ID field must be is_admin-gated"
    );
}

/// AuthModeSelector's own is_admin gate — the single place the "Authentication
/// Mode" `<Select>` now renders (KYO-234). `"enum DatasourcesViewState"` is the
/// next structural item in the file after AuthModeSelector, so it can't be
/// deleted by anything this component's own markup could plausibly change.
#[test]
fn auth_mode_selector_gates_the_selector_on_is_admin() {
    let f = extract_between(SRC, "fn AuthModeSelector(", "enum DatasourcesViewState");
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"Authentication Mode\"", 250),
        "AuthModeSelector's Authentication Mode selector must be is_admin-gated"
    );
}

/// KYO-234: guards against a provider quietly reintroducing its own inline
/// Authentication Mode selector (its own `<Show>`/`<Select>`) instead of
/// calling the shared `AuthModeSelector` component. Without this test, a
/// provider-local copy would escape every registry/gating/(Recommended)
/// guard in this file, since every one of them now checks
/// AuthModeSelector's body specifically rather than each provider's — this
/// is the test that makes that retargeting not a weakening.
#[test]
fn all_four_provider_sections_use_the_shared_auth_mode_selector() {
    let sections: &[(&str, &str, &str)] = &[
        ("BigQuery", "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ("Snowflake", "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ("Databricks", "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ("Synapse", "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
    ];
    for (name, start, end) in sections {
        let f = extract_between(SRC, start, end);
        assert!(
            f.contains("<AuthModeSelector"),
            "{name}AuthModeSection must render its Authentication Mode selector via \
             the shared <AuthModeSelector> component (KYO-234), not its own inline \
             <Show>/<Select> — a provider-local copy would escape every guard that \
             checks AuthModeSelector's body instead of this one"
        );
    }
}

// ── KYO-274: connection auth modes come from the registry ──────────
//
// The four `*AuthModeSection` components each hardcoded their own
// Authentication Mode `<Select>` options and a `match`-based description
// paragraph, independently of `kyomi-core`'s registry. This had already
// drifted silently: BigQuery's UI said "Kyomi OAuth (Recommended)" /
// "Enterprise OAuth" while the registry said "Google OAuth (Kyomi)" /
// "Google OAuth (Enterprise)" — nothing detected it, because nothing
// compared the two. Two of the four hardcoded lists had also drifted in
// *membership*, not just label text: Snowflake's UI offered `keypair`
// (a real, fully-wired mode) which had no registry entry at all — so it
// was invisible to `indexing_auth_modes()` too (KYO-187's selector could
// never offer key-pair for catalog indexing) — and Synapse's registry
// carried a plain `oauth` mode the UI never exposed and had no field UI
// for. KYO-274 added `keypair` to the registry and removed the dead
// Synapse `oauth` entry so ids match on both sides, then made all four
// components render `auth_modes` (from `get_datasource_types()`)
// instead of a local hardcoded list, so the two can no longer diverge.
//
// KYO-234 then extracted the `<Show>`/`<Select>` markup all four bodies
// shared into one `AuthModeSelector` component (see its doc comment).
// `auth_mode_select_options(&auth_modes.get())` and
// `auth_mode_description(&auth_modes.get()` therefore no longer appear in
// any of the four `*AuthModeSection` bodies at all — they moved to
// AuthModeSelector's body, which is the one place left to check them
// against. This is not a weaker guarantee: with a single implementation,
// per-provider drift of the kind described above is now structurally
// impossible rather than merely re-checked four times, and
// `all_four_provider_sections_use_the_shared_auth_mode_selector` (above)
// is what makes that true — it fails if a provider ever stops calling
// AuthModeSelector and starts rendering its own copy again.
#[test]
fn auth_mode_sections_read_options_from_registry_not_a_hardcoded_vec() {
    let selector = extract_between(SRC, "fn AuthModeSelector(", "enum DatasourcesViewState");
    assert!(
        selector.contains("auth_modes: Signal<Vec<AuthModeOption>>"),
        "AuthModeSelector must accept a registry-provided auth_modes prop"
    );
    assert!(
        selector.contains("auth_mode_select_options(&auth_modes.get())"),
        "AuthModeSelector's Select options must be built from \
         auth_mode_select_options(&auth_modes.get()), not a hardcoded list"
    );
    assert!(
        selector.contains("auth_mode_description(&auth_modes.get()"),
        "AuthModeSelector's description paragraph must come from \
         auth_mode_description(&auth_modes.get(), ...), not a local `match`"
    );
    assert!(
        !selector.contains("options=Signal::stored(vec!["),
        "AuthModeSelector must not hardcode Select options — that's the KYO-274 \
         drift pattern (BigQuery's UI once said \"Kyomi OAuth (Recommended)\" while the \
         registry said \"Google OAuth (Kyomi)\", and nothing caught it)"
    );

    // Belt-and-suspenders: each provider still declares the auth_modes prop
    // (needed to pass it into AuthModeSelector) and must not reintroduce a
    // hardcoded options list of its own alongside that call.
    let sections: &[(&str, &str, &str)] = &[
        ("BigQuery", "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ("Snowflake", "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ("Databricks", "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ("Synapse", "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
    ];
    for (name, start, end) in sections {
        let f = extract_between(SRC, start, end);
        assert!(
            f.contains("auth_modes: Signal<Vec<AuthModeOption>>"),
            "{name}AuthModeSection must accept a registry-provided auth_modes prop"
        );
        assert!(
            !f.contains("options=Signal::stored(vec!["),
            "{name}AuthModeSection must not hardcode Select options — that's the KYO-274 \
             drift pattern (BigQuery's UI once said \"Kyomi OAuth (Recommended)\" while the \
             registry said \"Google OAuth (Kyomi)\", and nothing caught it)"
        );
    }
}

/// `(Recommended)` must stay a UI-rendered affordance driven by
/// `AuthModeOption::is_default` — never baked into the selector as a
/// hardcoded string. It's fine (and required) for `auth_mode_select_options`
/// itself to construct the suffix; this test checks `AuthModeSelector`'s own
/// body specifically (KYO-234 — previously the four `*AuthModeSection`
/// bodies, before the markup was consolidated), since a hardcoded
/// "(Recommended)" there is exactly the pre-KYO-274 shape (BigQuery's UI
/// baked it into a `Signal::stored(vec![...])` literal).
#[test]
fn recommended_suffix_is_rendered_not_baked_into_display_name() {
    let selector = extract_between(SRC, "fn AuthModeSelector(", "enum DatasourcesViewState");
    assert!(
        !selector.contains("(Recommended)"),
        "AuthModeSelector must not hardcode \"(Recommended)\" itself — that string \
         belongs solely in the shared auth_mode_select_options helper, derived from \
         AuthModeOption::is_default"
    );

    let helper = extract_between(SRC, "fn auth_mode_select_options(", "fn auth_mode_description(");
    assert!(
        helper.contains("m.is_default") && helper.contains("(Recommended)"),
        "auth_mode_select_options must derive the \"(Recommended)\" suffix from \
         AuthModeOption::is_default"
    );

    // Belt-and-suspenders per provider — same rationale as the sibling
    // registry test above.
    let sections: &[(&str, &str, &str)] = &[
        ("BigQuery", "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ("Snowflake", "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ("Databricks", "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ("Synapse", "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
    ];
    for (name, start, end) in sections {
        let f = extract_between(SRC, start, end);
        assert!(
            !f.contains("(Recommended)"),
            "{name}AuthModeSection must not hardcode \"(Recommended)\" itself"
        );
    }
}

// ── KYO-274 review follow-up: connection auth-mode fetch failure ───
//
// `connection_auth_modes` used to resolve a failed `get_datasource_types()`
// fetch via a silent `.and_then(|r| r.ok())`, so a network blip made all
// four Authentication Mode selectors render with zero options and no
// explanation. `connection_auth_modes_unavailable_from` is the pure
// extraction of "did the fetch fail?" that now drives a visible warning
// instead — exercised directly here since (unlike the rest of this
// module) it has no view-tree branching to work around.

use leptos::prelude::ServerFnError;

use super::super::{connection_auth_modes_unavailable_from, DatasourceTypeInfo};

#[test]
fn connection_auth_modes_unavailable_from_failed_fetch_is_true() {
    let unavailable = connection_auth_modes_unavailable_from(&Some(Err(
        ServerFnError::new("simulated network failure"),
    )));
    assert!(
        unavailable,
        "a failed datasource-type registry fetch must report the connection auth \
         modes as unavailable, so the caller can show a warning instead of silently \
         rendering the four Authentication Mode selectors with zero options"
    );
}

#[test]
fn connection_auth_modes_unavailable_from_loading_is_false() {
    let unavailable = connection_auth_modes_unavailable_from(&None);
    assert!(
        !unavailable,
        "a fetch that hasn't resolved yet must not be reported as unavailable — that \
         would flash a false warning on every mount before the query settles"
    );
}

#[test]
fn connection_auth_modes_unavailable_from_success_is_false() {
    let unavailable = connection_auth_modes_unavailable_from(&Some(Ok(vec![
        DatasourceTypeInfo {
            type_id: "bigquery".to_string(),
            display_name: "BigQuery".to_string(),
            indexing_auth_modes: Vec::new(),
            connection_auth_modes: Vec::new(),
        },
    ])));
    assert!(
        !unavailable,
        "a successfully resolved fetch must never be reported as unavailable, \
         regardless of what it contains"
    );
}

/// `connection_auth_modes_unavailable` must be its own reactive scope,
/// not folded into the per-type `connection_auth_modes` derive — the
/// exact KYO-240 shape this repo has hit before (see
/// docs/CODING_STANDARDS.md's "Signal::derive is not memoized"): a
/// derive that reads both the query result and `ds_type` re-runs its
/// whole body, including any log call, on every provider switch, so one
/// stale failure would re-announce itself on every subsequent switch.
#[test]
fn connection_auth_modes_unavailable_is_a_memo_scoped_only_to_datasource_types() {
    let f = extract_between(SRC, "pub fn DatasourceModal(", "fn BigQueryAuthModeSection(");
    assert!(
        f.contains("Memo::new(move |_| connection_auth_modes_unavailable_from(&datasource_types.get()))"),
        "connection_auth_modes_unavailable must be a Memo scoped to datasource_types \
         alone, calling the pure connection_auth_modes_unavailable_from — not a \
         Signal::derive that also reads ds_type"
    );
}

/// The alert must actually gate the four provider selectors, not merely
/// render alongside an empty one — otherwise a user with a failed fetch
/// still sees a description-less, option-less BigQuery/Snowflake/
/// Databricks/Synapse dropdown. Checks ordering: the warning Alert
/// (gated on the failure) must appear before a sibling <Show> that
/// gates the four provider selectors on the *negation*, so exactly one
/// of the two branches renders.
#[test]
fn connection_auth_modes_unavailable_alert_wraps_the_four_selectors() {
    let f = extract_between(SRC, "pub fn DatasourceModal(", "fn BigQueryAuthModeSection(");
    let positive_pos = f
        .find("Show when=move || connection_auth_modes_unavailable.get()")
        .expect(
            "expected a <Show when=connection_auth_modes_unavailable.get()> block \
             rendering the failure warning",
        );
    let negated_pos = f
        .find("Show when=move || !connection_auth_modes_unavailable.get()")
        .expect(
            "expected a <Show when=!connection_auth_modes_unavailable.get()> block \
             gating the four *AuthModeSection selectors",
        );
    let bigquery_show_pos = f
        .find("Show when=move || ds_type.get() == \"bigquery\"")
        .expect("expected the BigQuery selector's own <Show>");
    assert!(
        positive_pos < negated_pos && negated_pos < bigquery_show_pos,
        "the failure Alert, the negated gate, and the BigQuery selector must appear \
         in that order — otherwise the four selectors aren't actually suppressed \
         while the fetch has failed"
    );
}

// ── KYO-406: BqProjectField custom-entry escape hatch ──────────────
//
// `BqProjectField` used to degrade to a free-text input only when the
// discovered project list was empty — once *any* project was discovered
// the user was locked into the dropdown, with no way to enter a project
// the discovery API didn't return (cross-project billing, a project the
// token lacks resourcemanager.projects.list on, a freshly created one).
// React's ProjectDropdowns.jsx offered a `__custom__` sentinel option
// (per field, independent state) plus a warning Alert when discovery
// failed but manual entry still worked; the Leptos port dropped both.
//
// `f` below spans from the doc comment preceding `BQ_CUSTOM_PROJECT_OPTION`
// through the end of `BqProjectField` — i.e. everything KYO-406 touched,
// including the doc comments (unlike `extract_between(SRC, "fn
// BqProjectField(", ...)`, which would start at the fn keyword and miss
// the disposal-panic comment above it).
fn bq_project_field_src(src: &str) -> &str {
    extract_between(
        src,
        "/// Sentinel option value that swaps a [`BqProjectField`] into custom-entry",
        "fn BigQueryAuthModeSection(",
    )
}

use super::super::{bq_project_select_options, BQ_CUSTOM_PROJECT_OPTION};

/// The actual bug, asserted by value rather than by the sentinel constant
/// merely appearing somewhere in the source (the KYO-427 → KYO-477 lesson):
/// once projects are discovered, the option list `Select` actually receives
/// must still contain a way to enter a project the API didn't return.
#[test]
fn bq_project_select_options_offers_custom_entry_when_projects_are_discovered() {
    let projects = vec![
        ("proj-a".to_string(), "Project A".to_string()),
        ("proj-b".to_string(), "Project B".to_string()),
    ];
    let options = bq_project_select_options(projects.clone());

    assert_eq!(
        &options[..projects.len()],
        &projects[..],
        "the discovered projects themselves must still be present, ahead of the sentinel"
    );
    assert_eq!(
        options.last(),
        Some(&(
            BQ_CUSTOM_PROJECT_OPTION.to_string(),
            "Enter custom project ID...".to_string(),
        )),
        "once any project is discovered, the option list must still offer a \
         custom-entry escape hatch — this is the KYO-406 bug: the dropdown \
         used to lock the user out of manual entry the moment the list was \
         non-empty"
    );
}

/// Confirms `BqProjectField`'s actual `Select` renders the tested pure
/// function's output, not the raw discovered list — otherwise the sentinel
/// proven present above never reaches the real dropdown.
#[test]
fn bq_project_field_select_uses_the_custom_sentinel_options() {
    let f = bq_project_field_src(SRC);
    assert!(
        f.contains("Signal::derive(move || bq_project_select_options(bq_projects.get()))"),
        "BqProjectField's dropdown Select must render \
         bq_project_select_options(bq_projects.get()), not the raw discovered list"
    );
    assert!(
        f.contains("options=select_options"),
        "the Select in BqProjectField's dropdown branch must consume select_options"
    );
}

/// AC: the custom/dropdown toggle is owned per `BqProjectField` instance,
/// not threaded from a shared parent-level signal — this fails if the
/// toggle is ever hoisted into a prop, which would let two instances of
/// the field (were there ever more than one rendered at once) flip
/// together instead of independently.
#[test]
fn bq_project_field_custom_dropdown_toggle_state_is_local_per_instance() {
    let f = bq_project_field_src(SRC);
    let props = extract_between(f, "fn BqProjectField(", ") -> impl IntoView {");
    assert!(
        !props.contains("is_custom"),
        "BqProjectField must not accept an is_custom prop from its caller — \
         that would let BigQueryAuthModeSection thread one shared signal into \
         multiple instances"
    );
    assert!(
        f.contains("let (is_custom, set_is_custom) = signal(false);"),
        "BqProjectField must own its custom/dropdown toggle as a signal local \
         to the component body — each <BqProjectField/> call creates its own \
         instance, so a locally-owned signal is independent per field"
    );

    // Belt-and-suspenders: no call site (Billing Project, across all three
    // auth modes) may pass a custom-mode prop down. KYO-415 removed the
    // Default Project field (it was never read by any query path), leaving
    // one BqProjectField call site per auth mode.
    assert_eq!(
        SRC.matches("<BqProjectField").count(),
        3,
        "expected 3 BqProjectField call sites (Billing Project across \
         kyomi_oauth, enterprise_oauth, and service_account) — update this \
         test if that count legitimately changed"
    );
    assert!(
        !SRC.contains("is_custom=") && !SRC.contains("set_is_custom="),
        "no BqProjectField call site may pass is_custom/set_is_custom as a \
         prop — the toggle must stay local to each component instance"
    );
}

/// The other half of the custom-entry escape hatch: picking the sentinel
/// must not leak "__custom__" into the field's real value, and there must
/// be a way back.
#[test]
fn bq_project_field_custom_mode_intercepts_sentinel_and_offers_back_to_dropdown() {
    let f = bq_project_field_src(SRC);
    assert!(
        f.contains("if val == BQ_CUSTOM_PROJECT_OPTION") && f.contains("set_is_custom.set(true);"),
        "selecting the sentinel must flip is_custom, not write \"__custom__\" \
         into the field's value signal via set_value"
    );
    assert!(
        f.contains("\"Back to dropdown\"") && f.contains("set_is_custom.set(false)"),
        "custom-entry mode must offer a way back to the dropdown, matching \
         React's \"Back to dropdown\" affordance"
    );
}

/// AC: helper text matches React's two variants exactly.
#[test]
fn bq_project_field_helper_text_matches_react_variants() {
    let f = bq_project_field_src(SRC);
    assert!(
        f.contains("Select from discovered projects or enter a custom ID"),
        "the dropdown-branch helper text must match React's copy"
    );
    assert!(
        f.contains("Connect to discover projects, or enter project ID manually"),
        "the no-projects-branch helper text must match React's copy"
    );
}

/// AC: preserve the `<Show>`-not-`{move || ...}` structure and the
/// disposal-panic comment while going from two branches to three. Going to
/// three states via nested `<Show>` (not a `{move || match ...}`) is the
/// only way to add the custom-entry branch without reintroducing the
/// disposal panic the doc comment documents.
#[test]
fn bq_project_field_uses_nested_show_not_reactive_closure_branching() {
    let f = bq_project_field_src(SRC);
    assert!(
        f.contains("avoids the disposal panic"),
        "the disposal-panic doc comment must survive KYO-406's move to three \
         branches — it documents a real Leptos crash someone already paid for"
    );

    // Count only the actual view! body — `f` also includes the doc comments
    // above `BqProjectField`, which themselves prose-reference "<Show>" twice.
    let body = &f[f.find("view! {").expect("BqProjectField must have a view! body")..];
    assert_eq!(
        body.matches("<Show").count(),
        2,
        "BqProjectField's three render states (no-projects / dropdown / \
         custom-entry) must come from exactly two nested <Show> components \
         (outer: has-projects vs not; inner: dropdown vs custom-entry)"
    );
    assert!(
        !body.contains("move || match") && !body.contains("move || if"),
        "BqProjectField must not branch its three states with a reactive \
         closure — that would recreate Select's internal Effect on every \
         switch and reintroduce the disposal panic"
    );
}

/// AC: discovery-failure messaging uses a Warning Alert stating manual
/// entry still works, matching ProjectDropdowns.jsx:47-56 — replacing the
/// bare red text that used to read as fatal. Checked across all three
/// BigQuery auth mode sections (AC: "Applies to all three auth modes").
#[test]
fn bq_projects_discovery_failure_renders_warning_alert_not_bare_text() {
    let f = extract_between(SRC, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection(");
    assert!(
        !f.contains("text-error-foreground mt-2\">{err}"),
        "BigQuery project discovery failures must no longer render as bare \
         red text — that read as fatal even though manual entry still works"
    );
    assert_eq!(
        f.matches(
            "bq_projects_error.get().filter(|_| bq_projects.get().is_empty()).map(|err| view! {"
        )
        .count(),
        3,
        "expected one discovery-failure-gated Alert per BigQuery auth mode \
         section (kyomi_oauth, enterprise_oauth, service_account)"
    );
    assert_eq!(
        f.matches("<Alert variant=AlertVariant::Warning class=\"mt-2\">").count(),
        3,
        "each discovery-failure message must render as a Warning Alert, not bare text"
    );
    assert_eq!(
        f.matches("You can still enter project IDs manually below.").count(),
        3,
        "the warning must state that manual entry still works, matching \
         ProjectDropdowns.jsx:47-56"
    );
}

// ── KYO-415: dead "Default Project" field removed ───────────────────
//
// All three BigQuery auth modes (kyomi_oauth, enterprise_oauth,
// service_account) rendered a "Default Project" BqProjectField
// alongside "Billing Project". Nothing ever read the saved value: the
// only project consumer on the driver side, `resolve_billing_project`
// (kyomi-connect's crates/kyomi-datasource/src/providers/bigquery.rs),
// reads connection_config["billing_project"] /
// connection_config["default_billing_project"] /
// credentials["billing_project"] — never `default_project` — and the
// BigQuery job URL (`/projects/{billing_project}/queries`) has no
// separate project-only default to apply one to (defaultDataset
// requires a datasetId, which a bare project ID can't supply). The
// field, its signals, and its save/load wiring were removed; Billing
// Project must survive untouched.

/// The failure mode this guards: deleting too much (Billing Project
/// disappears too) or too little (Default Project survives in one of
/// the three auth modes, or its signals/wiring linger unused).
#[test]
fn bigquery_default_project_field_is_gone_billing_project_survives() {
    let f = extract_between(SRC, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection(");

    assert!(
        !f.contains("Default Project"),
        "the dead \"Default Project\" field (KYO-415) must not render in any \
         BigQuery auth mode section"
    );
    assert_eq!(
        f.matches("\"Billing Project\"").count(),
        3,
        "expected one \"Billing Project\" BqProjectField per BigQuery auth \
         mode section (kyomi_oauth, enterprise_oauth, service_account) — \
         Billing Project must survive the Default Project removal"
    );

    // Belt-and-suspenders across the whole file: no leftover signal,
    // prop, or save/load wiring for the removed field.
    assert!(
        !SRC.contains("default_project") && !SRC.contains("cred_default_project"),
        "no default_project / cred_default_project signal, prop, or JSON key \
         may remain anywhere in datasources.rs after KYO-415 — the field is \
         fully removed, not just hidden"
    );
}
