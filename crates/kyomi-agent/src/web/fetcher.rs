// SPDX-License-Identifier: AGPL-3.0-or-later

//! Kyomi's own web fetcher (`WEB_FETCH_PROVIDER=builtin`).
//!
//! The model chooses the URL, so this is an SSRF surface. Defences, per hop:
//!
//! * scheme must be `http`/`https`, no credentials in the URL, port 80/443 only;
//! * the host is resolved here and **every** answer must be a public unicast
//!   address ([`kyomi_mcp_client::is_public_address`], shared with the outbound
//!   MCP client: loopback, link-local incl. cloud metadata, private, CGNAT,
//!   multicast, documentation and transition ranges are refused);
//! * the connection is pinned to those validated answers (`resolve_to_addrs`),
//!   so a second DNS lookup at connect time cannot rebind to an internal host;
//! * proxies are disabled and redirects are followed manually (max 5), each
//!   hop re-validated from scratch;
//! * the whole fetch has a deadline and the body a byte cap.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_util::StreamExt;
use kyomi_core::web_tools::WebBackendConfig;
use kyomi_mcp_client::{Resolver, SystemResolver, is_public_address};
use url::Url;

use super::{FetchRequest, FetchedPage, WebError, WebFetchProvider, WebUsage, truncate_chars};

const MAX_REDIRECTS: usize = 5;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(20);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_URL_LEN: usize = 2048;

pub struct BuiltinFetcher {
    resolver: Arc<dyn Resolver>,
    /// Operator domain lists (`WEB_FETCH_ALLOWED_DOMAINS` / `WEB_FETCH_BLOCKED_DOMAINS`):
    /// lower-cased, each matching itself and its subdomains.
    allowed_domains: Vec<String>,
    blocked_domains: Vec<String>,
    /// Test seam (same precedent as `kyomi-mcp-client`): lets tests reach a
    /// loopback server. Compiled out of non-test builds.
    #[cfg(test)]
    allow_loopback: bool,
}

impl BuiltinFetcher {
    pub fn new(cfg: &WebBackendConfig) -> kyomi_core::Result<Self> {
        let mut fetcher = Self::with_resolver(Arc::new(SystemResolver));
        fetcher.allowed_domains = cfg.allowed_domains.clone();
        fetcher.blocked_domains = cfg.blocked_domains.clone();
        Ok(fetcher)
    }

    pub fn with_resolver(resolver: Arc<dyn Resolver>) -> Self {
        Self {
            resolver,
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            #[cfg(test)]
            allow_loopback: false,
        }
    }

    fn is_allowed_ip(&self, ip: IpAddr) -> bool {
        #[cfg(test)]
        if self.allow_loopback && ip.is_loopback() {
            return true;
        }
        is_public_address(ip)
    }

    /// Operator policy, applied to the host of every hop (including redirects): the
    /// deny list first, then the allow list when one is configured.
    fn check_domain_policy(&self, host: &str) -> Result<(), WebError> {
        let host = host.trim_matches(['[', ']']).trim_end_matches('.').to_lowercase();
        let matches = |d: &String| host == *d || host.ends_with(&format!(".{d}"));
        if self.blocked_domains.iter().any(matches) {
            return Err(WebError::Blocked("this domain is blocked by the operator".into()));
        }
        if !self.allowed_domains.is_empty() && !self.allowed_domains.iter().any(matches) {
            return Err(WebError::Blocked("this domain is not on the operator's allow list".into()));
        }
        Ok(())
    }

