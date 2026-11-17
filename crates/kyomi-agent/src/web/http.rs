// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared HTTP base for the Kyomi-executed backends: one place for the client,
//! timeout, status mapping and the single retry on transient failure.

use std::time::Duration;

use serde_json::Value;

use super::WebError;

const TIMEOUT: Duration = Duration::from_secs(20);

/// POST a JSON body with bearer auth and return the parsed JSON response.
/// Status codes map onto [`WebError`] identically for every vendor; `quota`
/// lists vendor-specific statuses that mean "quota exhausted".
pub(crate) async fn post_json(
    client: &reqwest::Client,
    url: &str,
    api_key: &str,
    body: &Value,
    quota_statuses: &[u16],
) -> Result<Value, WebError> {
    let mut last = WebError::Upstream("no attempt made".into());
    for attempt in 0..2 {
        let sent = client
            .post(url)
            .bearer_auth(api_key)
            .timeout(TIMEOUT)
            .json(body)
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) if e.is_timeout() => {
                last = WebError::Timeout;
                continue;
            }
            Err(e) => {
                last = WebError::Upstream(e.without_url().to_string());
                continue;
            }
        };
        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(Duration::from_secs);
        if (200..300).contains(&status) {
            return response
                .json::<Value>()
                .await
                .map_err(|e| WebError::Upstream(format!("unparsable response: {}", e.without_url())));
        }
        let text = response.text().await.unwrap_or_default();
        let err = map_status(status, retry_after, &text, quota_statuses);
        // Only transient failures are retried, once.
        if attempt == 0 && matches!(err, WebError::Upstream(_)) && status >= 500 {
            last = err;
            continue;
        }
        return Err(err);
    }
    Err(last)
}

pub(crate) fn map_status(status: u16, retry_after: Option<Duration>, body: &str, quota: &[u16]) -> WebError {
    if quota.contains(&status) {
        return WebError::QuotaExhausted;
    }
    match status {
        401 | 403 => WebError::Unauthorized,
        429 => WebError::RateLimited { retry_after },
        400 | 422 => WebError::InvalidRequest(truncate(body)),
        _ => WebError::Upstream(format!("HTTP {status}: {}", truncate(body))),
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(300).collect()
}

/// Merge opaque backend options into a request body (top-level keys). Options
/// may tune anything the vendor accepts (depth, engine, ...) except the
/// `protected` keys Kyomi sets itself (the query/URL and the result cap), so
/// config cannot widen what the model asked for or lift Kyomi's limits.
pub(crate) fn merge_options(body: &mut Value, options: &serde_json::Map<String, Value>, protected: &[&str]) {
    if let Some(map) = body.as_object_mut() {
        for (k, v) in options {
            if !protected.contains(&k.as_str()) {
                map.insert(k.clone(), v.clone());
            }
        }
    }
}

/// Test helper: a one-shot local HTTP server returning a canned response and
/// reporting the request it saw.
#[cfg(test)]
pub(crate) mod test_server {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    pub struct Captured {
        pub head: String,
        pub body: String,
    }

    /// Serve `responses` (status, body) in order, one per connection.
    pub async fn serve(responses: Vec<(u16, &'static str)>) -> (String, tokio::sync::mpsc::UnboundedReceiver<Captured>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for (status, body) in responses {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let (head_end, content_len) = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break (buf.len(), 0);
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..pos]).to_lowercase();
                        let len = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        break (pos + 4, len);
                    }
                };
                while buf.len() < head_end + content_len {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let head = String::from_utf8_lossy(&buf[..head_end.saturating_sub(4)]).to_string();
                let body_in = String::from_utf8_lossy(&buf[head_end.min(buf.len())..]).to_string();
                let _ = tx.send(Captured { head, body: body_in });
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (format!("http://{addr}"), rx)
    }
}
