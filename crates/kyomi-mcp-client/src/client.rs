// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::{
    DispatchCertainty as D, Error, ErrorKind as K, HttpRequest, HttpResponse, HttpTransport,
    Limits, schema, sse,
};
use reqwest::Method;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::sync::{Mutex, Semaphore, oneshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revision {
    Modern20260728,
    Legacy20250326,
}
impl Revision {
    pub fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "2026-07-28" => Ok(Self::Modern20260728),
            "2025-03-26" => Ok(Self::Legacy20250326),
            _ => Err(Error::new(K::UnsupportedRevision, D::NotSent)),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Modern20260728 => "2026-07-28",
            Self::Legacy20250326 => "2025-03-26",
        }
    }
}
/// Remote JSON-RPC errors and tool isError results are completed typed outcomes, never transport errors.
#[derive(Debug, PartialEq)]
pub enum RpcOutcome {
    Result(Value),
    RemoteError {
        code: i64,
        message: String,
        data: Option<Value>,
    },
}
impl RpcOutcome {
    pub fn into_result(self) -> Result<Value, Self> {
        match self {
            Self::Result(v) => Ok(v),
            e => Err(e),
        }
    }
}
/// Bounded diagnostics contain the exact untrusted remote name and a static reason,
/// never credentials, headers, URLs or arbitrary server error text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RejectedTool {
    pub name: String,
    pub reason: ToolRejectionReason,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolRejectionReason {
    InvalidHeaderAnnotations,
}
impl ToolRejectionReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidHeaderAnnotations => {
                "invalid x-mcp-header annotations; fix the server tool definition"
            }
        }
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCatalog {
    pub tools: Vec<Value>,
    pub rejected: Vec<RejectedTool>,
}