    fn check_url(&self, raw: &str) -> Result<Url, WebError> {
        let blocked = |m: &str| WebError::Blocked(m.to_string());
        if raw.len() > MAX_URL_LEN {
            return Err(blocked("URL is too long"));
        }
        let url = Url::parse(raw).map_err(|_| WebError::InvalidRequest("not a valid absolute URL".into()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(blocked("only http and https URLs can be fetched"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(blocked("URLs with credentials are not allowed"));
        }
        let Some(host) = url.host_str() else {
            return Err(blocked("URL has no host"));
        };
        self.check_domain_policy(host)?;
        #[cfg(test)]
        if self.allow_loopback {
            return Ok(url);
        }
        match url.port() {
            None | Some(80) | Some(443) => Ok(url),
            Some(_) => Err(blocked("only ports 80 and 443 can be fetched")),
        }
    }

    /// Resolve and validate (shared pinned-egress code in `kyomi-mcp-client`);
    /// returns the addresses the connection is pinned to.
    async fn validated_addresses(&self, url: &Url) -> Result<(String, Vec<SocketAddr>), WebError> {
        let host = url.host_str().unwrap_or_default().trim_matches(['[', ']']).to_string();
        let port = url.port_or_known_default().unwrap_or(443);
        let addresses = kyomi_mcp_client::validated_addresses(self.resolver.as_ref(), &host, port, |ip| self.is_allowed_ip(ip))
            .await
            .map_err(|e| match e.kind {
                kyomi_mcp_client::ErrorKind::Dns => WebError::InvalidRequest("the host name could not be resolved".into()),
                _ => WebError::Blocked("the address is not a public internet address".into()),
            })?;
        Ok((host, addresses))
    }

    async fn fetch_inner(&self, request: &FetchRequest) -> Result<FetchedPage, WebError> {
        let mut url = self.check_url(&request.url)?;
        for _ in 0..=MAX_REDIRECTS {
            let (host, addresses) = self.validated_addresses(&url).await?;
            let client = kyomi_mcp_client::pinned_client(&host, &addresses, CONNECT_TIMEOUT, DEADLINE)
                .map_err(|_| WebError::Upstream("could not build the HTTP client".into()))?
                .build()
                .map_err(|e| WebError::Upstream(e.to_string()))?;
            let response = client
                .get(url.clone())
                .header("accept", "text/html, text/plain, application/json, application/xml;q=0.9, */*;q=0.1")
                .send()
                .await
                .map_err(|e| if e.is_timeout() { WebError::Timeout } else { WebError::Upstream("connection failed".into()) })?;
            // Defence in depth: the peer must be one of the validated answers.
            if !kyomi_mcp_client::peer_is_pinned(&response, &addresses) {
                return Err(WebError::Blocked("connected to an unexpected address".into()));
            }
            let status = response.status();
            if status.is_redirection() {
                let location = response
                    .headers()
                    .get("location")
                    .and_then(|v| v.to_str().ok())
                    .ok_or_else(|| WebError::Upstream("redirect without a Location header".into()))?;
                let next = url
                    .join(location)
                    .map_err(|_| WebError::InvalidRequest("redirect to an invalid URL".into()))?;
                url = self.check_url(next.as_str())?;
                continue;
            }
            if status.as_u16() == 429 {
                return Err(WebError::RateLimited { retry_after: None });
            }
            if !status.is_success() {
                return Err(WebError::Upstream(format!("the site returned HTTP {}", status.as_u16())));
            }
            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let kind = ContentKind::from_content_type(&content_type)
                .ok_or_else(|| WebError::InvalidRequest(format!("unsupported content type {content_type:?}")))?;
            if response.content_length().is_some_and(|n| n > MAX_BODY_BYTES as u64) {
                return Err(WebError::InvalidRequest("the page is too large".into()));
            }
            let mut body = Vec::new();
            let mut stream = response.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|_| WebError::Upstream("download failed".into()))?;
                if body.len() + chunk.len() > MAX_BODY_BYTES {
                    return Err(WebError::InvalidRequest("the page is too large".into()));
                }
                body.extend_from_slice(&chunk);
            }
            let text = String::from_utf8_lossy(&body);
            let (title, text) = match kind {
                ContentKind::Html => (html_title(&text), html_to_text(&text)),
                ContentKind::Text => (None, text.into_owned()),
            };
            let (content, truncated) = truncate_chars(&text, request.max_chars);
            return Ok(FetchedPage { url: url.to_string(), title, content, truncated, usage: WebUsage { calls: 1, credits: None } });
        }
        Err(WebError::InvalidRequest("too many redirects".into()))
    }
}

#[async_trait]
impl WebFetchProvider for BuiltinFetcher {
    fn name(&self) -> &str {
        "builtin"
    }

