//! OAuth panel messaging correctness: the Google-error-translation wiring
//! guard (`translate_google_oauth_error` itself, and its direct unit
//! tests, live in `utils::oauth_popup` per KYO-421 — this file only pins
//! that both the list-level and modal-level `postMessage` listeners route
//! `GoogleError` through it, and that no other provider's error arm does),
//! and the shared `oauth_config_missing` predicate (KYO-519) that decides
//! whether an OAuth panel shows a normal Connect button or an
//! explanatory "not configured" alert — extracted after three of the
//! four config-bearing surfaces each got that check wrong in a different
//! way.
//!
//! See
//! `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`.

use super::{extract_between, SRC};

// ── KYO-408: Google OAuth denial error translation ──────────────────
//
// `translate_google_oauth_error` itself moved to `utils::oauth_popup`
// (KYO-421), once a third call site (onboarding) needed it — its two
// direct unit tests (`translate_google_oauth_error_rewrites_access_denied`,
// `translate_google_oauth_error_passes_other_errors_through_unchanged`)
// moved with it into that module's own `mod tests`, alongside its other
// wasm32-or-test pure-predicate helpers. The wiring guard below stays
// here: it is a source-text assertion over `datasources.rs`, not a test
// of the function itself.

/// Both OAuth `postMessage` listeners (list-level in `DatasourcesContent`
/// and modal-level in `DatasourceModal`) must translate `GoogleError`
/// specifically, and must NOT apply that translation to the other
/// providers' error arms — `translate_google_oauth_error` assumes an
/// OAuth2 `access_denied` code from Google's shared-app allowlist
/// rejection specifically; applying it to a Snowflake/Databricks/
/// Microsoft/BigQuery-enterprise error would misdescribe those. The
/// sibling guard for the onboarding page's own listener lives in
/// `pages/onboarding/datasource_onboarding.rs`'s own test module.
#[test]
fn google_error_translation_is_not_applied_to_other_providers_error_arms() {
    let list_level = extract_between(
        SRC,
        "OAuthMessage::GoogleError { error } => {",
        "OAuthMessage::SnowflakeError { error }",
    );
    assert!(
        list_level.contains("translate_google_oauth_error(error)"),
        "the list-level listener's GoogleError arm must call \
         translate_google_oauth_error"
    );

    let list_level_others = extract_between(
        SRC,
        "OAuthMessage::SnowflakeError { error }\n                | OAuthMessage::DatabricksError { error }\n                \
         | OAuthMessage::MicrosoftError { error }\n                | OAuthMessage::MicrosoftEnterpriseError { error }\n                \
         | OAuthMessage::BigqueryEnterpriseError { error } => {",
        "toast_error(error);\n                }",
    );
    assert!(
        !list_level_others.contains("translate_google_oauth_error"),
        "the list-level listener's non-Google error arm must pass `error` straight to \
         toast_error, not through translate_google_oauth_error"
    );

    let modal_level = extract_between(
        SRC,
        "OAuthMessage::GoogleError { error } => {\n                    set_modal_oauth_connecting",
        "OAuthMessage::SnowflakeError { error }",
    );
    assert!(
        modal_level.contains("translate_google_oauth_error(error)"),
        "the modal-level listener's GoogleError arm must call translate_google_oauth_error"
    );
}

// ── KYO-519: oauth_config_missing — shared "OAuth not configured" predicate ──
//
// Ports React's `OAuthConnect.jsx:49-58`: `configFields.every(f =>
// !!connectionConfig[f.name])` gates whether the Connect button (vs. an
// explanatory Alert) renders. Negated — "not configured" — "all fields
// present" becomes "any field missing", i.e. OR. Before this extraction,
// three of the four config-bearing call sites got that wrong in three
// different ways: BigQuery enterprise_oauth and Synapse used `&&` (so a
// half-filled config — exactly what an interrupted admin leaves behind —
// still showed a normal Connect button, because BOTH fields had to be
// empty to trip the warning), and Snowflake was hardcoded
// `Signal::stored(false)` (never warned at all — it had no
// `cfg_oauth_client_id`/`cfg_oauth_client_secret` signals in scope to
// read in the first place, having never received them as props).
// Databricks alone already had the correct `||`. This predicate is now
// the single place the check is spelled out; the guard test below pins
// that a fifth surface can't reintroduce a hand-rolled copy.

