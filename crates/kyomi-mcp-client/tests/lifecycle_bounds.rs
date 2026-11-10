// SPDX-License-Identifier: AGPL-3.0-or-later
use async_trait::async_trait;
use kyomi_mcp_client::*;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;
struct Controlled {
    stall: &'static str,
    modern: bool,
    requests: Mutex<Vec<Value>>,
    entered: Notify,
}
#[async_trait]
impl HttpTransport for Controlled {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let value: Value = if request.method == reqwest::Method::DELETE {
            json!({"method":"DELETE"})
        } else {
            serde_json::from_slice(&request.body).unwrap()
        };
        self.requests.lock().unwrap().push(value.clone());
        let method = value["method"].as_str().unwrap();
        if method == self.stall {
            self.entered.notify_one();
            std::future::pending::<()>().await;
        }
        let id = value.get("id").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => json!({"protocolVersion":"2025-03-26","capabilities":{}}),
            "tools/list" => {
                json!({"tools":[{"name":"write_issue","inputSchema":{"type":"object"}}]})
            }
            _ => json!({}),
        };
        Ok(HttpResponse {
            status: if id.is_null() { 202 } else { 200 },
            content_type: "application/json".into(),
            session_id: (!self.modern).then(|| "fixture-session".into()),
            body: if id.is_null() {
                Vec::new()
            } else {
                serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap()
            },
            www_authenticate: None,
            retry_after: None,
        })
    }
}
fn controlled(stall: &'static str, revision: Revision) -> (McpClient, Arc<Controlled>) {
    let transport = Arc::new(Controlled {
        stall,
        modern: revision == Revision::Modern20260728,
        requests: Mutex::new(Vec::new()),
        entered: Notify::new(),
    });
    (
        McpClient::new(
            transport.clone(),
            "https://mcp.example.com/mcp".into(),
            None,
            revision,
            Limits {
                deadline: Duration::from_millis(100),
                ..Limits::default()
            },
        )
        .unwrap(),
        transport,
    )
}
#[tokio::test(start_paused = true)]
async fn stalled_initialized_notification_and_waiting_legacy_lock_share_deadlines() {
    let (client, transport) = controlled("notifications/initialized", Revision::Legacy20250326);
    let (init, waiting) = tokio::join!(client.initialize(), async {
        transport.entered.notified().await;
        client.list_tools().await
    });
    assert_eq!(init.err().unwrap().kind, ErrorKind::Deadline);
    assert!(matches!(
        waiting.err().unwrap().kind,
        ErrorKind::Deadline | ErrorKind::Protocol
    ));
    let requests = transport.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["method"], "initialize");
    assert_eq!(requests[1]["method"], "notifications/initialized");
}
#[tokio::test(start_paused = true)]
async fn stalled_delete_is_bounded_and_locally_invalidates_session() {
    let (client, transport) = controlled("DELETE", Revision::Legacy20250326);
    client.initialize().await.unwrap();
    assert_eq!(
        client.close().await.err().unwrap().kind,
        ErrorKind::Deadline
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().dispatch,
        DispatchCertainty::NotSent
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 3);
}
#[tokio::test(start_paused = true)]
async fn cancellation_is_bound_to_the_live_call_and_has_distinct_era_semantics() {
    for revision in [Revision::Legacy20250326, Revision::Modern20260728] {
        let (client, transport) = controlled("tools/call", revision);
        if revision == Revision::Legacy20250326 {
            client.initialize().await.unwrap();
        }
        client.list_tools().await.unwrap();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (result, ()) = tokio::join!(
            client.call_tool_cancellable("write_issue", json!({}), receiver),
            async {
                transport.entered.notified().await;
                sender.send(()).unwrap();
            }
        );
        let error = result.err().unwrap();
        assert_eq!(error.kind, ErrorKind::Cancelled);
        assert_eq!(error.dispatch, DispatchCertainty::MayHaveBeenSent);
        let requests = transport.requests.lock().unwrap();
        let call = requests
            .iter()
            .find(|r| r["method"] == "tools/call")
            .unwrap();
        let cancel = requests
            .iter()
            .find(|r| r["method"] == "notifications/cancelled");
        if revision == Revision::Legacy20250326 {
            assert_eq!(cancel.unwrap()["params"]["requestId"], call["id"]);
            assert_ne!(call["id"], requests[0]["id"]);
        } else {
            assert!(cancel.is_none());
        }
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "tools/call")
                .count(),
            1
        );
    }
}
#[tokio::test(start_paused = true)]
async fn dropping_cancellation_sender_does_not_cancel_or_replay_a_write() {
    let (client, transport) = controlled("tools/call", Revision::Modern20260728);
    client.list_tools().await.unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    drop(sender);
    assert_eq!(
        client
            .call_tool_cancellable("write_issue", json!({}), receiver)
            .await
            .err()
            .unwrap()
            .kind,
        ErrorKind::Deadline
    );
    assert_eq!(transport.requests.lock().unwrap().len(), 2);
}
struct Paged {
    calls: Mutex<usize>,
    outage: bool,
}
#[async_trait]
impl HttpTransport for Paged {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        let count = {
            let mut count = self.calls.lock().unwrap();
            *count += 1;
            *count
        };
        tokio::time::sleep(Duration::from_millis(40)).await;
        Ok(HttpResponse{status:if self.outage{503}else{200},content_type:"application/json".into(),session_id:None,body:serde_json::to_vec(&json!({"jsonrpc":"2.0","id":value["id"],"result":{"tools":[],"nextCursor":count.to_string()}})).unwrap(),www_authenticate:None,retry_after:None})
    }
}
#[tokio::test(start_paused = true)]
async fn catalog_pages_and_retry_backoff_have_one_total_deadline() {
    for outage in [false, true] {
        let transport = Arc::new(Paged {
            calls: Mutex::new(0),
            outage,
        });
        let client = McpClient::new(
            transport.clone(),
            "https://mcp.example.com/mcp".into(),
            None,
            Revision::Modern20260728,
            Limits {
                deadline: Duration::from_millis(100),
                backoff: Duration::from_secs(1),
                ..Limits::default()
            },
        )
        .unwrap();
        assert_eq!(
            client.list_tools().await.err().unwrap().kind,
            ErrorKind::Deadline
        );
        assert_eq!(*transport.calls.lock().unwrap(), if outage { 1 } else { 3 });
    }
}

