// SPDX-License-Identifier: AGPL-3.0-or-later

//! The `server_fn` `Client` every `#[server(...)]` in this crate names via
//! `client = crate::server_fns::paywall_client::PaywallAwareClient`
//! (KYO-806).
//!
//! `server_fn` 0.8 has no global request/response hook — the `#[server]`
//! macro hard-codes `server_fn::client::browser::BrowserClient` as the
//! `Client` implementation unless a `client = ...` argument overrides it
//! (`server_fn_macro::ServerFnArgs::client_type`,
//! `server_fn_macro-0.8.10/src/lib.rs` ~L140). Intercepting every
//! server-fn response for a `402 payment_required` status therefore means
//! providing that override on every `#[server(...)]` attribute — this type
//! is that override. A source-scan test
//! (`kyo_806_paywall_client_allowlist_tests` in `server_fns/mod.rs`) fails
//! the build if a future `#[server(...)]` in this crate omits it, closing
//! the class the same way `kyo_805_billing_gate_allowlist_tests` already
//! does for `extract_allow_lapsed`.
//!
//! `BrowserClient` is the macro's default under **both** `ssr` and
//! `hydrate` — the `ssr` build never actually calls `Client::send` (the
//! server dispatches the function body directly), but the generated
//! `impl ServerFn` still names the type, so this wrapper must compile
//! (not necessarily do anything useful) under both feature sets. It does:
//! delegating every method straight to `BrowserClient` costs nothing on the
//! `ssr` side and is the real interception point on `hydrate`.

use std::future::Future;

use bytes::Bytes;
use futures_util::{Sink, Stream};
use leptos::server_fn::client::{browser::BrowserClient, Client};
use leptos::server_fn::error::FromServerFnError;
use leptos::server_fn::response::ClientRes;

use crate::utils::billing_lapse::{is_payment_required_status, report_payment_required};

/// A `server_fn::client::Client` that behaves exactly like
/// [`BrowserClient`], except every response is checked for HTTP 402
/// (`kyomi_core::Error::PaymentRequired`, KYO-805) on the way back — see the
/// module doc for why every `#[server(...)]` in this crate must name this
/// type as its `client`.
pub struct PaywallAwareClient;

impl<Error, InputStreamError, OutputStreamError>
    Client<Error, InputStreamError, OutputStreamError> for PaywallAwareClient
where
    Error: FromServerFnError,
    InputStreamError: FromServerFnError,
    OutputStreamError: FromServerFnError,
{
    type Request =
        <BrowserClient as Client<Error, InputStreamError, OutputStreamError>>::Request;
    type Response =
        <BrowserClient as Client<Error, InputStreamError, OutputStreamError>>::Response;

    async fn send(req: Self::Request) -> Result<Self::Response, Error> {
        let res =
            <BrowserClient as Client<Error, InputStreamError, OutputStreamError>>::send(req)
                .await?;
        if is_payment_required_status(<Self::Response as ClientRes<Error>>::status(&res)) {
            report_payment_required();
        }
        Ok(res)
    }

    fn open_websocket(
        path: &str,
    ) -> impl Future<
        Output = Result<
            (
                impl Stream<Item = Result<Bytes, Bytes>> + Send + 'static,
                impl Sink<Bytes> + Send + 'static,
            ),
            Error,
        >,
    > + Send {
        // Not used by this crate's own WebSocket protocol (the sync engine
        // and chat connect via `crate::utils::websocket` directly, not
        // through `server_fn`'s websocket support) — delegated for
        // completeness and correctness should that ever change.
        <BrowserClient as Client<Error, InputStreamError, OutputStreamError>>::open_websocket(
            path,
        )
    }

    fn spawn(future: impl Future<Output = ()> + Send + 'static) {
        <BrowserClient as Client<Error, InputStreamError, OutputStreamError>>::spawn(future);
    }
}
