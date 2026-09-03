// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared BigQuery REST API response parsing and list pagination.
//!
//! `datasets.list` and `tables.list` return **HTTP 200 with the list key
//! entirely absent** when the caller cannot see the resource — they do not
//! return 403. Confirmed against the live API (KYO-619):
//!
//! ```text
//! nexata-dev      (has access)  -> 200, "datasets" key PRESENT
//! filemaker-1105  (no access)   -> 200, "datasets" key ABSENT
//!                                  body: {"kind":"bigquery#datasetList","etag":"..."}
//! ```
//!
//! Before this module existed, every one of the **five** call sites that
//! parse a BigQuery list response collapsed "key absent" and "key present,
//! empty array" onto the same `Vec::new()` via `.unwrap_or_default()`:
//!
//! - `user_dataset::list_bigquery_datasets` (`datasets` key)
//! - `user_dataset::list_bigquery_tables` (`tables` key)
//! - `user_dataset::get_bigquery_table_schema` (`schema.fields` key)
//! - `bigquery_public::index_public_dataset_tables`'s inline table listing (`tables` key)
//! - `bigquery_public::get_public_table_schema` (`schema.fields` key)
//!
//! A missing key is not a legitimate zero — it means the response never
//! answered the question — so folding it into an empty `Vec` makes "the
//! project is genuinely empty" and "we can't see the project" the same byte,
//! and the catalog archiver treats the latter as "every table was deleted"
//! (see `docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md`).
//!
//! [`parse_list_field`] is the single place that distinguishes the three
//! states a BigQuery list response can be in. [`paginate`] drives it across
//! every page of a `nextPageToken`-paginated endpoint, mirroring the loop
//! shape in `kyomi-connect`'s `BigQueryProvider::list_active_projects`
//! (`~/repos/kyomi-connect/crates/kyomi-datasource/src/providers/bigquery.rs`).

use kyomi_core::Result;
use serde_json::Value;
use tracing::{debug, warn};

use crate::catalog::types::ColumnEntry;

/// The `missing_key_hint` every BigQuery *listing* call site
/// (`datasets.list`, `tables.list`) passes to [`parse_list_field`]/
/// [`paginate`] — a single shared literal so the three listing call sites
/// can't drift into three different wordings of the same hint. The two
/// `schema.fields` call sites deliberately pass `None` instead; see
/// `parse_list_field`'s doc comment for why.
pub(crate) const MISSING_LIST_KEY_HINT: &str = "likely no access to this resource";

