//! `ConnectionTestResultBadge` call sites and the failure arm that
//! must render the server's specific reason (KYO-469).

use super::{extract_between, SRC};

// ── KYO-469: connection test failures rendered "Failed" with the ──────
// ── server's specific reason discarded ─────────────────────────────

/// **The point of KYO-469, half 1.** `ConnectionTestResultBadge`'s
/// failure arm must read `TestConnectionResult::message` — the
/// server's sanitized, specific failure reason — not render only a
/// hardcoded heading. Before this component existed, a BigQuery
/// service_account validation failure (malformed JSON, wrong
/// project_id, disabled key, BigQuery API not enabled, missing IAM
/// role, revoked key — six distinct, user-fixable causes) rendered the
/// same two-syllable word no matter which one occurred, because the
/// failure arm read only `TestConnectionResult::success`.
#[test]
fn connection_test_result_badge_failure_arm_renders_server_message() {
    let badge_fn = extract_between(
        SRC,
        "fn ConnectionTestResultBadge(",
        "fn BqProjectField(",
    );
    assert!(
        badge_fn.contains("r.message"),
        "ConnectionTestResultBadge's failure branch must read \
         TestConnectionResult::message — rendering only a hardcoded heading silently \
         discards the server's specific, sanitized failure reason (KYO-469)"
    );
}

/// The generic Test & Discover site (used by every non-BigQuery
/// provider, and BigQuery's own OAuth modes) must render its result
/// through the shared `ConnectionTestResultBadge`, not an inline copy
/// of the success/failure arms.
#[test]
fn generic_test_and_discover_site_uses_connection_test_result_badge() {
    let site = extract_between(
        SRC,
        "\"Test & Discover\"",
        "\"Validate connection and discover available resources\"",
    );
    assert!(
        site.contains("<ConnectionTestResultBadge"),
        "the generic Test & Discover site must render its result through \
         ConnectionTestResultBadge (KYO-469), not an inline duplicate of the \
         success/failure arms"
    );
}

/// The BigQuery service_account "Validate & Discover Projects" site —
/// the exact site the KYO-469 bug report was about — must render its
/// result through the same shared component.
#[test]
fn bigquery_validate_and_discover_site_uses_connection_test_result_badge() {
    let f = extract_between(
        SRC,
        "fn BigQueryAuthModeSection(",
        "fn SnowflakeAuthModeSection(",
    );
    assert!(
        f.contains("<ConnectionTestResultBadge"),
        "the BigQuery service_account Validate & Discover Projects site must render its \
         result through ConnectionTestResultBadge (KYO-469), not an inline duplicate of \
         the success/failure arms"
    );
}
