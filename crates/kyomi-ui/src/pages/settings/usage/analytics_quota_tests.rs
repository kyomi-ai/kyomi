// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::server_fns::usage::AnalyticsEventsUsage;
use crate::utils::permissions::analytics_quota_applies;

fn cloud_usage() -> UsageData {
    let mut data = UsageData::unmetered();
    data.analytics_events = Some(AnalyticsEventsUsage {
        events_used: 12_345,
        events_included: 100_000,
        bundle_events: 5_678,
    });
    data
}

#[test]
fn self_hosted_usage_and_error_fallback_have_no_analytics_quota_card() {
    assert!(!analytics_quota_applies(true));
    let data = UsageData::unmetered();
    assert!(data.analytics_events.is_none());
    assert!(data.allowed);
    assert!(!data.blocked);
    let owner = Owner::new();
    owner.with(|| {
        let html = view! { <UsageContent data=data is_owner=true is_self_hosted=true/> }.to_html();
        assert!(!html.contains("Analytics Events"), "{html}");
        assert!(!html.contains("100,000"), "{html}");
        assert!(
            html.contains("AI Usage"),
            "AI usage must still render: {html}"
        );
    });
}

#[test]
fn cloud_usage_preserves_nonzero_metering_and_allowance() {
    assert!(analytics_quota_applies(false));
    let data = cloud_usage();
    let owner = Owner::new();
    owner.with(|| {
        let html = view! { <UsageContent data=data is_owner=true is_self_hosted=false/> }.to_html();
        for expected in ["Analytics Events", "12,345", "100,000", "5,678"] {
            assert!(html.contains(expected), "missing {expected}: {html}");
        }
    });
}

#[test]
fn both_usage_server_functions_use_the_shared_quota_loader() {
    for (source, start, end) in [
        (
            include_str!("../../../server_fns/usage.rs"),
            "pub async fn get_ai_usage_status()",
            "async fn cloud_ai_usage_status",
        ),
        (
            include_str!("../../../server_fns/analytics.rs"),
            "pub async fn get_analytics_usage()",
            "async fn cloud_analytics_usage",
        ),
    ] {
        let body = crate::test_support::extract_between(source, start, end);
        assert!(
            body.contains("load_with_analytics_quota(ac.ctx.config.self_hosted"),
            "both endpoints must gate before Cloud billing/quota work: {body}"
        );
    }
}

#[test]
fn analytics_quota_transport_preserves_cloud_fields_and_self_hosted_absence() {
    let self_hosted = serde_json::to_value(UsageData::unmetered()).expect("serialize usage");
    for field in [
        "analytics_events_used",
        "analytics_events_included",
        "analytics_bundle_events",
    ] {
        assert!(
            self_hosted.get(field).is_none(),
            "self-hosted has no {field} entitlement"
        );
    }
    let decoded: UsageData =
        serde_json::from_value(self_hosted).expect("deserialize self-hosted usage");
    assert!(decoded.analytics_events.is_none());

    let data = cloud_usage();
    let cloud = serde_json::to_value(data).expect("serialize Cloud usage");
    assert_eq!(cloud["analytics_events_used"], 12_345);
    assert_eq!(cloud["analytics_events_included"], 100_000);
    assert_eq!(cloud["analytics_bundle_events"], 5_678);
    let decoded: UsageData = serde_json::from_value(cloud).expect("deserialize Cloud usage");
    let quota = decoded
        .analytics_events
        .expect("Cloud quota must survive transport");
    assert_eq!(quota.events_used, 12_345);
    assert_eq!(quota.events_included, 100_000);
    assert_eq!(quota.bundle_events, 5_678);
}

#[test]
fn self_hosted_hides_even_a_stale_nonzero_cloud_quota_response() {
    let data = cloud_usage();
    let owner = Owner::new();
    owner.with(|| {
        let html = view! { <UsageContent data=data is_owner=true is_self_hosted=true/> }.to_html();
        assert!(!html.contains("Analytics Events"), "{html}");
        assert!(!html.contains("100,000"), "{html}");
    });
}