struct RejectCancellation {
    inner: Arc<Controlled>,
}
#[async_trait]
impl HttpTransport for RejectCancellation {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        if value["method"] == "notifications/cancelled" {
            self.inner.requests.lock().unwrap().push(value);
            return Err(Error::new(ErrorKind::UnsafeUrl, DispatchCertainty::NotSent));
        }
        self.inner.execute(request).await
    }
}
#[tokio::test(start_paused = true)]
async fn undispatched_cancel_notification_cannot_erase_original_write_uncertainty() {
    let (_, inner) = controlled("tools/call", Revision::Legacy20250326);
    let client = McpClient::new(
        Arc::new(RejectCancellation {
            inner: inner.clone(),
        }),
        "https://mcp.example.com/mcp".into(),
        None,
        Revision::Legacy20250326,
        Limits::default(),
    )
    .unwrap();
    client.initialize().await.unwrap();
    client.list_tools().await.unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let (result, ()) = tokio::join!(
        client.call_tool_cancellable("write_issue", json!({}), receiver),
        async {
            inner.entered.notified().await;
            sender.send(()).unwrap();
        }
    );
    let error = result.err().unwrap();
    assert_eq!(error.kind, ErrorKind::UnsafeUrl);
    assert_eq!(error.dispatch, DispatchCertainty::MayHaveBeenSent);
    let requests = inner.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "tools/call")
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|r| r["method"] == "notifications/cancelled")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
async fn presignalled_cancellation_never_issues_call_or_cancellation_post() {
    let (client, transport) = controlled("tools/call", Revision::Legacy20250326);
    client.initialize().await.unwrap();
    client.list_tools().await.unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    sender.send(()).unwrap();
    let error = client
        .call_tool_cancellable("write_issue", json!({}), receiver)
        .await
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Cancelled);
    assert_eq!(error.dispatch, DispatchCertainty::NotSent);
    assert!(
        !transport
            .requests
            .lock()
            .unwrap()
            .iter()
            .any(|r| r["method"] == "tools/call" || r["method"] == "notifications/cancelled")
    );
}
