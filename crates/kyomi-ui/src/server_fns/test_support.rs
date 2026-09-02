// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared test-only support for `#[server]` fns whose input must decode as
//! real JSON (`server_fn::codec::Json`) rather than the macro's default
//! `PostUrl` (`serde_qs` over `application/x-www-form-urlencoded`).
//!
//! `serde_json::Value` is self-describing, so its `Deserialize` impl defers
//! entirely to the format doing the decoding. Under `PostUrl`, every leaf
//! decodes as a JSON *string*, because `serde_qs` has no type information
//! beyond "this looks like text in a form field" — silently corrupting any
//! numeric or boolean leaf. KYO-428 (`datasources.rs`'s
//! `create_datasource_modal`/`update_datasource_settings`/
//! `discover_datasource_resources`/`save_datasource_credentials`) and
//! KYO-459 (`sql_editor.rs`'s `generate_chart_from_results`) are two
//! independent instances of exactly this bug, both fixed the same way:
//! `#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]`.
//!
//! [`assert_json_input_codec`] guards that fix at the source: it inspects
//! the `server_fn::ServerFn::Protocol` associated type the `#[server]`
//! macro actually generates for a given function, and asserts the *input*
//! side of that protocol is not `PostUrl` — the one property that flips if
//! the attribute is ever removed or edited back to the default. A test that
//! instead built a local struct or a decoding truth table would keep
//! passing even if `input = ...` were deleted, since nothing would exercise
//! the macro-generated wire type at all.
//!
//! ## Provenance (KYO-476)
//!
//! Extracted from two near-identical private copies: `datasources.rs`'s
//! `json_input_codec_tests::{input_encoding_of, assert_json_input_codec}`
//! and `sql_editor.rs`'s `chart_json_codec_tests::input_encoding_of` (whose
//! single call site had its assertion logic inlined directly, rather than
//! extracted, because at the time it was the only function needing this
//! guard). A third `#[server]` fn adopting the KYO-428/KYO-459 precedent
//! would otherwise have been a third copy-paste — see
//! `docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md`.
//!
//! `input_encoding_of` was byte-identical between the two originals and is
//! reproduced here unchanged. `assert_json_input_codec`'s two originals
//! differed only in the *regression-specific* half of the first failure
//! message — KYO-428's port/secure/encrypt/trust_server_certificate wording
//! vs KYO-459's sample_rows/analyze_chart_column wording — so that half is
//! now the `regression_context` parameter, letting every call site keep its
//! own exact wording rather than picking one and discarding the other's.

use leptos::server_fn::ServerFn;

/// Extract the type name of the *first* generic argument of
/// `server_fn::Http<Input, Output>` from a full `type_name::<Protocol>()`
/// string — i.e. the input encoding. Splitting on the first top-level
/// comma is sufficient here because neither `PostUrl` nor
/// `Post<JsonEncoding>` (what `server_fn::codec::Json` expands to)
/// contains a comma of its own.
fn input_encoding_of(protocol_type_name: &str) -> &str {
    protocol_type_name
        .split_once("Http<")
        .and_then(|(_, rest)| rest.split_once(','))
        .map(|(input, _)| input)
        .unwrap_or_else(|| {
            panic!(
                "expected `{protocol_type_name}` to be a server_fn::Http<Input, Output> \
                 protocol with a comma-separated generic argument list"
            )
        })
}

/// Assert a server_fn's `Protocol::Input` side is the JSON codec, not
/// the default `PostUrl` form codec. `T` is one of the PascalCase
/// structs the `#[server]` macro generates for the annotated function
/// (`CreateDatasourceModal`, `GenerateChartFromResults`, etc.) — asserting
/// against the real macro-generated type, not a hand-rolled stand-in, is
/// what makes this load-bearing against the attribute actually being
/// removed.
///
/// `regression_context` is the ticket-specific explanation of what silently
/// breaks under the default `PostUrl` codec for the function under test. It
/// is spliced into the first failure message so each call site keeps its
/// own exact wording — see the module doc comment above for why this is a
/// parameter rather than one fixed string shared by every caller.
pub(crate) fn assert_json_input_codec<T: ServerFn>(regression_context: &str) {
    let protocol = std::any::type_name::<T::Protocol>();
    let input_encoding = input_encoding_of(protocol);
    assert!(
        !input_encoding.contains("PostUrl"),
        "expected a JSON input codec, but {protocol} still uses the \
         default form-urlencoded PostUrl codec — {regression_context}"
    );
    assert!(
        input_encoding.contains("JsonEncoding"),
        "expected the input encoding to be server_fn::codec::Json \
         (JsonEncoding), got {input_encoding} in protocol {protocol}"
    );
}
