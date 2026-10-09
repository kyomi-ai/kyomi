// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use std::cell::Cell;

#[tokio::test]
async fn inapplicable_analytics_views_never_fetch_quota() {
    for access in [
        AnalyticsAccess::SelfHosted,
        AnalyticsAccess::Denied,
        AnalyticsAccess::BillingDisabled,
    ] {
        let calls = Cell::new(0);
        let result = fetch_analytics_usage(access, || {
            calls.set(calls.get() + 1);
            async { Err(ServerFnError::new("must never fetch")) }
        })
        .await
        .expect("an unavailable view should skip the request");
        assert!(result.is_none());
        assert_eq!(calls.get(), 0, "quota fetched for {access:?}");
    }
}

#[tokio::test]
async fn cloud_analytics_fetch_preserves_nonzero_usage() {
    let calls = Cell::new(0);
    let result = fetch_analytics_usage(AnalyticsAccess::Allowed, || {
        calls.set(calls.get() + 1);
        async {
            Ok(Some(AnalyticsUsageData {
                events_used: 12_345,
                events_limit: 100_000,
                usage_percent: 12.345,
                bundle_balance: 5_678,
                status: "ok".to_string(),
            }))
        }
    })
    .await
    .expect("Cloud fetch should succeed")
    .expect("Cloud quota must exist");
    assert_eq!(calls.get(), 1);
    assert_eq!(result.events_used, 12_345);
    assert_eq!(result.events_limit, 100_000);
    assert_eq!(result.bundle_balance, 5_678);
}

#[tokio::test]
async fn cloud_analytics_fetch_preserves_errors() {
    assert!(
        fetch_analytics_usage(AnalyticsAccess::Allowed, || async {
            Err(ServerFnError::new("quota unavailable"))
        })
        .await
        .is_err()
    );
}

#[tokio::test]
async fn analytics_page_ssr_constructs_local_quota_resource_and_renders_fallback() {
    use axum::{body::Body, http::Request};
    use std::time::Duration;

    let _ = any_spawner::Executor::init_tokio();
    tokio::task::LocalSet::new()
        .run_until(async {
            // Use the real integration to provide the SSR shared context.
            // Match SettingsShell's client-only UserContext, which remains
            // pending on the server. AnalyticsPage must establish Transition
            // before awaiting it; the creation owner has no Suspense notifier.
            let handler = leptos_axum::render_app_to_stream_in_order_with_context(
                || {
                    let user_ctx = LocalResource::new(|| {
                        std::future::pending::<Result<UserContext, ServerFnError>>()
                    });
                    provide_context(user_ctx);
                },
                || view! { <AnalyticsPage/> },
            );
            let request = Request::builder()
                .uri("/settings/analytics")
                .body(Body::empty())
                .expect("valid SSR request");
            let html = tokio::time::timeout(Duration::from_secs(2), async {
                let response = handler(request).await;
                assert_eq!(response.status(), axum::http::StatusCode::OK);
                let body = axum::body::to_bytes(response.into_body(), 128 * 1024)
                    .await
                    .expect("SSR response body");
                String::from_utf8(body.to_vec()).expect("SSR response is HTML")
            })
            .await
            .expect("SSR must render fallback instead of waiting on client-only context");
            assert!(
                html.contains("Analytics"),
                "SSR must render the page heading"
            );
            assert!(
                html.contains("animate-pulse"),
                "SSR must render the loading skeleton"
            );
            assert!(
                !html.contains("Events This Month"),
                "pending local data cannot render a quota"
            );
        })
        .await;
}
