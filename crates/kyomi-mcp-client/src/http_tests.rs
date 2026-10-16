// SPDX-License-Identifier: AGPL-3.0-or-later
use super::*;
use axum::{Router, response::IntoResponse, routing::any};
use std::sync::atomic::{AtomicUsize, Ordering};

fn request(url: String) -> HttpRequest {
    HttpRequest {
        method: Method::POST,
        url,
        authorization: Some("Bearer synthetic-fixture".into()),
        content_type: Some("application/json".into()),
        body: br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#.to_vec(),
        mcp_headers: Vec::new(),
    }
}
async fn server(
    response: impl IntoResponse + Clone + Send + Sync + 'static,
) -> (String, tokio::task::JoinHandle<()>) {
    let app = Router::new().route(
        "/mcp",
        any(move || {
            let response = response.clone();
            async move { response }
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, handle)
}
fn fixture_client(limits: Limits) -> SafeHttpClient {
    let mut client = SafeHttpClient::new(limits, DeploymentPolicy::default()).unwrap();
    client.test_http = true;
    client
}
#[tokio::test]
async fn real_http_body_and_headers_are_bounded() {
    let (url, handle) = server((
        [
            ("content-type", "application/json"),
            (
                "www-authenticate",
                "Bearer resource_metadata=\"https://example.com/meta\"",
            ),
        ],
        "{}",
    ))
    .await;
    let response = fixture_client(Limits::default())
        .execute(request(url.clone()))
        .await
        .unwrap();
    assert_eq!(response.body, b"{}");
    assert!(
        response
            .www_authenticate
            .unwrap()
            .contains("resource_metadata")
    );
    let limits = Limits {
        body_bytes: 1,
        ..Limits::default()
    };
    assert_eq!(
        fixture_client(limits)
            .execute(request(url))
            .await
            .err()
            .unwrap()
            .kind,
        K::Limit
    );
    handle.abort();
}
#[tokio::test]
async fn redirects_never_forward_credentials() {
    let (url, handle) = server((
        axum::http::StatusCode::FOUND,
        [("location", "https://example.com/stolen")],
        "",
    ))
    .await;
    assert_eq!(
        fixture_client(Limits::default())
            .execute(request(url))
            .await
            .err()
            .unwrap()
            .kind,
        K::Redirect
    );
    handle.abort();
}
#[tokio::test]
async fn sse_frame_limit_and_final_response_without_eof() {
    let frame = "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n";
    let (url, handle) = server(([("content-type", "text/event-stream")], frame)).await;
    let limits = Limits {
        sse_frame_bytes: 8,
        ..Limits::default()
    };
    assert_eq!(
        fixture_client(limits)
            .execute(request(url))
            .await
            .err()
            .unwrap()
            .kind,
        K::Limit
    );
    handle.abort();
    let app = Router::new().route(
        "/mcp",
        any(move || async move {
            let stream = futures_util::stream::once(async move { Ok::<_, std::io::Error>(frame) })
                .chain(futures_util::stream::pending());
            (
                [("content-type", "text/event-stream")],
                axum::body::Body::from_stream(stream),
            )
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let response = fixture_client(Limits::default())
        .execute(request(url))
        .await
        .unwrap();
    assert_eq!(response.body, frame.as_bytes());
    handle.abort();
}
struct AnswerResolver {
    answers: Vec<SocketAddr>,
    calls: AtomicUsize,
}
#[async_trait]
impl Resolver for AnswerResolver {
    async fn resolve(&self, _: &str, _: u16) -> Result<Vec<SocketAddr>, Error> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.answers.clone())
    }
}
#[tokio::test]
async fn blocked_dns_answers_never_connect_and_are_checked_every_time() {
    for addresses in [
        vec![SocketAddr::from(([127, 0, 0, 1], 443))],
        vec![
            SocketAddr::from(([192, 0, 2, 1], 443)),
            SocketAddr::from(([10, 0, 0, 1], 443)),
        ],
        Vec::new(),
    ] {
        let resolver = Arc::new(AnswerResolver {
            answers: addresses,
            calls: AtomicUsize::new(0),
        });
        let client = SafeHttpClient::with_resolver(
            Limits::default(),
            DeploymentPolicy::default(),
            resolver.clone(),
        )
        .unwrap();
        for _ in 0..2 {
            let error = client
                .execute(request("https://mcp.example.com/mcp".into()))
                .await
                .err()
                .unwrap();
            assert_eq!(error.kind, K::UnsafeUrl);
            assert_eq!(error.dispatch, D::NotSent);
        }
        assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    }
}
#[tokio::test]
async fn hostile_schemes_credentials_and_private_literals_fail_before_dispatch() {
    let client = SafeHttpClient::new(Limits::default(), DeploymentPolicy::default()).unwrap();
    for url in [
        "http://example.com/mcp",
        "file:///etc/passwd",
        "https://user:secret@example.com/mcp",
        "https://127.0.0.1/mcp",
        "https://[::ffff:127.0.0.1]/mcp",
        "https://169.254.169.254/latest/meta-data",
        "https://[::1]/mcp",
        "https://example.com/#fragment",
    ] {
        let error = client.execute(request(url.into())).await.err().unwrap();
        assert_eq!(error.kind, K::UnsafeUrl, "{url}");
        assert_eq!(error.dispatch, D::NotSent);
    }
}
#[test]
fn addresses_cover_private_transition_and_reserved_ranges() {
    for ip in [
        "0.0.0.0",
        "10.1.2.3",
        "127.0.0.1",
        "169.254.169.254",
        "172.31.0.1",
        "192.168.1.1",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fe80::1",
        "fc00::1",
        "::ffff:10.0.0.1",
        "2001:db8::1",
        "2002::1",
        "64:ff9b::1",
    ] {
        assert!(!is_public_address(ip.parse().unwrap()), "{ip}");
    }
    // Shared-carrier and benchmarking ranges are not public destinations.
    for octets in [[100, 64, 0, 1], [198, 18, 0, 1]] {
        assert!(!is_public_address(IpAddr::V4(std::net::Ipv4Addr::from(
            octets
        ))));
    }
    // Build synthetic public-unicast samples, without naming live service addresses.
    assert!(is_public_address(IpAddr::V4(std::net::Ipv4Addr::new(
        11, 1, 2, 3
    ))));
    assert!(is_public_address(IpAddr::V6(std::net::Ipv6Addr::new(
        0x2600, 1, 2, 3, 0, 0, 0, 1
    ))));
}
#[test]
fn admin_private_policy_is_exact_origin_and_https_only() {
    assert!(DeploymentPolicy::allow_private_https_origins(&["http://example.com".into()]).is_err());
    assert!(
        DeploymentPolicy::allow_private_https_origins(&["https://example.com/path".into()])
            .is_err()
    );
    let policy =
        DeploymentPolicy::allow_private_https_origins(&["https://example.com:8443".into()])
            .unwrap();
    assert!(
        policy
            .private_authorities
            .contains("https://example.com:8443")
    );
    assert!(!policy.private_authorities.contains("https://example.com"));
}
#[tokio::test]
async fn deadline_and_concurrency_return_unknown_and_busy() {
    let app = Router::new().route(
        "/mcp",
        any(|| async {
            std::future::pending::<()>().await;
            ""
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = fixture_client(Limits {
        deadline: std::time::Duration::from_millis(30),
        concurrency: 1,
        ..Limits::default()
    });
    let permit = client.slots.acquire().await.unwrap();
    assert_eq!(
        client
            .execute(request(url.clone()))
            .await
            .err()
            .unwrap()
            .kind,
        K::Busy
    );
    drop(permit);
    let error = client.execute(request(url)).await.err().unwrap();
    assert_eq!(error.kind, K::Deadline);
    assert_eq!(error.dispatch, D::MayHaveBeenSent);
    handle.abort();
}

struct RebindingResolver {
    port: u16,
    calls: AtomicUsize,
}
#[async_trait]
impl Resolver for RebindingResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, Error> {
        assert_eq!(host, "fixture.invalid");
        assert_eq!(port, self.port);
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![SocketAddr::from((
            if call == 0 {
                [127, 0, 0, 1]
            } else {
                [169, 254, 169, 254]
            },
            port,
        ))])
    }
}
#[tokio::test]
async fn hostname_connection_is_dns_pinned_and_rebinding_never_reuses_a_connection() {
    let hits = Arc::new(AtomicUsize::new(0));
    let captured = hits.clone();
    let app = Router::new().route(
        "/mcp",
        any(move |headers: axum::http::HeaderMap| {
            let hits = captured.clone();
            async move {
                assert!(
                    headers["host"]
                        .to_str()
                        .unwrap()
                        .starts_with("fixture.invalid:")
                );
                hits.fetch_add(1, Ordering::SeqCst);
                ([("content-type", "application/json")], "{}")
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let resolver = Arc::new(RebindingResolver {
        port,
        calls: AtomicUsize::new(0),
    });
    let mut client = SafeHttpClient::with_resolver(
        Limits::default(),
        DeploymentPolicy::default(),
        resolver.clone(),
    )
    .unwrap();
    client.test_http = true;
    let url = format!("http://fixture.invalid:{port}/mcp");
    assert_eq!(
        client.execute(request(url.clone())).await.unwrap().body,
        b"{}"
    );
    // The socket accepted the pinned answer while retaining the hostname authority.
    // A second lookup changes to metadata/private space and must never dispatch.
    let error = client.execute(request(url)).await.err().unwrap();
    assert_eq!(error.kind, K::UnsafeUrl);
    assert_eq!(error.dispatch, D::NotSent);
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 2);
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    handle.abort();
}
