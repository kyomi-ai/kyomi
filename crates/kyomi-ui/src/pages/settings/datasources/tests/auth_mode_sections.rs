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

#[test]
fn bigquery_auth_mode_admin_fields_are_gated() {
    let f = extract_between(SRC, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection(");
    assert!(
        f.contains("is_admin: Signal<bool>"),
        "BigQueryAuthModeSection must accept an is_admin prop"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"Authentication Mode\"", 250),
        "BigQuery Authentication Mode selector must be is_admin-gated"
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
fn snowflake_auth_mode_selector_is_gated() {
    let f = extract_between(SRC, "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection(");
    assert!(
        f.contains("is_admin: Signal<bool>"),
        "SnowflakeAuthModeSection must accept an is_admin prop"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"Authentication Mode\"", 250),
        "Snowflake Authentication Mode selector must be is_admin-gated"
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
        appears_shortly_before(f, IS_ADMIN_GATE, "\"Authentication Mode\"", 250),
        "Databricks Authentication Mode selector must be is_admin-gated"
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
        appears_shortly_before(f, IS_ADMIN_GATE, "\"Authentication Mode\"", 250),
        "Synapse Authentication Mode selector must be is_admin-gated"
    );
    assert!(
        appears_shortly_before(f, IS_ADMIN_GATE, "\"OAuth Client ID\"", 1500),
        "Synapse OAuth Client ID field must be is_admin-gated"
    );
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
#[test]
fn auth_mode_sections_read_options_from_registry_not_a_hardcoded_vec() {
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
            f.contains("auth_mode_select_options(&auth_modes.get())"),
            "{name}AuthModeSection's Select options must be built from \
             auth_mode_select_options(&auth_modes.get()), not a hardcoded list"
        );
        assert!(
            f.contains("auth_mode_description(&auth_modes.get()"),
            "{name}AuthModeSection's description paragraph must come from \
             auth_mode_description(&auth_modes.get(), ...), not a local `match`"
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
/// `AuthModeOption::is_default` — never baked into a per-provider
/// component as a hardcoded string. It's fine (and required) for
/// `auth_mode_select_options` itself to construct the suffix; this test
/// checks the four `*AuthModeSection` bodies specifically, since a
/// hardcoded "(Recommended)" there is exactly the pre-KYO-274 shape
/// (BigQuery's UI baked it into a `Signal::stored(vec![...])` literal).
#[test]
fn recommended_suffix_is_rendered_not_baked_into_display_name() {
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
            "{name}AuthModeSection must not hardcode \"(Recommended)\" itself — that string \
             belongs solely in the shared auth_mode_select_options helper, derived from \
             AuthModeOption::is_default"
        );
    }

    let helper = extract_between(SRC, "fn auth_mode_select_options(", "fn auth_mode_description(");
    assert!(
        helper.contains("m.is_default") && helper.contains("(Recommended)"),
        "auth_mode_select_options must derive the \"(Recommended)\" suffix from \
         AuthModeOption::is_default"
    );
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
