// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::{DispatchCertainty as D, Error, ErrorKind as K, Limits};
use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::{
    Method,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};
use tokio::sync::Semaphore;
use url::Url;

/// Construct only from trusted deployment configuration; never deserialize user input into it.
#[derive(Clone, Default)]
pub struct DeploymentPolicy {
    private_authorities: BTreeSet<String>,
}
impl DeploymentPolicy {
    /// Exact HTTPS origins (host AND port), not suffixes. TLS validation remains mandatory.
    pub fn allow_private_https_origins(origins: &[String]) -> Result<Self, Error> {
        let mut policy = Self::default();
        for origin in origins {
            let url = checked_url(origin)?;
            if url.path() != "/" || url.query().is_some() {
                return Err(Error::new(K::Configuration, D::NotSent));
            }
            policy
                .private_authorities
                .insert(url.origin().ascii_serialization());
        }
        Ok(policy)
    }
}
#[async_trait]
pub trait Resolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, Error>;
}
pub struct SystemResolver;
#[async_trait]
impl Resolver for SystemResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, Error> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|a| a.collect())
            .map_err(|_| Error::new(K::Dns, D::NotSent))
    }
}
/// Only explicit outbound credentials can be attached; there is no incoming HeaderMap to forward.
/// Authorization's audience is the request URL. Redirects are always refused.
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub authorization: Option<String>,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
    pub mcp_headers: Vec<(String, String)>,
}
#[derive(Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub session_id: Option<String>,
    pub body: Vec<u8>,
    pub www_authenticate: Option<String>,
    pub retry_after: Option<String>,
}
#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error>;
}
/// Every request resolves anew, validates ALL answers, and pins the socket connection to those
/// exact answers. Proxies and redirects are disabled. No alternate DNS lookup occurs at connect.
pub struct SafeHttpClient {
    limits: Limits,
    policy: DeploymentPolicy,
    resolver: Arc<dyn Resolver>,
    slots: Semaphore,
    #[cfg(test)]
    test_http: bool,
}
impl SafeHttpClient {
    pub fn new(limits: Limits, policy: DeploymentPolicy) -> Result<Self, Error> {
        Self::with_resolver(limits, policy, Arc::new(SystemResolver))
    }
    pub fn with_resolver(
        limits: Limits,
        policy: DeploymentPolicy,
        resolver: Arc<dyn Resolver>,
    ) -> Result<Self, Error> {
        limits.validate()?;
        Ok(Self {
            slots: Semaphore::new(limits.concurrency),
            limits,
            policy,
            resolver,
            #[cfg(test)]
            test_http: false,
        })
    }
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        #[cfg(test)]
        let url = if self.test_http {
            Url::parse(&request.url).map_err(|_| Error::new(K::UnsafeUrl, D::NotSent))?
        } else {
            checked_url(&request.url)?
        };
        #[cfg(not(test))]
        let url = checked_url(&request.url)?;
        if request.body.len() > self.limits.request_bytes {
            return Err(Error::new(K::Limit, D::NotSent));
        }
        let host = url
            .host_str()
            .ok_or_else(|| Error::new(K::UnsafeUrl, D::NotSent))?
            .trim_matches(['[', ']']);
        let port = url
            .port_or_known_default()
            .ok_or_else(|| Error::new(K::UnsafeUrl, D::NotSent))?;
        let addresses = if let Ok(ip) = host.parse::<IpAddr>() {
            vec![SocketAddr::new(ip, port)]
        } else {
            self.resolver.resolve(host, port).await?
        };
        #[cfg(test)]
        let test_allowed = self.test_http;
        #[cfg(not(test))]
        let test_allowed = false;
        let private_allowed = self
            .policy
            .private_authorities
            .contains(&url.origin().ascii_serialization());
        if addresses.is_empty()
            || addresses.len() > 32
            || addresses.iter().any(|a| {
                a.port() != port
                    || (!private_allowed
                        && !is_public_address(a.ip())
                        && !(test_allowed && a.ip().is_loopback()))
            })
        {
            return Err(Error::new(K::UnsafeUrl, D::NotSent));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(self.limits.connect_timeout)
            .timeout(self.limits.deadline)
            .retry(reqwest::retry::never())
            .pool_max_idle_per_host(0)
            .resolve_to_addrs(host, &addresses)
            .build()
            .map_err(|_| Error::new(K::Configuration, D::NotSent))?;
        let mut headers = HeaderMap::new();
        headers.insert(
            "accept",
            HeaderValue::from_static("application/json, text/event-stream"),
        );
        if let Some(auth) = request.authorization {
            let mut value = HeaderValue::from_str(&auth)
                .map_err(|_| Error::new(K::Configuration, D::NotSent))?;
            value.set_sensitive(true);
            headers.insert("authorization", value);
        }
        if let Some(ct) = request.content_type {
            headers.insert(
                "content-type",
                HeaderValue::from_str(&ct).map_err(|_| Error::new(K::Configuration, D::NotSent))?,
            );
        }
        for (name, value) in request.mcp_headers {
            // Never allow this protocol-only channel to smuggle cookies, Host or authorization.
            if !name.to_ascii_lowercase().starts_with("mcp-") {
                return Err(Error::new(K::Configuration, D::NotSent));
            }
            let mut value = HeaderValue::from_str(&value)
                .map_err(|_| Error::new(K::Configuration, D::NotSent))?;
            value.set_sensitive(true);
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .map_err(|_| Error::new(K::Configuration, D::NotSent))?,
                value,
            );
        }
        let expected_id = serde_json::from_slice::<serde_json::Value>(&request.body)
            .ok()
            .and_then(|v| v.get("id").cloned());
        let response = client
            .request(request.method, url)
            .headers(headers)
            .body(request.body)
            .send()
            .await
            .map_err(|e| {
                Error::new(
                    if e.is_timeout() {
                        K::Deadline
                    } else {
                        K::Transport
                    },
                    D::MayHaveBeenSent,
                )
            })?;
        // Defense in depth: reject any connector that reports an unexpected actual peer.
        if !response
            .remote_addr()
            .is_some_and(|peer| addresses.contains(&peer))
        {
            return Err(Error::new(K::UnsafeUrl, D::MayHaveBeenSent));
        }
        if response.status().is_redirection() {
            return Err(Error::new(K::Redirect, D::ResponseReceived));
        }
        if response
            .content_length()
            .is_some_and(|n| n > self.limits.body_bytes as u64)
        {
            return Err(Error::new(K::Limit, D::ResponseReceived));
        }
        let status = response.status().as_u16();
        let www_authenticate = response
            .headers()
            .get("www-authenticate")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 8192)
            .map(str::to_owned);
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .filter(|v| v.len() <= 128)
            .map(str::to_owned);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let session_id = response
            .headers()
            .get("mcp-session-id")
            .map(|v| v.to_str().map(str::to_owned))
            .transpose()
            .map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?;
        if session_id.as_ref().is_some_and(|s| {
            s.is_empty() || s.len() > 256 || !s.bytes().all(|b| (0x21..=0x7e).contains(&b))
        }) {
            return Err(Error::new(K::Protocol, D::ResponseReceived));
        }
        let is_sse = content_type.split(';').next() == Some("text/event-stream");
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        let mut frame_bytes = 0;
        let mut previous_newline = false;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| Error::new(K::Transport, D::MayHaveBeenSent))?;
            if body.len().saturating_add(chunk.len()) > self.limits.body_bytes {
                return Err(Error::new(K::Limit, D::ResponseReceived));
            }
            if is_sse {
                for &byte in &chunk {
                    frame_bytes += 1;
                    if frame_bytes > self.limits.sse_frame_bytes {
                        return Err(Error::new(K::Limit, D::ResponseReceived));
                    }
                    if byte == b'\n' {
                        if previous_newline {
                            frame_bytes = 0;
                        }
                        previous_newline = true;
                    } else if byte != b'\r' {
                        previous_newline = false;
                    }
                }
            }
            body.extend_from_slice(&chunk);
            if is_sse && let Some(id) = &expected_id {
                // Stop at the response even when the peer keeps the HTTP stream open.
                // Partial UTF-8 is legal between chunks; parse only complete frame prefixes.
                if let Some(end) = crate::sse::complete_prefix(&body) {
                    let messages = crate::sse::messages(&body[..end], &self.limits, false)?;
                    if messages.iter().any(|m| {
                        m.get("id") == Some(id)
                            || (m.get("id").is_some() && m.get("method").is_some())
                    }) {
                        body.truncate(end);
                        break;
                    }
                }
            }
        }
        Ok(HttpResponse {
            status,
            content_type,
            session_id,
            body,
            www_authenticate,
            retry_after,
        })
    }
}
#[async_trait]
impl HttpTransport for SafeHttpClient {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| Error::new(K::Busy, D::NotSent))?;
        // DNS, connection and the entire streamed body share one deadline.
        tokio::time::timeout(self.limits.deadline, self.send(request))
            .await
            .map_err(|_| Error::new(K::Deadline, D::MayHaveBeenSent))?
    }
}
fn checked_url(raw: &str) -> Result<Url, Error> {
    let url = Url::parse(raw).map_err(|_| Error::new(K::UnsafeUrl, D::NotSent))?;
    if raw.len() > 8192
        || url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
        || url.fragment().is_some()
    {
        return Err(Error::new(K::UnsafeUrl, D::NotSent));
    }
    Ok(url)
}
/// Conservative public-unicast predicate. IPv6 is restricted to global unicast,
/// excluding documentation and transition ranges; mapped IPv4 uses the IPv4 rules.
pub fn is_public_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a == 10
                || a == 127
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 169 && b == 254)
                || (a == 172 && (16..=31).contains(&b))
                || (a == 192 && (b == 168 || b == 0 || (b == 88 && c == 99) || (b == 0 && c == 2)))
                || (a == 198 && (b == 18 || b == 19 || (b == 51 && c == 100)))
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_address(IpAddr::V4(v4));
            }
            let s = ip.segments();
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && s[0] != 0x2002
                && s[0] != 0x3fff
        }
    }
}

#[cfg(test)]
#[path = "http_tests.rs"]
mod tests;
