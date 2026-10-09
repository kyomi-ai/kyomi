// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::server_fns::usage::{AnalyticsEventsUsage, UsageData};
use std::cell::Cell;

#[tokio::test]
async fn self_hosted_usage_endpoints_share_absence_without_loading_cloud_data() {
    let calls = Cell::new(0);
    let analytics: Option<AnalyticsUsageData> = load_with_analytics_quota(true, || {
        calls.set(calls.get() + 1);
        async { Err::<AnalyticsUsageData, _>(ServerFnError::new("must not load Cloud quota")) }
    })
    .await
    .expect("self-hosted has no quota lookup");
    assert!(analytics.is_none());

    let usage = load_with_analytics_quota(true, || {
        calls.set(calls.get() + 1);
        async { Err::<UsageData, _>(ServerFnError::new("must not load Cloud billing")) }
    })
    .await
    .expect("self-hosted has no Cloud billing")
    .unwrap_or_else(UsageData::unmetered);
    assert!(usage.analytics_events.is_none());
    assert!(usage.allowed);
    assert_eq!(calls.get(), 0);
}

#[tokio::test]
async fn cloud_usage_endpoints_keep_nonzero_quota_results() {
    let analytics = load_with_analytics_quota(false, || async {
        Ok::<_, ServerFnError>(AnalyticsUsageData {
            events_used: 12_345,
            events_limit: 100_000,
            usage_percent: 12.345,
            bundle_balance: 5_678,
            status: "ok".to_string(),
        })
    })
    .await
    .expect("Cloud loader succeeds")
    .expect("Cloud quota applies");
    assert_eq!(analytics.events_used, 12_345);
    assert_eq!(analytics.events_limit, 100_000);
    assert_eq!(analytics.bundle_balance, 5_678);

    let usage = load_with_analytics_quota(false, || async {
        let mut data = UsageData::unmetered();
        data.analytics_events = Some(AnalyticsEventsUsage {
            events_used: 12_345,
            events_included: 100_000,
            bundle_events: 5_678,
        });
        Ok::<_, ServerFnError>(data)
    })
    .await
    .expect("Cloud loader succeeds")
    .unwrap_or_else(UsageData::unmetered);
    let quota = usage
        .analytics_events
        .expect("Cloud quota survives usage endpoint assembly");
    assert_eq!(quota.events_used, 12_345);
    assert_eq!(quota.events_included, 100_000);
    assert_eq!(quota.bundle_events, 5_678);
}

#[tokio::test]
async fn cloud_quota_load_failure_is_not_reported_as_absence() {
    let result = load_with_analytics_quota(false, || async {
        Err::<AnalyticsUsageData, _>(ServerFnError::new("metering failed"))
    })
    .await;
    assert!(result.is_err());
}