    async fn fetch(&self, request: &FetchRequest) -> Result<FetchedPage, WebError> {
        tokio::time::timeout(DEADLINE + Duration::from_secs(5), self.fetch_inner(request))
            .await
            .map_err(|_| WebError::Timeout)?
    }
}

enum ContentKind {
    Html,
    Text,
}

impl ContentKind {
    fn from_content_type(ct: &str) -> Option<Self> {
        match ct {
            "" | "text/html" | "application/xhtml+xml" => Some(Self::Html),
            "application/json" | "application/xml" | "application/ld+json" => Some(Self::Text),
            t if t.starts_with("text/") => Some(Self::Text),
            _ => None,
        }
    }
}

fn html_title(html: &str) -> Option<String> {
    let re = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").ok()?;
    let raw = re.captures(html)?.get(1)?.as_str();
    let t = decode_entities(raw).split_whitespace().collect::<Vec<_>>().join(" ");
    (!t.is_empty()).then_some(t)
}

/// Reduce HTML to readable text: drop scripts, styles and comments, turn block
/// tags into line breaks, strip the rest, decode common entities.
fn html_to_text(html: &str) -> String {
    let drop = regex::Regex::new(
        r"(?is)<head\b.*?</head\s*>|<script\b.*?</script\s*>|<style\b.*?</style\s*>|<noscript\b.*?</noscript\s*>|<template\b.*?</template\s*>|<svg\b.*?</svg\s*>|<!--.*?-->",
    )
    .expect("static regex");
    let block = regex::Regex::new(r"(?i)</?(p|div|br|li|ul|ol|h[1-6]|tr|table|section|article|header|footer|pre|blockquote)\b[^>]*>").expect("static regex");
    let tag = regex::Regex::new(r"(?s)<[^>]*>").expect("static regex");
    let s = drop.replace_all(html, " ");
    let s = block.replace_all(&s, "\n");
    let s = tag.replace_all(&s, " ");
    let s = decode_entities(&s);
    let mut out = String::new();
    let mut blank = false;
    for line in s.lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            blank = true;
            continue;
        }
        if blank && !out.is_empty() {
            out.push('\n');
        }
        blank = false;
        out.push_str(&line);
        out.push('\n');
    }
    out.trim_end().to_string()
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use kyomi_mcp_client::{DispatchCertainty, Error as McpError, ErrorKind};
    use std::collections::HashMap;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    struct MapResolver(HashMap<&'static str, Vec<IpAddr>>);

    #[async_trait]
    impl Resolver for MapResolver {
        async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, McpError> {
            self.0
                .get(host)
                .map(|ips| ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
                .ok_or_else(|| McpError::new(ErrorKind::Dns, DispatchCertainty::NotSent))
        }
    }

    fn fetcher(map: &[(&'static str, &[&str])]) -> BuiltinFetcher {
        let m = map
            .iter()
            .map(|(h, ips)| (*h, ips.iter().map(|i| i.parse().unwrap()).collect()))
            .collect();
        BuiltinFetcher::with_resolver(Arc::new(MapResolver(m)))
    }

    fn req(url: &str) -> FetchRequest {
        FetchRequest { url: url.into(), max_chars: 10_000 }
    }

    async fn blocked(f: &BuiltinFetcher, url: &str) {
        let err = f.fetch(&req(url)).await.unwrap_err();
        assert!(matches!(err, WebError::Blocked(_)), "{url} -> {err:?}");
    }

    #[tokio::test]
    async fn refuses_internal_literal_addresses_and_odd_urls() {
        let f = fetcher(&[]);
        for url in [
            "http://127.0.0.1/", "http://localhost:80/", "http://[::1]/", "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/", "http://192.168.1.1/", "http://172.16.0.1/", "http://0.0.0.0/",
            "http://[::ffff:10.0.0.1]/", "http://[fd00::1]/", "http://[fe80::1]/",
            "file:///etc/passwd", "ftp://example.com/", "http://user:pw@example.com/", "https://example.com:8443/",
        ] {
            let err = f.fetch(&req(url)).await.unwrap_err();
            assert!(matches!(err, WebError::Blocked(_) | WebError::InvalidRequest(_)), "{url} -> {err:?}");
            if !url.starts_with("http://localhost") {
                assert!(matches!(err, WebError::Blocked(_)), "{url} -> {err:?}");
            }
        }
    }

    #[tokio::test]
    async fn refuses_hostnames_resolving_to_internal_addresses_even_when_mixed() {
        let f = fetcher(&[
            ("metadata.evil.test", &["169.254.169.254"]),
            ("rebind.evil.test", &["2606:4700::1111", "10.0.0.1"]),
            ("v6.evil.test", &["::1"]),
        ]);
        blocked(&f, "http://metadata.evil.test/").await;
        blocked(&f, "http://rebind.evil.test/").await;
        blocked(&f, "https://v6.evil.test/").await;
        let err = f.fetch(&req("http://unknown.test/")).await.unwrap_err();
        assert!(matches!(err, WebError::InvalidRequest(_)));
    }

    /// A tiny origin used to exercise the real fetch path on loopback.
    async fn origin(routes: Vec<(&'static str, String)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let routes = routes.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let reply = routes
                        .iter()
                        .find(|(p, _)| *p == path)
                        .map(|(_, r)| r.clone())
                        .unwrap_or_else(|| "HTTP/1.1 404 NF\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into());
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn ok(ct: &str, body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\ncontent-type: {ct}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
    }

    fn loopback_fetcher() -> BuiltinFetcher {
        let mut f = fetcher(&[]);
        f.allow_loopback = true;
        f
    }

    /// An origin that accepts the connection and never answers. The clock is paused,
    /// so the 20 second deadline elapses in virtual time (the runtime advances it
    /// when everything is idle) instead of making the test wait.
    #[tokio::test(start_paused = true)]
    async fn an_origin_that_never_answers_ends_in_a_timeout_at_the_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let held = tokio::spawn(async move {
            let mut sockets = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                sockets.push(sock);
            }
        });
        let started = tokio::time::Instant::now();
        let err = loopback_fetcher().fetch(&req(&format!("http://{addr}/"))).await.unwrap_err();
        assert!(matches!(err, WebError::Timeout), "{err:?}");
        let waited = started.elapsed();
        assert!(waited >= DEADLINE && waited <= DEADLINE + Duration::from_secs(5), "ended at the deadline, not before or long after: {waited:?}");
        held.abort();
    }

    #[tokio::test]
    async fn fetches_html_as_text_with_title_and_strips_scripts() {
        let html = "<html><head><title> My &amp; Page </title><style>x{}</style></head><body><script>alert(1)</script><h1>Hello</h1><p>World &lt;b&gt;</p></body></html>";
        let base = origin(vec![("/", ok("text/html; charset=utf-8", html))]).await;
        let page = loopback_fetcher().fetch(&req(&format!("{base}/"))).await.unwrap();
        assert_eq!(page.title.as_deref(), Some("My & Page"));
        assert_eq!(page.content, "Hello\n\nWorld <b>");
        assert!(!page.truncated);
    }

    #[tokio::test]
    async fn follows_redirects_but_revalidates_every_hop() {
        let base = origin(vec![
            ("/a", "HTTP/1.1 302 F\r\nlocation: /b\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into()),
            ("/b", ok("text/plain", "done")),
            ("/evil", "HTTP/1.1 302 F\r\nlocation: http://169.254.169.254/latest/meta-data/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into()),
        ])
        .await;
        let f = loopback_fetcher();
        assert_eq!(f.fetch(&req(&format!("{base}/a"))).await.unwrap().content, "done");
        let err = f.fetch(&req(&format!("{base}/evil"))).await.unwrap_err();
        assert!(matches!(err, WebError::Blocked(_)), "{err:?}");
    }

    #[tokio::test]
    async fn rejects_binary_content_and_oversized_pages_and_truncates() {
        let big = "a".repeat(MAX_BODY_BYTES + 1);
        let base = origin(vec![
            ("/img", ok("image/png", "x")),
            ("/big", ok("text/plain", &big)),
            ("/t", ok("text/plain", "0123456789abc")),
        ])
        .await;
        let f = loopback_fetcher();
        assert!(matches!(f.fetch(&req(&format!("{base}/img"))).await.unwrap_err(), WebError::InvalidRequest(_)));
        assert!(matches!(f.fetch(&req(&format!("{base}/big"))).await.unwrap_err(), WebError::InvalidRequest(_)));
        let page = f.fetch(&FetchRequest { url: format!("{base}/t"), max_chars: 10 }).await.unwrap();
        assert_eq!((page.content.as_str(), page.truncated), ("0123456789", true));
    }

    #[tokio::test]
    async fn operator_deny_and_allow_lists_apply_to_the_host_and_every_redirect_hop() {
        let base = origin(vec![
            ("/a", "HTTP/1.1 302 F\r\nlocation: http://blocked.test/x\r\ncontent-length: 0\r\nconnection: close\r\n\r\n".into()),
            ("/ok", ok("text/plain", "fine")),
        ])
        .await;
        let mut f = loopback_fetcher();
        f.blocked_domains = vec!["blocked.test".into(), "evil.example".into()];
        // Denied directly, as a subdomain, and with case/trailing-dot variations.
        for url in ["http://blocked.test/", "http://www.Blocked.Test./x", "https://a.b.evil.example/"] {
            assert!(matches!(f.fetch(&req(url)).await.unwrap_err(), WebError::Blocked(m) if m.contains("blocked by the operator")), "{url}");
        }
        // Not a subdomain match: a different registrable name is fine for the deny list.
        assert_eq!(f.fetch(&req(&format!("{base}/ok"))).await.unwrap().content, "fine");
        // A redirect to a denied host is stopped at that hop.
        let host = base.trim_start_matches("http://").split(':').next().unwrap().to_string();
        f.blocked_domains = vec!["blocked.test".into()];
        let err = f.fetch(&req(&format!("{base}/a"))).await.unwrap_err();
        assert!(matches!(err, WebError::Blocked(m) if m.contains("blocked by the operator")), "redirect hop");
        // Allow list: only listed domains (and subdomains) pass.
        f.blocked_domains.clear();
        f.allowed_domains = vec!["docs.rs".into()];
        assert!(matches!(f.fetch(&req(&format!("{base}/ok"))).await.unwrap_err(), WebError::Blocked(m) if m.contains("allow list")));
        assert!(matches!(f.fetch(&req("http://evildocs.rs/")).await.unwrap_err(), WebError::Blocked(_)), "suffix must be a label boundary");
        f.allowed_domains = vec![host];
        assert_eq!(f.fetch(&req(&format!("{base}/ok"))).await.unwrap().content, "fine");
    }

    /// DNS rebinding: a name that resolves to an allowed address when checked and to
    /// something else on any later lookup. The connection is pinned to the validated
    /// answers, so the second answer can never be used: the fetch connects to the first
    /// answer (the local test origin) even though the host name does not exist in DNS,
    /// and the resolver is asked exactly once per hop.
    struct RebindingResolver {
        first: SocketAddr,
        calls: std::sync::atomic::AtomicUsize,
    }

    #[async_trait]
    impl Resolver for RebindingResolver {
        async fn resolve(&self, _host: &str, port: u16) -> Result<Vec<SocketAddr>, McpError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(vec![if n == 0 { SocketAddr::new(self.first.ip(), port) } else { SocketAddr::new("10.0.0.1".parse().unwrap(), port) }])
        }
    }

    #[tokio::test]
    async fn dns_rebinding_cannot_redirect_the_pinned_connection() {
        let base = origin(vec![("/", ok("text/plain", "from the validated address"))]).await;
        let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
        let resolver = Arc::new(RebindingResolver { first: SocketAddr::new("127.0.0.1".parse().unwrap(), port), calls: Default::default() });
        let mut f = BuiltinFetcher::with_resolver(resolver.clone());
        f.allow_loopback = true;
        let page = f.fetch(&req(&format!("http://rebind.invalid:{port}/"))).await.unwrap();
        assert_eq!(page.content, "from the validated address");
        assert_eq!(resolver.calls.load(std::sync::atomic::Ordering::SeqCst), 1, "no second lookup at connect time");
    }

    #[tokio::test]
    async fn a_private_first_answer_is_refused_before_any_connection() {
        struct PrivateFirst;
        #[async_trait]
        impl Resolver for PrivateFirst {
            async fn resolve(&self, _h: &str, port: u16) -> Result<Vec<SocketAddr>, McpError> {
                Ok(vec![SocketAddr::new("10.0.0.1".parse().unwrap(), port)])
            }
        }
        let f = BuiltinFetcher::with_resolver(Arc::new(PrivateFirst));
        assert!(matches!(f.fetch(&req("http://rebind.invalid/")).await.unwrap_err(), WebError::Blocked(_)));
    }
}