#[derive(Default)]
struct LegacyState {
    initialized: bool,
    session: Option<String>,
}
pub struct McpClient {
    transport: Arc<dyn HttpTransport>,
    endpoint: String,
    authorization: Option<String>,
    revision: Revision,
    limits: Limits,
    next_id: AtomicU64,
    legacy: Mutex<LegacyState>,
    slots: Semaphore,
    schemas: Mutex<BTreeMap<String, Value>>,
}
impl McpClient {
    /// Endpoint and credentials are connection scoped. No environment/global credential lookup.
    /// Revision is explicit: automatic fallback must never redispatch an ambiguous tool mutation.
    pub fn new(
        transport: Arc<dyn HttpTransport>,
        endpoint: String,
        authorization: Option<String>,
        revision: Revision,
        limits: Limits,
    ) -> Result<Self, Error> {
        limits.validate()?;
        Ok(Self {
            transport,
            endpoint,
            authorization,
            revision,
            slots: Semaphore::new(limits.concurrency),
            limits,
            next_id: AtomicU64::new(1),
            legacy: Mutex::new(LegacyState::default()),
            schemas: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn revision(&self) -> Revision {
        self.revision
    }
    async fn bounded<T>(
        &self,
        future: impl std::future::Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        tokio::time::timeout(self.limits.deadline, future)
            .await
            .map_err(|_| Error::new(K::Deadline, D::MayHaveBeenSent))?
    }
    pub async fn initialize(&self) -> Result<RpcOutcome, Error> {
        self.bounded(self.initialize_inner()).await
    }
    async fn initialize_inner(&self) -> Result<RpcOutcome, Error> {
        if self.revision == Revision::Modern20260728 {
            return self.discovery().await;
        }
        let mut state = self.legacy.lock().await;
        if state.initialized {
            return Err(Error::new(K::Protocol, D::NotSent));
        }
        let (outcome, session) = self.exchange_cancellable("initialize", json!({"protocolVersion":self.revision.as_str(), "capabilities":{}, "clientInfo":{"name":"kyomi-mcp-client","version":env!("CARGO_PKG_VERSION")}}), None, Vec::new(), None, Some(&mut state.session)).await?;
        if let RpcOutcome::Result(value) = &outcome {
            if value.get("protocolVersion").and_then(Value::as_str) != Some(self.revision.as_str())
            {
                return Err(Error::new(K::UnsupportedRevision, D::ResponseReceived));
            }
            state.session = session.clone();
            self.notify("notifications/initialized", json!({}), session)
                .await?;
            state.initialized = true;
        }
        Ok(outcome)
    }
    pub async fn discovery(&self) -> Result<RpcOutcome, Error> {
        self.bounded(self.discovery_inner()).await
    }
    async fn discovery_inner(&self) -> Result<RpcOutcome, Error> {
        if self.revision != Revision::Modern20260728 {
            return Err(Error::new(K::UnsupportedRevision, D::NotSent));
        }
        self.exchange("server/discover", json!({}), None, Vec::new())
            .await
            .map(|v| v.0)
    }
    async fn session(&self) -> Result<Option<String>, Error> {
        if self.revision == Revision::Modern20260728 {
            return Ok(None);
        }
        let state = self.legacy.lock().await;
        if !state.initialized {
            return Err(Error::new(K::Protocol, D::NotSent));
        }
        Ok(state.session.clone())
    }
    /// Only bounded discovery reads receive retries. Tools/call never retries, regardless of annotations.
    pub async fn list_tools(&self) -> Result<Vec<Value>, Error> {
        self.list_tools_catalog().await.map(|catalog| catalog.tools)
    }
    /// Use this catalog port when exclusions must be visible to a user. Invalid
    /// modern header definitions are excluded without losing valid siblings.
    pub async fn list_tools_catalog(&self) -> Result<ToolCatalog, Error> {
        self.bounded(self.catalog("tools/list", "tools")).await
    }
    pub async fn list_resources(&self) -> Result<Vec<Value>, Error> {
        self.bounded(self.catalog("resources/list", "resources"))
            .await
            .map(|catalog| catalog.tools)
    }
    async fn catalog(&self, method: &str, key: &str) -> Result<ToolCatalog, Error> {
        let session = self.session().await?;
        let mut cursor = None;
        let mut cursors = BTreeSet::new();
        let mut all = Vec::new();
        let mut bytes = 0usize;
        let mut schemas = BTreeMap::new();
        let mut names = BTreeSet::new();
        let mut rejected = Vec::new();
        let mut count = 0usize;
        for _ in 0..self.limits.pages {
            let params = cursor
                .as_ref()
                .map(|c| json!({"cursor":c}))
                .unwrap_or_else(|| json!({}));
            let mut attempt = 0;
            let result = loop {
                match self
                    .exchange(method, params.clone(), session.clone(), Vec::new())
                    .await
                {
                    Err(error)
                        if attempt < self.limits.read_retries
                            && matches!(
                                error.kind,
                                K::Http(429 | 502 | 503 | 504) | K::Transport | K::Deadline
                            ) =>
                    {
                        let delay = self
                            .limits
                            .backoff
                            .saturating_mul(1u32 << attempt)
                            .min(self.limits.max_backoff);
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                    }
                    result => break result?,
                }
            }
            .0;
            let RpcOutcome::Result(page) = result else {
                return Err(Error::new(K::Protocol, D::ResponseReceived));
            };
            bytes = bytes.saturating_add(
                serde_json::to_vec(&page)
                    .map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?
                    .len(),
            );
            if bytes > self.limits.catalog_bytes {
                return Err(Error::new(K::Limit, D::ResponseReceived));
            }
            let values = page
                .get(key)
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))?;
            for value in values {
                count = count.saturating_add(1);
                if count > self.limits.tools {
                    return Err(Error::new(K::Limit, D::ResponseReceived));
                }
                if key == "tools" {
                    let name = value
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|s| {
                            !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control)
                        })
                        .ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))?;
                    if !names.insert(name.to_owned()) {
                        return Err(Error::new(K::Protocol, D::ResponseReceived));
                    }
                    let input = value
                        .get("inputSchema")
                        .filter(|v| v.is_object())
                        .ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))?;
                    if serde_json::to_vec(input)
                        .map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?
                        .len()
                        > self.limits.schema_bytes
                    {
                        return Err(Error::new(K::Limit, D::ResponseReceived));
                    }
                    if let Some(output) = value.get("outputSchema") {
                        if !output.is_object() {
                            return Err(Error::new(K::Protocol, D::ResponseReceived));
                        }
                        if serde_json::to_vec(output)
                            .map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?
                            .len()
                            > self.limits.schema_bytes
                        {
                            return Err(Error::new(K::Limit, D::ResponseReceived));
                        }
                    }
                    if self.revision == Revision::Modern20260728
                        && schema::annotations(input).is_err()
                    {
                        rejected.push(RejectedTool {
                            name: name.to_owned(),
                            reason: ToolRejectionReason::InvalidHeaderAnnotations,
                        });
                        continue;
                    }
                    if schemas.insert(name.to_owned(), input.clone()).is_some() {
                        return Err(Error::new(K::Protocol, D::ResponseReceived));
                    }
                }
                all.push(value.clone());
            }
            match page.get("nextCursor") {
                None | Some(Value::Null) => {
                    if key == "tools" {
                        *self.schemas.lock().await = schemas;
                    }
                    return Ok(ToolCatalog {
                        tools: all,
                        rejected,
                    });
                }
                Some(Value::String(next))
                    if !next.is_empty()
                        && next.len() <= self.limits.request_bytes
                        && cursors.insert(next.clone()) =>
                {
                    cursor = Some(next.clone())
                }
                _ => return Err(Error::new(K::Protocol, D::ResponseReceived)),
            }
        }
        Err(Error::new(K::Limit, D::ResponseReceived))
    }
    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<RpcOutcome, Error> {
        self.bounded(self.call_tool_inner(name, arguments, None))
            .await
    }
    /// Cancellation is tied to this invocation, never an arbitrary/initialize/completed id.
    /// Sending () cancels; dropping the sender does not. A cancelled write remains unknown.
    pub async fn call_tool_cancellable(
        &self,
        name: &str,
        arguments: Value,
        cancellation: oneshot::Receiver<()>,
    ) -> Result<RpcOutcome, Error> {
        self.bounded(self.call_tool_inner(name, arguments, Some(cancellation)))
            .await
    }
    async fn call_tool_inner(
        &self,
        name: &str,
        arguments: Value,
        cancellation: Option<oneshot::Receiver<()>>,
    ) -> Result<RpcOutcome, Error> {
        let session = self.session().await?;
        let schemas = self.schemas.lock().await;
        let input = schemas
            .get(name)
            .ok_or_else(|| Error::new(K::Protocol, D::NotSent))?;
        let headers = if self.revision == Revision::Modern20260728 {
            schema::parameter_headers(input, &arguments)?
        } else {
            Vec::new()
        };
        drop(schemas);
        self.exchange_cancellable(
            "tools/call",
            json!({"name":name,"arguments":arguments}),
            session,
            headers,
            cancellation,
            None,
        )
        .await
        .map(|v| v.0)
    }
    pub async fn read_resource(&self, uri: &str) -> Result<RpcOutcome, Error> {
        self.bounded(self.read_resource_inner(uri)).await
    }
    async fn read_resource_inner(&self, uri: &str) -> Result<RpcOutcome, Error> {
        let session = self.session().await?;
        self.exchange("resources/read", json!({"uri":uri}), session, Vec::new())
            .await
            .map(|v| v.0)
    }
    pub async fn close(&self) -> Result<(), Error> {
        self.bounded(self.close_inner()).await
    }
    async fn close_inner(&self) -> Result<(), Error> {
        if self.revision == Revision::Modern20260728 {
            return Ok(());
        }
        let mut state = self.legacy.lock().await;
        // Invalidate locally even when DELETE fails; never automatically reinitialize/replay.
        state.initialized = false;
        if let Some(session) = state.session.take() {
            let response = self
                .transport
                .execute(self.request(Method::DELETE, Vec::new(), Some(session), Vec::new()))
                .await?;
            if !(200..300).contains(&response.status) && response.status != 405 {
                return Err(Error::new(K::Http(response.status), D::ResponseReceived));
            }
        }
        Ok(())
    }
    fn request(
        &self,
        method: Method,
        body: Vec<u8>,
        session: Option<String>,
        mut headers: Vec<(String, String)>,
    ) -> HttpRequest {
        if let Some(session) = session {
            headers.push(("mcp-session-id".into(), session));
        }
        HttpRequest {
            method,
            url: self.endpoint.clone(),
            authorization: self.authorization.clone(),
            content_type: Some("application/json".into()),
            body,
            mcp_headers: headers,
        }
    }
    async fn notify(
        &self,
        method: &str,
        params: Value,
        session: Option<String>,
    ) -> Result<(), Error> {
        let body = serde_json::to_vec(&json!({"jsonrpc":"2.0","method":method,"params":params}))
            .map_err(|_| Error::new(K::Protocol, D::NotSent))?;
        let response = self
            .transport
            .execute(self.request(Method::POST, body, session, Vec::new()))
            .await?;
        if response.status != 202 || !response.body.is_empty() {
            return Err(Error::new(K::Protocol, D::ResponseReceived));
        }
        Ok(())
    }
    async fn exchange(
        &self,
        method: &str,
        params: Value,
        session: Option<String>,
        headers: Vec<(String, String)>,
    ) -> Result<(RpcOutcome, Option<String>), Error> {
        self.exchange_cancellable(method, params, session, headers, None, None)
            .await
    }
    async fn exchange_cancellable(
        &self,
        method: &str,
        mut params: Value,
        session: Option<String>,
        mut headers: Vec<(String, String)>,
        mut cancellation: Option<oneshot::Receiver<()>>,
        initialization_session: Option<&mut Option<String>>,
    ) -> Result<(RpcOutcome, Option<String>), Error> {
        match cancellation.as_mut().map(|signal| signal.try_recv()) {
            Some(Ok(())) => return Err(Error::new(K::Cancelled, D::NotSent)),
            Some(Err(oneshot::error::TryRecvError::Closed)) => cancellation = None,
            _ => {}
        }
        let _slot = self
            .slots
            .try_acquire()
            .map_err(|_| Error::new(K::Busy, D::NotSent))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if self.revision == Revision::Modern20260728 {
            params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":self.revision.as_str(),"io.modelcontextprotocol/clientInfo":{"name":"kyomi-mcp-client","version":env!("CARGO_PKG_VERSION")},"io.modelcontextprotocol/clientCapabilities":{}});
            headers.push(("mcp-protocol-version".into(), self.revision.as_str().into()));
            headers.push(("mcp-method".into(), method.into()));
            if let Some(name) = params
                .get("name")
                .or_else(|| params.get("uri"))
                .and_then(Value::as_str)
            {
                headers.push(("mcp-name".into(), schema::header_value(name)));
            }
        }
        let body =
            serde_json::to_vec(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .map_err(|_| Error::new(K::Protocol, D::NotSent))?;
        if body.len() > self.limits.request_bytes {
            return Err(Error::new(K::Limit, D::NotSent));
        }
        let cancel_signal = async move {
            if let Some(receiver) = cancellation
                && receiver.await.is_ok()
            {
                return;
            }
            std::future::pending::<()>().await;
        };
        let response = tokio::select! {
            biased;
            response = self.transport.execute(self.request(Method::POST, body, session.clone(), headers)) => Some(response),
            _ = cancel_signal => None,
        };
        // The losing execute future is dropped before legacy cancellation is posted.
        let Some(response) = response else {
            if self.revision == Revision::Legacy20250326
                && let Err(mut error) = self
                    .notify("notifications/cancelled", json!({"requestId":id}), session)
                    .await
            {
                // This error describes the control POST, not the original write.
                error.dispatch = D::MayHaveBeenSent;
                return Err(error);
            }
            return Err(Error::new(K::Cancelled, D::MayHaveBeenSent));
        };
        let response = response?;
        // Retain an assigned legacy initialization session before decoding/status
        // checks so callers can close it even after a malformed initialization.
        if let Some(slot) = initialization_session
            && let Some(session) = &response.session_id
        {
            *slot = Some(session.clone());
        }
        if response.body.len() > self.limits.body_bytes {
            return Err(Error::new(K::Limit, D::ResponseReceived));
        }
        if response.status == 404 && self.revision == Revision::Legacy20250326 {
            return Err(Error::new(K::SessionExpired, D::ResponseReceived));
        }
        if !(200..300).contains(&response.status) {
            // Modern protocol errors may use HTTP 400/404; preserve their typed code.
            if response.status != 400 && response.status != 404 {
                return Err(Error::new(K::Http(response.status), D::ResponseReceived));
            }
        }
        let outcome = decode(&response, id, &self.limits)?;
        Ok((outcome, response.session_id))
    }
}
fn decode(response: &HttpResponse, id: u64, limits: &Limits) -> Result<RpcOutcome, Error> {
    let messages = match response.content_type.split(';').next() {
        Some("application/json") => vec![
            serde_json::from_slice(&response.body)
                .map_err(|_| Error::new(K::Protocol, D::ResponseReceived))?,
        ],
        Some("text/event-stream") => sse::messages(&response.body, limits, true)?,
        _ => return Err(Error::new(K::Protocol, D::ResponseReceived)),
    };
    let mut outcome = None;
    let mut notifications = 0;
    for message in messages {
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") || !message.is_object() {
            return Err(Error::new(K::Protocol, D::ResponseReceived));
        }
        if message.get("method").is_some() {
            if message.get("id").is_some() {
                return Err(Error::new(K::UnsupportedCapability, D::ResponseReceived));
            }
            if !matches!(
                message.get("method").and_then(Value::as_str),
                Some(
                    "notifications/progress"
                        | "notifications/message"
                        | "notifications/tools/list_changed"
                        | "notifications/resources/list_changed"
                )
            ) {
                return Err(Error::new(K::UnsupportedCapability, D::ResponseReceived));
            }
            notifications += 1;
            if notifications > limits.notifications {
                return Err(Error::new(K::Limit, D::ResponseReceived));
            }
            continue;
        }
        if message.get("id").and_then(Value::as_u64) != Some(id) || outcome.is_some() {
            return Err(Error::new(K::Protocol, D::ResponseReceived));
        }
        outcome = Some(match (message.get("result"), message.get("error")) {
            (Some(result), None) => {
                if result.get("inputRequests").is_some() || result.get("task").is_some() {
                    return Err(Error::new(K::UnsupportedCapability, D::ResponseReceived));
                }
                RpcOutcome::Result(result.clone())
            }
            (None, Some(error)) => RpcOutcome::RemoteError {
                code: error
                    .get("code")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))?,
                message: error
                    .get("message")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))?
                    .into(),
                data: error.get("data").cloned(),
            },
            _ => return Err(Error::new(K::Protocol, D::ResponseReceived)),
        });
    }
    outcome.ok_or_else(|| Error::new(K::Protocol, D::ResponseReceived))
}
