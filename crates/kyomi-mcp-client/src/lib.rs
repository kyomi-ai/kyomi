// SPDX-License-Identifier: AGPL-3.0-or-later
//! Outbound MCP only. No product identity, subprocesses, inbound service or global registry.
mod client;
mod http;
mod schema;
mod sse;

pub use client::{McpClient, Revision, RpcOutcome};
pub use http::{
    DeploymentPolicy, HttpRequest, HttpResponse, HttpTransport, Resolver, SafeHttpClient,
    SystemResolver, is_public_address,
};
use std::time::Duration;

/// Limits apply to each client/connection, independently of native tool execution.
#[derive(Clone, Debug)]
pub struct Limits {
    pub deadline: Duration,
    pub connect_timeout: Duration,
    pub body_bytes: usize,
    pub request_bytes: usize,
    pub sse_frame_bytes: usize,
    pub notifications: usize,
    pub catalog_bytes: usize,
    pub schema_bytes: usize,
    pub tools: usize,
    pub pages: usize,
    pub concurrency: usize,
    pub read_retries: usize,
    pub backoff: Duration,
    pub max_backoff: Duration,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(30),
            connect_timeout: Duration::from_secs(5),
            body_bytes: 4 * 1024 * 1024,
            request_bytes: 1024 * 1024,
            sse_frame_bytes: 1024 * 1024,
            notifications: 100,
            catalog_bytes: 4 * 1024 * 1024,
            schema_bytes: 64 * 1024,
            tools: 256,
            pages: 16,
            concurrency: 4,
            read_retries: 2,
            backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(2),
        }
    }
}
impl Limits {
    pub fn validate(&self) -> Result<(), Error> {
        if self.deadline.is_zero()
            || self.connect_timeout.is_zero()
            || self.body_bytes == 0
            || self.request_bytes == 0
            || self.sse_frame_bytes == 0
            || self.catalog_bytes == 0
            || self.schema_bytes == 0
            || self.tools == 0
            || self.pages == 0
            || self.concurrency == 0
            || self.read_retries > 8
            || self.backoff > self.max_backoff
        {
            return Err(Error::new(
                ErrorKind::Configuration,
                DispatchCertainty::NotSent,
            ));
        }
        Ok(())
    }
}
/// A failure after submission may have committed a remote write. Never replay it blindly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DispatchCertainty {
    NotSent,
    MayHaveBeenSent,
    ResponseReceived,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    Configuration,
    UnsafeUrl,
    Dns,
    Transport,
    Deadline,
    Busy,
    Cancelled,
    Limit,
    Redirect,
    Protocol,
    UnsupportedRevision,
    UnsupportedCapability,
    SessionExpired,
    Http(u16),
}
/// Deliberately carries no URLs, headers, credentials or raw remote error text.
#[derive(Debug, thiserror::Error)]
#[error("outbound MCP failure: {kind:?} ({dispatch:?})")]
pub struct Error {
    pub kind: ErrorKind,
    pub dispatch: DispatchCertainty,
}
impl Error {
    pub fn new(kind: ErrorKind, dispatch: DispatchCertainty) -> Self {
        Self { kind, dispatch }
    }
}