/// Extract a dataset id from one `datasets[]` entry of a BigQuery
/// `datasets.list` response. Shared by `user_dataset` and (indirectly, via
/// its own project enumeration) `bigquery_public`.
pub(crate) fn extract_dataset_id(ds: &Value) -> Option<String> {
    ds.get("datasetReference")
        .and_then(|r| r.get("datasetId"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract a table id from one `tables[]` entry of a BigQuery `tables.list`
/// response. Shared by `user_dataset` and `bigquery_public`.
pub(crate) fn extract_table_id(t: &Value) -> Option<String> {
    t.get("tableReference")
        .and_then(|r| r.get("tableId"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// Extract a [`ColumnEntry`] from one `schema.fields[]` entry of a BigQuery
/// `tables.get` response. Entries missing a `name` are dropped by the
/// caller (via `filter_map`/`parse_list_field`), not treated as fatal.
/// Shared by `user_dataset` and `bigquery_public`.
pub(crate) fn extract_column_entry(field: &Value) -> Option<ColumnEntry> {
    let name = field.get("name")?.as_str()?.to_string();
    let col_type = field.get("type").and_then(|v| v.as_str()).map(String::from);
    let description = field
        .get("description")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from);

    Some(ColumnEntry {
        name,
        col_type: col_type.clone(),
        native_type: col_type,
        description,
    })
}

/// Parse one JSON array field from a BigQuery REST response body,
/// distinguishing the three states that matter:
///
/// - `key` is **absent** from `body` — the response did not answer the
///   question (BigQuery's documented shape for "caller cannot see this
///   resource": HTTP 200, key entirely missing) -> `Err`.
/// - `key` is **present but not a JSON array** — a malformed/unexpected
///   response shape -> `Err`.
/// - `key` is **present and is an array**, empty or not -> `Ok`, applying
///   `extract` to every element and silently dropping elements `extract`
///   returns `None` for (e.g. a listed resource missing its id field — a
///   per-entry defect, not a per-response one, so it does not fail the
///   whole page).
///
/// This same rule applies on every page of a paginated listing, not just the
/// first: see [`paginate`]'s doc comment for why an absent key on page 2+ is
/// still an error rather than "no more results this page".
///
/// `missing_key_hint` is appended to the absent-key error only, and only
/// when `Some`. It exists because "absent key" means something different
/// depending on which caller is asking: at the three *listing* call sites
/// (`datasets`/`tables`), an absent key is the production incident this
/// module exists to fix — the caller most likely can't see the resource —
/// so those pass a hint saying so. At the two `schema.fields` call sites
/// (`get_bigquery_table_schema`/`get_public_table_schema`), a preceding
/// `tables.get` has already succeeded, which proves access; an absent
/// `fields` key there means something else entirely (e.g. a resource type
/// BigQuery doesn't attach a schema to), and hinting "no access" would send
/// an on-call engineer the wrong way — so those callers pass `None`.
pub(crate) fn parse_list_field<T>(
    body: &Value,
    key: &str,
    extract: impl Fn(&Value) -> Option<T>,
    missing_key_hint: Option<&str>,
) -> Result<Vec<T>> {
    // KYO-616: closes a `tracing` interest-cache race across parallel test
    // threads — see the doc comment on `catalog::helpers::test_tracing_race_guard`.
    #[cfg(test)]
    crate::catalog::helpers::test_tracing_race_guard::ensure_installed();

    let Some(value) = body.get(key) else {
        // KYO-616: this is the exact production incident KYO-619 fixed —
        // log it as its own field-carrying line (not just the propagated
        // `Err`'s text) so "absent" is directly greppable/queryable,
        // distinct from "present but empty" below.
        warn!(
            key,
            key_state = "absent",
            "BigQuery list field absent from response — not treated as zero results"
        );
        let hint = missing_key_hint.map(|h| format!(" ({h})")).unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery response missing expected \"{key}\" field — the API did not confirm \
             zero results{hint}"
        )));
    };

    let Some(array) = value.as_array() else {
        warn!(key, key_state = "not_array", "BigQuery list field is not a JSON array");
        return Err(kyomi_core::Error::Internal(format!(
            "BigQuery response \"{key}\" field is not an array"
        )));
    };

    let array_len = array.len();
    let extracted: Vec<T> = array.iter().filter_map(extract).collect();
    debug!(
        key,
        key_state = if array_len == 0 { "present_empty" } else { "populated" },
        array_len,
        extracted_len = extracted.len(),
        "BigQuery list field parsed"
    );

    Ok(extracted)
}

/// Perform one page of a BigQuery REST list GET request — `maxResults`
/// always, plus `pageToken` once a previous page has handed one back — and
/// return the parsed JSON body.
///
/// Shared by every BigQuery list endpoint (`datasets.list`, `tables.list`,
/// in both the user-OAuth and public-dataset indexers) so the request and
/// non-2xx handling isn't a further copy of itself alongside the parsing
/// duplication this module was written to close.
pub(crate) async fn fetch_bigquery_list_page(
    client: &reqwest::Client,
    access_token: &str,
    url: &str,
    page_token: Option<String>,
    request_label: &str,
) -> Result<Value> {
    // KYO-616: see `catalog::helpers::test_tracing_race_guard`'s doc comment.
    #[cfg(test)]
    crate::catalog::helpers::test_tracing_race_guard::ensure_installed();

    let mut query: Vec<(&str, String)> =
        vec![("maxResults", super::BIGQUERY_API_MAX_RESULTS.to_string())];
    if let Some(token) = page_token {
        query.push(("pageToken", token));
    }

    let resp = client
        .get(url)
        .query(&query)
        .header("Authorization", format!("Bearer {access_token}"))
        .timeout(std::time::Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| kyomi_core::Error::Internal(format!("{request_label} failed: {e}")))?;

    let status = resp.status();
    if !status.is_success() {
        // KYO-616: the response body is not logged (it may echo request
        // details) — only the status, which is what a caller needs to tell
        // "denied" from "server error" from "not found" at a glance.
        warn!(request_label, status = %status, "BigQuery list page request failed");
        let body = resp.text().await.unwrap_or_default();
        return Err(kyomi_core::Error::Internal(format!(
            "{request_label} failed (HTTP {status}): {body}"
        )));
    }
    debug!(request_label, status = %status, "BigQuery list page fetched");

    resp.json().await.map_err(|e| {
        kyomi_core::Error::Internal(format!("Failed to parse {request_label} response: {e}"))
    })
}

/// Drive a `nextPageToken`-paginated BigQuery REST list endpoint to
/// completion, folding every page's `list_key` array field through
/// [`parse_list_field`].
///
/// `fetch_page` performs one page's HTTP round trip given the previous
/// page's token (`None` for the first page) and returns the parsed JSON
/// body. Splitting the loop out from the HTTP client this way is what makes
/// it directly testable: kyomi-auth has no HTTP-mocking dev-dependency
/// (KYO-623), so tests drive `paginate` with a `fetch_page` closure reading
/// from an in-memory sequence of fixture `Value` pages instead of the
/// network. That means the exact loop-termination and cross-page
/// accumulation logic production code runs is what gets exercised — not a
/// re-derivation of it in the test body.
///
/// Termination: the loop stops as soon as a page's `nextPageToken` is
/// either absent or an empty string, matching the reference implementation
/// (`list_active_projects` in kyomi-connect) — `Some(token) if
/// !token.is_empty()` is the only condition that continues the loop, so
/// there is no path back to the top without a fresh, non-empty token pulled
/// from the page that was just fetched. A page can therefore be requested at
/// most once per token the server hands back; a server that always returns
/// the same non-empty token would loop forever, but that is true of the
/// reference implementation too and is a server misbehavior outside what a
/// client-side loop guard can fix.
///
/// **Absent list key on page 2+**: treated identically to an absent key on
/// page 1 — an `Err`, not an empty contribution. An absent key on the first
/// page means "no access"; an absent key on a later page of an
/// already-in-progress enumeration means the server started answering and
/// then stopped mid-page, which is a truncated enumeration. Silently
/// accepting that would reproduce exactly the failure mode this module
/// exists to close (a partial result that looks complete), just one page in
/// instead of zero.
///
/// `missing_key_hint` is forwarded unchanged to every page's
/// [`parse_list_field`] call — see that function's doc comment for what it
/// does and why the two classes of caller (listing vs. single-resource
/// `schema.fields` reads) pass different values.
pub(crate) async fn paginate<T, F, Fut>(
    list_key: &str,
    extract: impl Fn(&Value) -> Option<T>,
    missing_key_hint: Option<&str>,
    mut fetch_page: F,
) -> Result<Vec<T>>
where
    F: FnMut(Option<String>) -> Fut,
    Fut: std::future::Future<Output = Result<Value>>,
{
    // KYO-616: see `catalog::helpers::test_tracing_race_guard`'s doc comment.
    #[cfg(test)]
    crate::catalog::helpers::test_tracing_race_guard::ensure_installed();

    let mut results = Vec::new();
    let mut page_token: Option<String> = None;

    loop {
        let body = fetch_page(page_token.take()).await?;
        let items = parse_list_field(&body, list_key, &extract, missing_key_hint)?;
        let page_item_count = items.len();
        results.extend(items);

        page_token = body
            .get("nextPageToken")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
            .map(String::from);

        debug!(
            list_key,
            page_item_count,
            next_page_token_present = page_token.is_some(),
            total_so_far = results.len(),
            "BigQuery list pagination page processed"
        );

        if page_token.is_none() {
            break;
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Thin wrapper around `kyomi_test_tracing::capture_tracing()` — the
    /// actual KYO-616 interest-cache race fix now lives at the source, in
    /// `parse_list_field`/`fetch_bigquery_list_page`/`paginate` themselves
    /// via `catalog::helpers::test_tracing_race_guard::ensure_installed()`.
    /// See that module's doc comment for the full mechanism, and why a
    /// `rebuild_interest_cache()` call here alone was tried and confirmed
    /// insufficient.
    fn capture_tracing_for_test() -> kyomi_test_tracing::TracingCapture {
        kyomi_test_tracing::capture_tracing()
    }

    fn dataset_id(v: &Value) -> Option<String> {
        v.get("datasetReference")?
            .get("datasetId")?
            .as_str()
            .map(String::from)
    }

    fn table_id(v: &Value) -> Option<String> {
        v.get("tableReference")?
            .get("tableId")?
            .as_str()
            .map(String::from)
    }

    // ── parse_list_field: the three states, "datasets" key ────────────────

    /// The exact observed body from a project the caller cannot see.
    #[test]
    fn datasets_key_absent_is_err() {
        let body = json!({"kind": "bigquery#datasetList", "etag": "1B2M2Y8AsgTpgAmY7PhCfg=="});
        let result = parse_list_field(&body, "datasets", dataset_id, None);
        assert!(
            result.is_err(),
            "an absent \"datasets\" key must not be treated as zero datasets"
        );
    }

    /// The regression that matters most: a genuinely empty, accessible
    /// project must not start failing.
    #[test]
    fn datasets_key_present_but_empty_is_ok_empty() {
        let body = json!({"kind": "bigquery#datasetList", "datasets": []});
        let result = parse_list_field(&body, "datasets", dataset_id, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn datasets_key_present_with_entries_is_ok_entries() {
        let body = json!({
            "kind": "bigquery#datasetList",
            "datasets": [
                {"datasetReference": {"datasetId": "ds_a"}},
                {"datasetReference": {"datasetId": "ds_b"}},
            ],
        });
        let result = parse_list_field(&body, "datasets", dataset_id, None).unwrap();
        assert_eq!(result, vec!["ds_a".to_string(), "ds_b".to_string()]);
    }

    // ── parse_list_field: the three states, "tables" key ──────────────────

    #[test]
    fn tables_key_absent_is_err() {
        let body = json!({"kind": "bigquery#tableList"});
        let result = parse_list_field(&body, "tables", table_id, None);
        assert!(
            result.is_err(),
            "an absent \"tables\" key must not be treated as zero tables"
        );
    }

    #[test]
    fn tables_key_present_but_empty_is_ok_empty() {
        let body = json!({"kind": "bigquery#tableList", "tables": []});
        let result = parse_list_field(&body, "tables", table_id, None).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn tables_key_present_with_entries_is_ok_entries() {
        let body = json!({
            "kind": "bigquery#tableList",
            "tables": [
                {"tableReference": {"tableId": "t1"}},
                {"tableReference": {"tableId": "t2"}},
            ],
        });
        let result = parse_list_field(&body, "tables", table_id, None).unwrap();
        assert_eq!(result, vec!["t1".to_string(), "t2".to_string()]);
    }

    // ── malformed shapes ────────────────────────────────────────────────

    #[test]
    fn key_present_but_not_an_array_is_err() {
        let body = json!({"datasets": "not-an-array"});
        let result = parse_list_field(&body, "datasets", dataset_id, None);
        assert!(result.is_err());
    }

    #[test]
    fn entries_missing_id_field_are_dropped_not_fatal() {
        let body = json!({
            "datasets": [
                {"datasetReference": {"datasetId": "ds_a"}},
                {"datasetReference": {}},
                {"somethingElse": true},
            ],
        });
        let result = parse_list_field(&body, "datasets", dataset_id, None).unwrap();
        assert_eq!(
            result,
            vec!["ds_a".to_string()],
            "malformed individual entries are dropped, not treated as a whole-response failure"
        );
    }

    // ── response-shape logging (KYO-616) ───────────────────────────────────
    //
    // KYO-619 made absent-vs-empty a real distinction in `parse_list_field`'s
    // *return value*; these pin that the distinction is also visible in
    // logs — status/key-state/array-length/pagination — without needing a
    // database diff to reconstruct what a BigQuery list call actually saw.

    #[test]
    fn absent_key_logs_warn_with_key_state_absent() {
        let logs = capture_tracing_for_test();
        let body = json!({"kind": "bigquery#datasetList"});
        let _ = parse_list_field(&body, "datasets", dataset_id, None);

        assert!(
            logs.has_message_containing(tracing::Level::WARN, "key_state=\"absent\""),
            "expected a WARN naming the absent key state; captured: {:?}",
            logs.events()
        );
    }

    #[test]
    fn present_empty_key_logs_debug_with_key_state_and_zero_length() {
        let logs = capture_tracing_for_test();
        let body = json!({"kind": "bigquery#datasetList", "datasets": []});
        let _ = parse_list_field(&body, "datasets", dataset_id, None).unwrap();

        assert!(
            logs.has_message_containing(
                tracing::Level::DEBUG,
                "key_state=\"present_empty\""
            ),
            "expected a log line distinguishing present-but-empty from absent; captured: {:?}",
            logs.events()
        );
        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "array_len=0"),
            "captured: {:?}",
            logs.events()
        );
    }

    #[test]
    fn populated_key_logs_debug_with_the_real_array_length() {
        let logs = capture_tracing_for_test();
        let body = json!({
            "datasets": [
                {"datasetReference": {"datasetId": "ds_a"}},
                {"datasetReference": {"datasetId": "ds_b"}},
                {"datasetReference": {"datasetId": "ds_c"}},
            ],
        });
        let _ = parse_list_field(&body, "datasets", dataset_id, None).unwrap();

        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "key_state=\"populated\""),
            "captured: {:?}",
            logs.events()
        );
        assert!(
            logs.has_message_containing(tracing::Level::DEBUG, "array_len=3"),
            "the log must carry the real element count, not just a truthy flag; captured: {:?}",
            logs.events()
        );
    }

    #[tokio::test]
    async fn paginate_logs_next_page_token_presence_per_page() {
        let logs = capture_tracing_for_test();
        let pages = [
            json!({
                "datasets": [{"datasetReference": {"datasetId": "ds_a"}}],
                "nextPageToken": "page-2",
            }),
            json!({
                "datasets": [{"datasetReference": {"datasetId": "ds_b"}}],
            }),
        ];

        let mut call = 0usize;
        let _ = paginate("datasets", dataset_id, None, |_token| {
            call += 1;
            let page = pages[call - 1].clone();
            async move { Ok(page) }
        })
        .await
        .unwrap();

        let debug_events = logs.events_at(tracing::Level::DEBUG);
        assert!(
            debug_events
                .iter()
                .any(|(_, msg)| msg.contains("next_page_token_present=true")),
            "the first page must report a present nextPageToken; captured: {:?}",
            logs.events()
        );
        assert!(
            debug_events
                .iter()
                .any(|(_, msg)| msg.contains("next_page_token_present=false")),
            "the final page must report an absent nextPageToken; captured: {:?}",
            logs.events()
        );
    }

    // ── missing_key_hint (the two classes of caller) ──────────────────────

    /// Listing call sites pass a hint; it must actually show up in the
    /// error an on-call engineer sees.
    #[test]
    fn absent_key_error_includes_hint_when_provided() {
        let body = json!({"kind": "bigquery#datasetList"});
        let err = parse_list_field(&body, "datasets", dataset_id, Some(MISSING_LIST_KEY_HINT))
            .unwrap_err();
        assert!(
            err.to_string().contains(MISSING_LIST_KEY_HINT),
            "hint must appear in the error text, got: {err}"
        );
    }

    /// `schema.fields` call sites pass `None` — the "no access" hint would
    /// be actively misleading there (a preceding `tables.get` already
    /// proved access), so it must not appear.
    #[test]
    fn absent_key_error_omits_hint_when_none() {
        let body = json!({"kind": "bigquery#datasetList"});
        let err = parse_list_field(&body, "datasets", dataset_id, None).unwrap_err();
        assert!(
            !err.to_string().contains("access"),
            "no hint was provided, so the error must not fabricate one, got: {err}"
        );
    }

    // ── paginate ────────────────────────────────────────────────────────

    /// Every entry across pages is returned, and the loop stops once a page
    /// arrives with no `nextPageToken`.
    #[tokio::test]
    async fn multi_page_returns_every_entry_and_terminates() {
        let pages = [
            json!({
                "datasets": [{"datasetReference": {"datasetId": "ds_a"}}],
                "nextPageToken": "page-2",
            }),
            json!({
                "datasets": [{"datasetReference": {"datasetId": "ds_b"}}],
                // no nextPageToken -> terminate
            }),
        ];

        let mut requested_tokens: Vec<Option<String>> = Vec::new();
        let result = paginate("datasets", dataset_id, None, |token| {
            requested_tokens.push(token.clone());
            let page = pages[requested_tokens.len() - 1].clone();
            async move { Ok(page) }
        })
        .await
        .unwrap();

        assert_eq!(result, vec!["ds_a".to_string(), "ds_b".to_string()]);
        assert_eq!(
            requested_tokens,
            vec![None, Some("page-2".to_string())],
            "second request must carry the token the first page returned"
        );
    }

    /// An empty-string `nextPageToken` must terminate the loop rather than
    /// being treated as "there is a next page" and looping forever.
    #[tokio::test]
    async fn empty_string_next_page_token_terminates() {
        let mut calls = 0usize;
        let result = paginate("datasets", dataset_id, None, |_token| {
            calls += 1;
            async move {
                Ok(json!({
                    "datasets": [{"datasetReference": {"datasetId": "ds_a"}}],
                    "nextPageToken": "",
                }))
            }
        })
        .await
        .unwrap();

        assert_eq!(result, vec!["ds_a".to_string()]);
        assert_eq!(calls, 1, "an empty-string token must not trigger a second page fetch");
    }

    /// An absent list key on the second page of an in-progress enumeration
    /// is a truncated enumeration, not "no more items" — it must fail the
    /// same way an absent key on page 1 does.
    #[tokio::test]
    async fn absent_key_on_subsequent_page_is_err_not_silently_accepted() {
        let mut call = 0usize;
        let result: Result<Vec<String>> = paginate("datasets", dataset_id, None, |_token| {
            call += 1;
            let this_call = call;
            async move {
                if this_call == 1 {
                    Ok(json!({
                        "datasets": [{"datasetReference": {"datasetId": "ds_a"}}],
                        "nextPageToken": "page-2",
                    }))
                } else {
                    // page 2 answers with 200 but the key is missing entirely.
                    Ok(json!({"kind": "bigquery#datasetList"}))
                }
            }
        })
        .await;

        assert!(
            result.is_err(),
            "a missing key on page 2 must surface as an error, not truncate silently"
        );
    }

    /// A single page with no `nextPageToken` at all makes exactly one
    /// request.
    #[tokio::test]
    async fn single_page_response_makes_one_request() {
        let mut calls = 0usize;
        let result = paginate("tables", table_id, None, |_token| {
            calls += 1;
            async move { Ok(json!({"tables": []})) }
        })
        .await
        .unwrap();

        assert!(result.is_empty());
        assert_eq!(calls, 1);
    }
}
