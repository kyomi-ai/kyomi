// SPDX-License-Identifier: AGPL-3.0-or-later
use async_trait::async_trait;
use kyomi_mcp_client::*;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
#[derive(Default)]
struct Fixture {
    requests: Mutex<Vec<HttpRequest>>,
    responses: Mutex<VecDeque<Result<HttpResponse, Error>>>,
}
#[async_trait]
impl HttpTransport for Fixture {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, Error> {
        self.requests.lock().unwrap().push(request);
        self.responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected additional HTTP dispatch")
    }
}
fn response(id: u64, result: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        content_type: "application/json".into(),
        session_id: None,
        body: serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap(),
        www_authenticate: None,
        retry_after: None,
    }
}
fn fixture(
    responses: Vec<Result<HttpResponse, Error>>,
    revision: Revision,
    limits: Limits,
) -> (McpClient, Arc<Fixture>) {
    let transport = Arc::new(Fixture {
        responses: Mutex::new(responses.into()),
        ..Fixture::default()
    });
    (
        McpClient::new(
            transport.clone(),
            "https://mcp.example.com/mcp".into(),
            None,
            revision,
            limits,
        )
        .unwrap(),
        transport,
    )
}
fn tool() -> Value {
    json!({"name":"write_issue","inputSchema":{"type":"object","properties":{"title":{"type":"string","x-mcp-header":"Title"}}}})
}
#[tokio::test]
async fn modern_is_stateless_and_mirrors_exact_metadata_and_encoded_parameters() {
    let (client, fixture) = fixture(
        vec![
            Ok(response(1, json!({"tools":[tool()]}))),
            Ok(response(2, json!({"ok":true}))),
        ],
        Revision::Modern20260728,
        Limits::default(),
    );
    assert_eq!(client.list_tools().await.unwrap().len(), 1);
    assert_eq!(
        client
            .call_tool("write_issue", json!({"title":" padded "}))
            .await
            .unwrap(),
        RpcOutcome::Result(json!({"ok":true}))
    );
    client.close().await.unwrap();
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    for request in requests.iter() {
        let value: Value = serde_json::from_slice(&request.body).unwrap();
        assert_ne!(value["method"], "initialize");
        assert_eq!(
            value["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"],
            "2026-07-28"
        );
        assert_eq!(
            value["params"]["_meta"]["io.modelcontextprotocol/clientCapabilities"],
            json!({})
        );
        assert!(
            !request
                .mcp_headers
                .iter()
                .any(|(k, _)| k == "mcp-session-id")
        );
    }
    assert!(
        requests[1]
            .mcp_headers
            .contains(&("mcp-param-Title".into(), "=?base64?IHBhZGRlZCA=?=".into()))
    );
}
#[tokio::test]
async fn legacy_initializes_then_sends_session_and_delete() {
    let mut init = response(
        1,
        json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}}}),
    );
    init.session_id = Some("fixture-session".into());
    let accepted = HttpResponse {
        status: 202,
        body: Vec::new(),
        ..response(0, json!({}))
    };
    let (client, fixture) = fixture(
        vec![
            Ok(init),
            Ok(accepted.clone()),
            Ok(response(2, json!({"tools":[tool()]}))),
            Ok(accepted),
        ],
        Revision::Legacy20250326,
        Limits::default(),
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().dispatch,
        DispatchCertainty::NotSent
    );
    client.initialize().await.unwrap();
    client.list_tools().await.unwrap();
    client.close().await.unwrap();
    assert!(client.list_tools().await.is_err());
    let requests = fixture.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    let initial: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(initial["method"], "initialize");
    assert_eq!(initial["params"]["capabilities"], json!({}));
    assert!(initial["params"].get("_meta").is_none());
    for request in &requests[1..] {
        assert!(
            request
                .mcp_headers
                .contains(&("mcp-session-id".into(), "fixture-session".into()))
        );
    }
    assert_eq!(requests[3].method, reqwest::Method::DELETE);
}
#[tokio::test]
async fn unsupported_legacy_revision_never_sends_initialized() {
    let (client, f) = fixture(
        vec![Ok(response(1, json!({"protocolVersion":"1900-01-01"})))],
        Revision::Legacy20250326,
        Limits::default(),
    );
    assert_eq!(
        client.initialize().await.err().unwrap().kind,
        ErrorKind::UnsupportedRevision
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    assert!(Revision::parse("2025-11-25").is_err());
}
#[tokio::test]
async fn sse_notifications_and_typed_errors_are_preserved() {
    let mut r = response(1, json!({}));
    r.content_type = "text/event-stream; charset=utf-8".into();
    r.body=b": keepalive\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"error\":{\"code\":-32022,\"message\":\"unsupported\",\"data\":{\"supported\":[\"2025-03-26\"]}}}\r\n\r\n".to_vec();
    let (client, _) = fixture(vec![Ok(r)], Revision::Modern20260728, Limits::default());
    assert!(matches!(
        client.discovery().await.unwrap(),
        RpcOutcome::RemoteError { code: -32022, .. }
    ));
}
#[tokio::test]
async fn unsolicited_requests_and_mrtr_are_denied() {
    for method in [
        "sampling/createMessage",
        "elicitation/create",
        "roots/list",
        "tasks/get",
    ] {
        let mut r = response(1, json!({}));
        r.body = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":2,"method":method})).unwrap();
        let (client, f) = fixture(vec![Ok(r)], Revision::Modern20260728, Limits::default());
        assert_eq!(
            client.discovery().await.err().unwrap().kind,
            ErrorKind::UnsupportedCapability
        );
        assert_eq!(f.requests.lock().unwrap().len(), 1);
    }
    let (client, _) = fixture(
        vec![Ok(response(1, json!({"inputRequests":[]})))],
        Revision::Modern20260728,
        Limits::default(),
    );
    assert_eq!(
        client.discovery().await.err().unwrap().kind,
        ErrorKind::UnsupportedCapability
    );
}
#[tokio::test]
async fn mismatched_ids_and_duplicate_responses_are_rejected() {
    let (client, _) = fixture(
        vec![Ok(response(99, json!({})))],
        Revision::Modern20260728,
        Limits::default(),
    );
    assert_eq!(
        client.discovery().await.err().unwrap().kind,
        ErrorKind::Protocol
    );
    let mut r = response(1, json!({}));
    r.content_type = "text/event-stream".into();
    r.body=b"data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n".to_vec();
    let (client, _) = fixture(vec![Ok(r)], Revision::Modern20260728, Limits::default());
    assert_eq!(
        client.discovery().await.err().unwrap().kind,
        ErrorKind::Protocol
    );
}
#[tokio::test]
async fn discovery_retries_bounded_outages_but_mutations_are_never_replayed() {
    let outage = HttpResponse {
        status: 503,
        ..response(1, json!({}))
    };
    let limits = Limits {
        backoff: Duration::ZERO,
        ..Limits::default()
    };
    let (client, f) = fixture(
        vec![
            Ok(outage.clone()),
            Ok(response(2, json!({"tools":[tool()]}))),
            Err(Error::new(
                ErrorKind::Transport,
                DispatchCertainty::MayHaveBeenSent,
            )),
        ],
        Revision::Modern20260728,
        limits.clone(),
    );
    client.list_tools().await.unwrap();
    let error = client
        .call_tool("write_issue", json!({"title":"test"}))
        .await
        .err()
        .unwrap();
    assert_eq!(error.dispatch, DispatchCertainty::MayHaveBeenSent);
    assert_eq!(f.requests.lock().unwrap().len(), 3);
    let (client, f) = fixture(
        vec![Ok(outage.clone()), Ok(outage.clone()), Ok(outage)],
        Revision::Modern20260728,
        limits,
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().kind,
        ErrorKind::Http(503)
    );
    assert_eq!(f.requests.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn pagination_is_bounded_and_rejects_repeated_cursors() {
    let (client, f) = fixture(
        vec![
            Ok(response(1, json!({"tools":[],"nextCursor":"same"}))),
            Ok(response(2, json!({"tools":[],"nextCursor":"same"}))),
        ],
        Revision::Modern20260728,
        Limits::default(),
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().kind,
        ErrorKind::Protocol
    );
    assert_eq!(f.requests.lock().unwrap().len(), 2);
    let limits = Limits {
        pages: 1,
        ..Limits::default()
    };
    let (client, f) = fixture(
        vec![Ok(response(1, json!({"tools":[],"nextCursor":"next"})))],
        Revision::Modern20260728,
        limits,
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().kind,
        ErrorKind::Limit
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn schema_catalog_tool_count_and_body_limits_are_enforced() {
    for limits in [
        Limits {
            schema_bytes: 1,
            ..Limits::default()
        },
        Limits {
            catalog_bytes: 1,
            ..Limits::default()
        },
        Limits {
            body_bytes: 1,
            ..Limits::default()
        },
    ] {
        let (client, _) = fixture(
            vec![Ok(response(1, json!({"tools":[tool()]})))],
            Revision::Modern20260728,
            limits,
        );
        assert_eq!(
            client.list_tools().await.err().unwrap().kind,
            ErrorKind::Limit
        );
    }
    let (client, _) = fixture(
        vec![Ok(response(1, json!({"tools":[tool(),tool()]})))],
        Revision::Modern20260728,
        Limits {
            tools: 1,
            ..Limits::default()
        },
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().kind,
        ErrorKind::Limit
    );
}
#[tokio::test]
async fn invalid_schema_header_tools_have_diagnostics_and_preserve_valid_siblings() {
    for schema in [
        json!({"type":"object","items":{"type":"string","x-mcp-header":"Bad"}}),
        json!({"type":"object","properties":{"a":{"type":"number","x-mcp-header":"A"}}}),
        json!({"type":"object","properties":{"a":{"type":"string","x-mcp-header":"A"},"b":{"type":"string","x-mcp-header":"a"}}}),
    ] {
        let name = "bad / untrusted original-name";
        let (client, wire) = fixture(
            vec![
                Ok(response(
                    1,
                    json!({"tools":[{"name":name,"inputSchema":schema},tool()]}),
                )),
                Ok(response(2, json!({"ok":true}))),
            ],
            Revision::Modern20260728,
            Limits::default(),
        );
        let catalog = client.list_tools_catalog().await.unwrap();
        assert_eq!(catalog.tools, vec![tool()]);
        assert_eq!(
            catalog.rejected,
            vec![RejectedTool {
                name: name.into(),
                reason: ToolRejectionReason::InvalidHeaderAnnotations
            }]
        );
        assert!(catalog.rejected[0].reason.as_str().contains("x-mcp-header"));
        assert_eq!(
            client
                .call_tool(name, json!({}))
                .await
                .err()
                .unwrap()
                .dispatch,
            DispatchCertainty::NotSent
        );
        assert_eq!(
            client
                .call_tool("write_issue", json!({"title":"allowed"}))
                .await
                .unwrap(),
            RpcOutcome::Result(json!({"ok":true}))
        );
        assert_eq!(
            wire.requests.lock().unwrap().len(),
            2,
            "rejected tool never dispatched; valid sibling remains callable"
        );
    }
}

#[tokio::test]
async fn rejected_tools_count_toward_catalog_limits_and_duplicate_identity_checks() {
    let rejected = json!({"name":"bad","inputSchema":{"type":"object","properties":{"a":{"type":"number","x-mcp-header":"A"}}}});
    let (client, _) = fixture(
        vec![Ok(response(1, json!({"tools":[rejected.clone(),tool()]})))],
        Revision::Modern20260728,
        Limits {
            tools: 1,
            ..Limits::default()
        },
    );
    assert_eq!(
        client.list_tools_catalog().await.err().unwrap().kind,
        ErrorKind::Limit
    );
    let (client, _) = fixture(
        vec![Ok(response(
            1,
            json!({"tools":[rejected.clone(),rejected]}),
        ))],
        Revision::Modern20260728,
        Limits::default(),
    );
    assert_eq!(
        client.list_tools_catalog().await.err().unwrap().kind,
        ErrorKind::Protocol
    );
}

#[tokio::test]
async fn output_schema_has_the_same_bound_as_input_schema() {
    let mut remote_tool = tool();
    remote_tool["outputSchema"] = json!({"type":"object","description":"x".repeat(512)});
    let (client, _) = fixture(
        vec![Ok(response(1, json!({"tools":[remote_tool]})))],
        Revision::Modern20260728,
        Limits {
            schema_bytes: 256,
            ..Limits::default()
        },
    );
    assert_eq!(
        client.list_tools().await.err().unwrap().kind,
        ErrorKind::Limit
    );
}