use super::super::oauth_config_missing;

#[test]
fn oauth_config_missing_true_when_neither_field_set() {
    assert!(
        oauth_config_missing("", ""),
        "neither client_id nor client_secret set must be reported as missing"
    );
}

#[test]
fn oauth_config_missing_true_when_only_client_id_set() {
    // This is the case the pre-KYO-519 `&&` formula (BigQuery
    // enterprise_oauth, Synapse) got wrong: `"id".is_empty() && "".is_empty()`
    // short-circuits to false on the first operand, so `&&` reported this
    // half-filled config as "configured" even though the secret is blank
    // and any connect attempt would fail server-side.
    assert!(
        oauth_config_missing("client-id-abc", ""),
        "client_id set but client_secret empty must still be reported as missing"
    );
}

#[test]
fn oauth_config_missing_true_when_only_client_secret_set() {
    assert!(
        oauth_config_missing("", "shh-secret"),
        "client_secret set but client_id empty must still be reported as missing"
    );
}

#[test]
fn oauth_config_missing_false_when_both_fields_set() {
    assert!(
        !oauth_config_missing("client-id-abc", "shh-secret"),
        "both fields set means OAuth is fully configured — must not be reported as missing"
    );
}

/// Extracts the four `*AuthModeSection` component bodies, in the same
/// boundaries `auth_mode_sections.rs` already establishes (Synapse's
/// section has no fifth sibling function to anchor on, so its end marker
/// is the next item's `struct` keyword rather than a `fn`).
fn config_bearing_auth_mode_sections(src: &str) -> [(&'static str, &str); 4] {
    [
        (
            "BigQueryAuthModeSection",
            extract_between(src, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection("),
        ),
        (
            "SnowflakeAuthModeSection",
            extract_between(src, "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ),
        (
            "DatabricksAuthModeSection",
            extract_between(src, "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ),
        (
            "SynapseAuthModeSection",
            extract_between(src, "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
        ),
    ]
}

#[test]
fn all_four_config_bearing_surfaces_route_through_oauth_config_missing() {
    const CALL: &str =
        "oauth_config_missing(&cfg_oauth_client_id.get(), &cfg_oauth_client_secret.get())";

    for (name, section) in config_bearing_auth_mode_sections(SRC) {
        let call_count = section.matches(CALL).count();
        assert_eq!(
            call_count, 1,
            "{name} must derive its cfg_missing signal from exactly one call to \
             oauth_config_missing(...) — found {call_count}. Zero means this surface \
             lost (or never had) the predicate call — the exact KYO-519 Snowflake \
             defect, where the section had no cfg_oauth_client_id/secret signals in \
             scope at all; more than one suggests a second, possibly-drifted copy"
        );

        // The regression this guards against: a call site re-deriving the
        // emptiness check inline instead of calling the shared predicate —
        // whether spelled with `&&` (BigQuery enterprise_oauth, Synapse,
        // pre-KYO-519) or `||` (what Databricks already had, which was
        // correct but still an independent copy).
        assert!(
            !section.contains("cfg_oauth_client_id.get().is_empty()"),
            "{name} must not re-derive the OAuth-config emptiness check inline via \
             a raw `.get().is_empty()` read — that inline re-derivation is exactly \
             the failure mode oauth_config_missing exists to prevent from \
             recurring on a fifth surface (docs/standards/code-organization/\
             propagate-predicate-changes-to-every-copy.md)"
        );
    }
}

#[test]
fn snowflake_cfg_missing_is_derived_not_hardcoded_false() {
    let snowflake = extract_between(
        SRC,
        "fn SnowflakeAuthModeSection(",
        "fn DatabricksAuthModeSection(",
    );
    assert!(
        !snowflake.contains("cfg_missing=Signal::stored(false)"),
        "SnowflakeAuthModeSection's OAuth status panel must not hardcode \
         cfg_missing=Signal::stored(false) — that was the KYO-519 defect: this \
         component never received cfg_oauth_client_id/secret as props, so a member \
         always saw a normal-looking Connect button regardless of whether the admin \
         had configured OAuth, and clicking it started a flow that could not succeed"
    );
    assert!(
        snowflake.contains("cfg_missing=sf_cfg_missing"),
        "SnowflakeAuthModeSection's OAuth status panel must read cfg_missing from \
         the derived sf_cfg_missing signal"
    );
}

#[test]
fn bigquery_kyomi_oauth_is_the_only_surviving_stored_false_exception() {
    // BigQuery's kyomi_oauth mode is the one deliberate exception: it's
    // account-level, globally-hosted Kyomi OAuth with no configFields at
    // all, so there is nothing for oauth_config_missing to check.
    let bigquery = extract_between(SRC, "fn BigQueryAuthModeSection(", "fn SnowflakeAuthModeSection(");
    let stored_false_count = bigquery.matches("cfg_missing=Signal::stored(false)").count();
    assert_eq!(
        stored_false_count, 1,
        "expected exactly one cfg_missing=Signal::stored(false) inside \
         BigQueryAuthModeSection — the documented kyomi_oauth exception. Zero means \
         the exception was removed (and kyomi_oauth is now probably reading the \
         wrong, enterprise_oauth-scoped signals); more than one means \
         enterprise_oauth also regressed to a hardcoded false"
    );

    // None of the other three config-bearing surfaces may hardcode false —
    // each has real configFields to check.
    for (name, section) in [
        (
            "SnowflakeAuthModeSection",
            extract_between(SRC, "fn SnowflakeAuthModeSection(", "fn DatabricksAuthModeSection("),
        ),
        (
            "DatabricksAuthModeSection",
            extract_between(SRC, "fn DatabricksAuthModeSection(", "fn SynapseAuthModeSection("),
        ),
        (
            "SynapseAuthModeSection",
            extract_between(SRC, "fn SynapseAuthModeSection(", "struct ConnectionFieldsSignals"),
        ),
    ] {
        assert!(
            !section.contains("cfg_missing=Signal::stored(false)"),
            "{name} must not hardcode cfg_missing=Signal::stored(false) — only \
             BigQuery's kyomi_oauth mode has no configFields to check"
        );
    }
}

// ── KYO-714: the disconnect toast must not over-claim ────────────────

/// `datasource_disconnect_message` is the whole of AC4: the toast may only
/// say "disconnected" when the grant is actually gone at the provider.
/// Snowflake, Databricks and Microsoft Enterprise have no revocation
/// mechanism Kyomi can call, so their disconnect yields `NotSupported` and
/// must tell the user the grant is still live and where to finish the job.
#[test]
fn not_supported_outcome_does_not_claim_the_account_was_disconnected() {
    use super::super::datasource_disconnect_message;
    use crate::server_fns::datasource_oauth::DatasourceOAuthRevocationOutcome as Outcome;

    let message = datasource_disconnect_message(Outcome::NotSupported);

    assert!(
        !message.contains("Account disconnected"),
        "the no-revocation outcome must not reuse the wording that claims the \
         account was disconnected: {message}"
    );
    assert!(
        message.contains("revoke"),
        "the user must be told the grant is still live and needs revoking at the \
         provider: {message}"
    );
    assert!(
        message.contains("account settings"),
        "the user must be told where to finish revoking: {message}"
    );
}

/// The three outcomes that really do leave nothing behind at the provider
/// keep the original wording — this change must not degrade the honest case
/// into a hedge.
#[test]
fn outcomes_that_cleared_the_grant_keep_the_plain_wording() {
    use super::super::datasource_disconnect_message;
    use crate::server_fns::datasource_oauth::DatasourceOAuthRevocationOutcome as Outcome;

    for outcome in [
        Outcome::Revoked,
        Outcome::AlreadyInvalid,
        Outcome::NoStoredToken,
    ] {
        assert_eq!(
            datasource_disconnect_message(outcome),
            "Account disconnected",
            "{outcome:?} leaves nothing live at the provider, so the plain wording \
             is true and must be kept"
        );
    }
}

/// The Effect that fires the toast must route through
/// `datasource_disconnect_message` rather than passing a literal. A literal
/// here is exactly the pre-KYO-714 defect, and it would be invisible to the
/// two tests above — they'd still pass against a function nobody calls.
#[test]
fn disconnect_effect_derives_its_toast_from_the_revocation_outcome() {
    let effect = extract_between(
        SRC,
        "if let Some(result) = datasource_disconnect_action.value().get() {",
        "let ssh_key_action = Action::new(",
    );

    assert!(
        effect.contains("toast_success(datasource_disconnect_message(result.revocation))"),
        "the datasource disconnect Effect must derive its toast from the \
         revocation outcome, not hardcode a success string"
    );
}
