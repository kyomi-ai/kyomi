//! KYO-704: BigQuery's `kyomi_oauth` auth mode is retired — it escalated
//! the user's **sign-in** credential to account-wide Google Cloud
//! project-listing scope, with nothing to de-escalate it afterward, and
//! production usage (measured 2026-09-08) was zero datasources, zero
//! queries ever run through it. Phase A (already merged) removed it from
//! `kyomi_core::datasource_registry::BIGQUERY_META.auth_modes` and made
//! `service_account` the registry default. This file pins the UI-side
//! half: the default this file's own `BIGQUERY_DEFAULT_AUTH_MODE` resolves
//! to, and the shape of `oauth_url_for_datasource`'s BigQuery arm now that
//! one of its two prior branches (`kyomi_oauth`'s shared Google endpoint)
//! no longer has anything to route to for the new default.

use super::super::{oauth_url_for_datasource, BIGQUERY_DEFAULT_AUTH_MODE};
use super::{extract_between, SRC};

// ── The default itself ──────────────────────────────────────────────────

#[test]
fn bigquery_default_auth_mode_constant_is_service_account() {
    assert_eq!(
        BIGQUERY_DEFAULT_AUTH_MODE, "service_account",
        "BIGQUERY_DEFAULT_AUTH_MODE must match the registry default \
         (service_account_auth_mode(true) in \
         kyomi_core::datasource_registry::BIGQUERY_META) now that kyomi_oauth is \
         retired — a second, drifted default here would reopen exactly the KYO-442 \
         class of bug the constant exists to prevent"
    );
}

/// The create-mode form's `bq_auth_mode` signal — this is a Leptos
/// component body, not a standalone function, so (following this test
/// module's established pattern for reactive code, e.g. `create_mode.rs`)
/// this asserts against the production source text rather than mounting
/// the component.
#[test]
fn bq_auth_mode_signal_initial_value_is_service_account() {
    assert!(
        SRC.contains("let (bq_auth_mode, set_bq_auth_mode) = signal(\"service_account\".to_string());"),
        "the create-mode form's bq_auth_mode signal must initialize to \
         \"service_account\" — a fresh BigQuery datasource must default to \
         service_account in the UI, not the retired kyomi_oauth"
    );
    assert!(
        !SRC.contains("signal(\"kyomi_oauth\".to_string())"),
        "no signal in datasources.rs may still initialize to the retired \
         kyomi_oauth as its default value"
    );
}

/// `reset_form` (used both to seed a brand-new create-mode form and to
/// clear it between opens) must reset `bq_auth_mode` to the same default
/// as its initial signal value above — a reset that reintroduced
/// `"kyomi_oauth"` here would silently default every *reopened* create
/// form back to the retired mode even though the signal's own initial
/// value was fixed.
#[test]
fn reset_form_resets_bq_auth_mode_to_service_account() {
    let reset_call = extract_between(
        SRC,
        "set_cfg_ssh_passphrase.set(String::new());",
        "set_cfg_oauth_client_id.set(String::new());",
    );
    assert!(
        reset_call.contains("set_bq_auth_mode.set(\"service_account\".to_string());"),
        "reset_form must reset bq_auth_mode to \"service_account\", matching the \
         signal's own default — got: {reset_call:?}"
    );
}

// ── `oauth_url_for_datasource`'s BigQuery arm ───────────────────────────

#[test]
fn oauth_url_for_datasource_bigquery_service_account_has_no_oauth_url() {
    assert_eq!(
        oauth_url_for_datasource("bigquery", "my-ds", Some("service_account")),
        "",
        "service_account has no OAuth flow at all — oauth_url_for_datasource must \
         return the same empty-string \"unsupported\" sentinel it already uses for \
         every other non-OAuth datasource type, not a stale Google OAuth URL"
    );
}

/// The load-bearing case: an `auth_mode: None` row (never configured, or a
/// brand-new create-mode form) must resolve through `BIGQUERY_DEFAULT_AUTH_MODE`
/// — i.e. service_account — and therefore must ALSO produce no OAuth URL.
/// Before this ticket, both the default and this function's fallback were
/// `"kyomi_oauth"`, so `None` produced the (gated) Google OAuth URL; a fix
/// that updated only the constant and missed this function's own match arm
/// would leave a null `auth_mode` still routing to the retired endpoint.
#[test]
fn oauth_url_for_datasource_bigquery_absent_auth_mode_has_no_oauth_url() {
    assert_eq!(
        oauth_url_for_datasource("bigquery", "my-ds", None),
        "",
        "an absent auth_mode resolves to BIGQUERY_DEFAULT_AUTH_MODE (service_account), \
         which has no OAuth flow — got a non-empty URL, meaning None is still \
         resolving to the retired kyomi_oauth endpoint"
    );
}

/// A pre-KYO-704 row whose *stored* `auth_mode` still literally names the
/// retired `kyomi_oauth` mode must not keep launching the now-retired
/// Google endpoint either — `apps/server/src/routes/auth_google_oauth.rs`'s
/// `google_oauth_connect` handler now unconditionally rejects that request
/// server-side, so routing a popup there produces a worse experience (a
/// raw JSON error inside the popup) than resolving to the same
/// `Unsupported` outcome every other non-OAuth mode already gets, with its
/// existing clear-toast handling (`ListConnectAction::Unsupported`,
/// `pages/settings/datasources.rs`).
#[test]
fn oauth_url_for_datasource_bigquery_explicit_retired_kyomi_oauth_has_no_oauth_url() {
    assert_eq!(
        oauth_url_for_datasource("bigquery", "my-ds", Some("kyomi_oauth")),
        "",
        "an explicit, stored kyomi_oauth auth_mode must resolve to the same \
         no-OAuth-URL outcome as service_account now that the mode is retired"
    );
}

/// Regression guard: `enterprise_oauth` is untouched by this ticket and
/// must keep producing its slug-scoped connect URL exactly as before.
#[test]
fn oauth_url_for_datasource_bigquery_enterprise_oauth_is_unaffected() {
    assert_eq!(
        oauth_url_for_datasource("bigquery", "my-ds", Some("enterprise_oauth")),
        "/api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=my-ds",
        "enterprise_oauth must keep working end to end — KYO-704 only retires \
         kyomi_oauth and changes the default, not this mode"
    );
}
