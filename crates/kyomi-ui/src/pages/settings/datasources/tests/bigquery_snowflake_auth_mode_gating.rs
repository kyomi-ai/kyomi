//! KYO-702 — switching a datasource's auth mode left the previous mode's
//! secret sitting in `connection_config` forever, because
//! `build_connection_config`'s `"bigquery"` and `"snowflake"` arms wrote
//! `oauth_client_id`/`oauth_client_secret` (and, for BigQuery,
//! `service_account_json`) unconditionally instead of gating them on the
//! active auth mode — unlike the `"databricks"` and `"synapse"` arms in the
//! same file, which already gated their equivalent writes correctly.
//!
//! The authoritative fix is server-side (`kyomi-auth`
//! `credential_service::strip_inactive_auth_mode_fields`, called from both
//! `create_datasource` and `update_datasource` in `datasource_service.rs`)
//! — `kyomi-core`, and therefore the registry data that drives it, is an
//! `ssr`-only dependency of `kyomi-ui` and is not compiled into the WASM
//! hydrate build, so equivalent enforcement cannot live here. These tests
//! pin the client-side half: the `"bigquery"` and `"snowflake"` arms now
//! gate their oauth/service-account writes on the active mode, matching the
//! `"databricks"`/`"synapse"` arms they were inconsistent with.

use super::{build_connection_config_body, extract_between};

// ── BigQuery: oauth pair gated on "enterprise_oauth", JSON gated on "service_account" ──

#[test]
fn bigquery_arm_gates_oauth_pair_on_enterprise_oauth_mode() {
    let body = build_connection_config_body();
    let bigquery_arm = extract_between(body, "\"bigquery\" => {", "_ => {}");

    assert!(
        bigquery_arm.contains("if bq_mode == \"enterprise_oauth\" {"),
        "build_connection_config's \"bigquery\" arm must gate its \
         oauth_client_id/oauth_client_secret writes on bq_mode == \
         \"enterprise_oauth\" — writing them unconditionally leaves a \
         leftover client secret in connection_config after switching to \
         \"service_account\" (KYO-702)"
    );

    // The oauth pair's map.insert calls must appear strictly after the
    // enterprise_oauth gate opens, i.e. inside it rather than above it.
    let gate_pos = bigquery_arm
        .find("if bq_mode == \"enterprise_oauth\" {")
        .expect("gate must be present");
    let oauth_id_pos = bigquery_arm
        .find("map.insert(\"oauth_client_id\".to_string()")
        .expect("oauth_client_id write must be present");
    let oauth_secret_pos = bigquery_arm
        .find("map.insert(\"oauth_client_secret\".to_string()")
        .expect("oauth_client_secret write must be present");
    assert!(
        oauth_id_pos > gate_pos && oauth_secret_pos > gate_pos,
        "oauth_client_id/oauth_client_secret writes must be inside the \
         enterprise_oauth gate, not above it (KYO-702)"
    );
}

#[test]
fn bigquery_arm_gates_service_account_json_on_service_account_mode() {
    let body = build_connection_config_body();
    let bigquery_arm = extract_between(body, "\"bigquery\" => {", "_ => {}");

    assert!(
        bigquery_arm.contains(
            "if bq_mode == \"service_account\" && !cfg_service_account_json.get_untracked().is_empty() {"
        ),
        "build_connection_config's \"bigquery\" arm must gate its \
         service_account_json write on bq_mode == \"service_account\" — \
         writing it unconditionally leaves a leftover (and potentially real) \
         service account key in connection_config after switching to \
         \"enterprise_oauth\" (KYO-702)"
    );
}

// ── Snowflake: oauth pair gated on "oauth" ──────────────────────────────

#[test]
fn snowflake_arm_gates_oauth_pair_on_oauth_mode() {
    let body = build_connection_config_body();
    let snowflake_arm = extract_between(body, "\"snowflake\" => {", "\"databricks\" => {");

    assert!(
        snowflake_arm.contains("if sf_auth_mode.get_untracked() == \"oauth\" {"),
        "build_connection_config's \"snowflake\" arm must gate its \
         oauth_client_id/oauth_client_secret writes on \
         sf_auth_mode == \"oauth\" — Snowflake's OAuth mode_id is \"oauth\", \
         not \"enterprise_oauth\" (see datasource_registry.rs's \
         oauth_auth_mode(\"snowflake\", ...)); writing the pair \
         unconditionally left a leftover client secret in connection_config \
         after switching to \"password\" or \"keypair\" (KYO-702)"
    );

    let gate_pos = snowflake_arm
        .find("if sf_auth_mode.get_untracked() == \"oauth\" {")
        .expect("gate must be present");
    let oauth_id_pos = snowflake_arm
        .find("map.insert(\"oauth_client_id\".to_string()")
        .expect("oauth_client_id write must be present");
    let oauth_secret_pos = snowflake_arm
        .find("map.insert(\"oauth_client_secret\".to_string()")
        .expect("oauth_client_secret write must be present");
    assert!(
        oauth_id_pos > gate_pos && oauth_secret_pos > gate_pos,
        "oauth_client_id/oauth_client_secret writes must be inside the \
         \"oauth\" gate, not above it (KYO-702)"
    );
}

// ── Regression guard: the Databricks/Synapse gates this was modeled on ──

#[test]
fn databricks_and_synapse_arms_still_gate_their_oauth_pair() {
    // KYO-702's fix mirrors these two arms, which were already correct.
    // Pin that they still are, so a future edit can't regress the
    // reference implementation while "fixing" bigquery/snowflake.
    let body = build_connection_config_body();

    let databricks_arm = extract_between(body, "\"databricks\" => {", "\"sqlserver\" => {");
    assert!(
        databricks_arm.contains("if db_auth_mode.get_untracked() == \"oauth\" {"),
        "the databricks arm's oauth gate must remain in place"
    );

    let synapse_arm = extract_between(body, "\"synapse\" => {", "\"bigquery\" => {");
    assert!(
        synapse_arm.contains("if syn_mode == \"enterprise_oauth\" {"),
        "the synapse arm's enterprise_oauth gate must remain in place"
    );
}
