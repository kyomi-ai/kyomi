// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared query execution utilities for agent tools.
//!
//! Extracts the common pattern of:
//! 1. Resolve datasource by slug
//! 2. Get/decrypt credentials
//! 3. Create provider
//! 4. Execute query
//! 5. Convert rows to dict format
//!
//! Used by `forecast_data`, `render_chart`, and dashboard tools.


use arrow_array::{
    Array, BooleanArray, Date32Array, Float64Array, Int32Array, Int64Array, LargeStringArray,
    StringArray, TimestampMicrosecondArray, UInt64Array,
};
use serde_json::Value;
use tracing;

use super::QueryContext;

/// Unrestricted SQL may commit before a provider reports a timeout/error. Both
/// transport errors and returned error envelopes lack a rollback guarantee.
pub(super) fn sql_result_domain_outcome(text: &str) -> agent_runtime::DomainOutcome {
    match serde_json::from_str::<Value>(text) {
        Ok(value) if value.get("error").is_none_or(Value::is_null) => agent_runtime::DomainOutcome::Succeeded,
        _ => agent_runtime::DomainOutcome::Unknown,
    }
}

/// Skip a quoted string/identifier, honouring doubled-quote and backslash
/// escapes. Returns the index just past the closing quote (or the end).
fn skip_clickhouse_quoted(bytes: &[u8], mut i: usize, quote: u8) -> usize {
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == quote {
            if bytes.get(i + 1) == Some(&quote) {
                i += 2;
            } else {
                return i + 1;
            }
        } else {
            i += 1;
        }
    }
    i
}

/// Skip a (possibly nested) block comment. `i` points at the opening `/`.
fn skip_clickhouse_block_comment(bytes: &[u8], mut i: usize) -> usize {
    i += 2;
    let mut depth = 1;
    while i + 1 < bytes.len() && depth > 0 {
        if bytes[i] == b'/' && bytes[i + 1] == b'*' {
            depth += 1;
            i += 2;
        } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            depth -= 1;
            i += 2;
        } else {
            i += 1;
        }
    }
    i
}

fn skip_clickhouse_line_comment(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

fn is_clickhouse_statement_starter(word: &str) -> bool {
    matches!(
        word,
        "SELECT"
            | "INSERT"
            | "CREATE"
            | "ALTER"
            | "DROP"
            | "TRUNCATE"
            | "RENAME"
            | "UPDATE"
            | "DELETE"
            | "OPTIMIZE"
            | "ATTACH"
            | "DETACH"
            | "USE"
            | "SET"
            | "KILL"
            | "SYSTEM"
            | "GRANT"
            | "REVOKE"
            | "EXPLAIN"
            | "SHOW"
            | "DESCRIBE"
            | "DESC"
            | "EXISTS"
            | "CHECK"
            | "WATCH"
            | "BACKUP"
            | "RESTORE"
            | "MOVE"
            | "EXCHANGE"
            | "UNDROP"
            | "REFRESH"
            | "LIGHTWEIGHT"
    )
}

/// Return the top-level statement keyword, skipping strings, quoted
/// identifiers and comments. A leading `WITH` is a CTE prefix: the first
/// top-level statement keyword after it decides whether this is a `SELECT` or
/// a mutation such as `WITH ... INSERT`.
fn clickhouse_statement_keyword(sql: &str) -> Option<String> {
    let bytes = sql.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut first_word: Option<String> = None;
    let mut after_with = false;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"' | b'`') => {
                i = skip_clickhouse_quoted(bytes, i, quote);
            }
            b'-' if bytes.get(i + 1) == Some(&b'-') => {
                i = skip_clickhouse_line_comment(bytes, i);
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i = skip_clickhouse_block_comment(bytes, i);
            }
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            byte if byte.is_ascii_alphabetic() || byte == b'_' => {
                let start = i;
                i += 1;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                {
                    i += 1;
                }
                if depth == 0 {
                    let word = sql[start..i].to_ascii_uppercase();
                    if first_word.is_none() {
                        first_word = Some(word.clone());
                        if word == "WITH" {
                            after_with = true;
                        } else {
                            return Some(word);
                        }
                    } else if after_with && is_clickhouse_statement_starter(&word) {
                        return Some(word);
                    }
                }
            }
            _ => i += 1,
        }
    }
    first_word
}

/// A single read-only ClickHouse statement. `WITH ... SELECT` is accepted;
/// `WITH ... INSERT` and every other top-level mutation are not. Batches and
/// ambiguous statements stay uncertain because an earlier statement can have
/// committed.
fn clickhouse_read_only_query(sql: &str) -> bool {
    // Conservatively exclude batches. Semicolons inside strings/comments may
    // exclude a safe query too; they must never authorize retrying a batch that
    // could have committed an earlier statement.
    let trimmed = sql.trim();
    let single_statement = !trimmed.strip_suffix(';').unwrap_or(trimmed).contains(';');
    single_statement && clickhouse_statement_keyword(trimmed).as_deref() == Some("SELECT")
}

/// ClickHouse renders server-side exceptions as `Code: N. DB::Exception: ...`.
/// Client-side transport and response-decoding failures use other shapes, so
/// they cannot prove the query reached the server's analyzer.
fn clickhouse_server_error(error: &str) -> bool {
    error.starts_with("Code: ") && error.contains(" DB::Exception: ")
}

fn clickhouse_error_code(error: &str) -> Option<u16> {
    error
        .strip_prefix("Code: ")?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Errors that a ClickHouse `EXPLAIN` can raise during parsing or analysis.
/// Execution, admission and resource codes (notably 159 timeout, 202 too many
/// simultaneous queries, 209 socket timeout and 241 memory limit) are
/// deliberately absent: a dry run failing with one of those does not prove the
/// original statement was rejected before execution.
const CLICKHOUSE_ANALYSIS_ERROR_CODES: &[u16] = &[
    6, 8, 15, 16, 20, 25, 26, 27, 34, 35, 36, 37, 38, 41, 42, 43, 44, 46, 47, 48, 50, 51, 52, 53,
    59, 60, 62, 63, 70, 72, 73, 78, 81, 115, 133, 134, 174, 179, 181, 182, 183, 184, 206, 207,
    215, 223, 258, 264, 352, 376, 377, 378, 403, 406, 417, 440, 441, 467, 475, 476, 498, 544,
    545, 557, 558, 596, 628, 675, 676, 704, 719, 731, 756,
];

fn clickhouse_analysis_error(error: &str) -> bool {
    clickhouse_error_code(error)
        .is_some_and(|code| CLICKHOUSE_ANALYSIS_ERROR_CODES.contains(&code))
}

/// Definitely analysis-only errors we can recognize without a second round
/// trip. The dry-run fallback handles every other unknown server error.
fn known_clickhouse_rejection(raw_error: &str) -> bool {
    matches!(clickhouse_error_code(raw_error), Some(47 | 62 | 184))
}

/// Only a single read-only ClickHouse statement can be retried after a
/// server-side error.
fn clickhouse_recoverable_error(datasource_type: &str, sql: &str, raw_error: &str) -> bool {
    datasource_type == "clickhouse"
        && clickhouse_read_only_query(sql)
        && clickhouse_server_error(raw_error)
}

/// Best-effort bound for the exploratory `EXPLAIN`. The canonical validation
/// dry run uses 30s; this path runs only after a failed query and must not hold
/// the turn open. A timeout keeps the outcome uncertain.
const QUERY_RECOVERY_DRY_RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A ClickHouse dry-run failure confirms a pre-execution rejection only when it
/// is a parse/analysis error. A successful dry run, a transport failure, or an
/// execution/admission/resource error leaves the outcome uncertain.
fn dry_run_confirms_rejection(
    dry_run: &kyomi_connect_protocol::Result<kyomi_datasource_server::DryRunResult>,
) -> bool {
    match dry_run {
        Ok(result) if !result.valid => {
            let confirmed = clickhouse_analysis_error(&result.message);
            tracing::debug!(
                confirmed,
                message = %result.message,
                "ClickHouse dry run classified for query recovery"
            );
            confirmed
        }
        Ok(_) => false,
        Err(error) => {
            tracing::debug!(%error, "ClickHouse dry run failed; keeping query outcome uncertain");
            false
        }
    }
}

/// Decide whether a model-visible query failure is safe to return to the model.
///
/// For analysis errors not in `known_clickhouse_rejection`, ask the provider
/// for a dry run (`EXPLAIN`): it repeats query analysis without executing
/// anything. A dry-run failure carrying a parse/analysis error proves the
/// original statement never started; a successful dry run or an
/// execution/admission/resource error remains uncertain.
pub(crate) async fn query_error_is_recoverable(
    provider: &dyn kyomi_datasource_server::DatasourceProvider,
    datasource_type: &str,
    sql: &str,
    raw_error: &str,
) -> bool {
    if !clickhouse_recoverable_error(datasource_type, sql, raw_error) {
        return false;
    }
    if known_clickhouse_rejection(raw_error) {
        return true;
    }
    let dry_run = tokio::time::timeout(
        QUERY_RECOVERY_DRY_RUN_TIMEOUT,
        provider.dry_run(sql),
    )
    .await;
    match dry_run {
        Ok(result) => dry_run_confirms_rejection(&result),
        Err(_) => {
            tracing::debug!("ClickHouse dry run timed out; keeping query outcome uncertain");
            false
        }
    }
}

/// Build the model-visible error envelope with an explicit recovery decision.
/// `query_rejected_before_execution` is read by `QueryDatasourceTool`'s
/// `result_domain_outcome`: true means the turn may continue and the model may
/// repair or retry, false means the outcome is uncertain and the run must halt.
pub(crate) fn query_error_response_with_rejection(
    datasource_slug: &str,
    raw_error: &str,
    query_rejected_before_execution: bool,
) -> String {
    serde_json::json!({
        "error": kyomi_core::sanitize_error(raw_error),
        "datasource": datasource_slug,
        "query_rejected_before_execution": query_rejected_before_execution,
    })
    .to_string()
}

#[cfg(test)]
mod query_recovery_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use super::{query_error_is_recoverable, query_error_response_with_rejection};
    use crate::test_support::{build_ctx, seed_user_and_workspace, test_pool};
    use crate::tools::{AgentTool, ToolContext, datasource::QueryDatasourceTool};
    use agent_runtime::DomainOutcome;
    use kyomi_core::connect_protocol::{ConnectOp, ConnectResponse, ConnectResponseBody};
    use kyomi_datasource_server::connect::registry::CommandPayload;
    use kyomi_datasource_server::{
        ConnectRegistry, DatasourceProvider, DryRunResult, QueryResult, QueryStatus,
    };

    type DryRunSlot = std::sync::Mutex<Option<kyomi_connect_protocol::Result<DryRunResult>>>;

    struct DryRunProbe {
        result: DryRunSlot,
        calls: AtomicUsize,
    }

    impl DryRunProbe {
        fn new(result: Option<kyomi_connect_protocol::Result<DryRunResult>>) -> Self {
            Self {
                result: std::sync::Mutex::new(result),
                calls: AtomicUsize::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl DatasourceProvider for DryRunProbe {
        async fn test_connection(&self) -> kyomi_connect_protocol::Result<bool> {
            Ok(true)
        }

        async fn execute_query(
            &self,
            _sql: &str,
            _limit: Option<u32>,
            _offset: Option<u32>,
            _include_total: bool,
            _job_id: Option<&str>,
        ) -> kyomi_connect_protocol::Result<QueryResult> {
            unreachable!("only dry_run is exercised")
        }

        async fn dry_run(&self, _sql: &str) -> kyomi_connect_protocol::Result<DryRunResult> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.result.lock().unwrap().take().expect("one dry run")
        }

        async fn close(&self) {}
    }

    struct HangingProbe;

    #[async_trait::async_trait]
    impl DatasourceProvider for HangingProbe {
        async fn test_connection(&self) -> kyomi_connect_protocol::Result<bool> {
            Ok(true)
        }

        async fn execute_query(
            &self,
            _sql: &str,
            _limit: Option<u32>,
            _offset: Option<u32>,
            _include_total: bool,
            _job_id: Option<&str>,
        ) -> kyomi_connect_protocol::Result<QueryResult> {
            unreachable!("only dry_run is exercised")
        }

        async fn dry_run(&self, _sql: &str) -> kyomi_connect_protocol::Result<DryRunResult> {
            std::future::pending::<kyomi_connect_protocol::Result<DryRunResult>>().await
        }

        async fn close(&self) {}
    }

    struct ConnectFixture {
        query_calls: Arc<AtomicUsize>,
        dry_run_calls: Arc<AtomicUsize>,
        handler: tokio::task::JoinHandle<()>,
        connection_id: u64,
    }

    async fn attach_clickhouse_connect(
        ctx: &mut ToolContext,
        query_error: String,
        dry_run: DryRunResult,
    ) -> ConnectFixture {
        let kyomi_core::DbPool::Sqlite(pool) = &ctx.db else {
            unreachable!("SQLite fixture")
        };
        sqlx::query(
            "INSERT INTO datasource_configs \
             (id, workspace_id, name, datasource_type, connection_config, active, slug, connection_type) \
             VALUES ('ds-clickhouse', 'ws-1', 'Analytics', 'clickhouse', '{}', 1, 'analytics', 'connect')",
        )
        .execute(pool)
        .await
        .expect("seed datasource");
        let registry = ConnectRegistry::new_local();
        let (sender, mut receiver) = tokio::sync::mpsc::channel::<CommandPayload>(16);
        let connection_id = registry.register("ds-clickhouse", sender).await;
        let query_calls = Arc::new(AtomicUsize::new(0));
        let dry_run_calls = Arc::new(AtomicUsize::new(0));
        let observed_query = query_calls.clone();
        let observed_dry_run = dry_run_calls.clone();
        let dry_run_json = serde_json::to_value(dry_run).expect("serialize dry run");
        let handler = tokio::spawn(async move {
            while let Some((request, response)) = receiver.recv().await {
                let body = match &request.op {
                    ConnectOp::ExecuteQuery => {
                        observed_query.fetch_add(1, Ordering::SeqCst);
                        let result = QueryResult {
                            status: QueryStatus::Error,
                            columns: None,
                            total_rows: None,
                            has_more: false,
                            bytes_processed: None,
                            execution_time_ms: Some(1),
                            error: Some(query_error.clone()),
                            record_batch: None,
                            job_id: None,
                        };
                        ConnectResponseBody::Result {
                            result: serde_json::to_value(result).expect("serialize query result"),
                        }
                    }
                    ConnectOp::DryRun => {
                        observed_dry_run.fetch_add(1, Ordering::SeqCst);
                        ConnectResponseBody::Result {
                            result: dry_run_json.clone(),
                        }
                    }
                    _ => panic!("unexpected Connect op"),
                };
                response
                    .send(ConnectResponse { id: request.id, body })
                    .expect("deliver result");
            }
        });
        ctx.connect_registry = Some(registry);
        ConnectFixture {
            query_calls,
            dry_run_calls,
            handler,
            connection_id,
        }
    }

    async fn finish_connect(ctx: &ToolContext, fixture: ConnectFixture) {
        ctx.connect_registry
            .as_ref()
            .expect("registry")
            .unregister("ds-clickhouse", fixture.connection_id)
            .await;
        fixture.handler.await.expect("connect handler");
    }

    async fn query_context() -> ToolContext {
        let db = test_pool().await;
        seed_user_and_workspace(&db).await;
        build_ctx(db)
    }

    fn analysis_error(code: u16) -> String {
        format!("Code: {code}. DB::Exception: analysis failure (test)")
    }

    fn execution_error(code: u16) -> String {
        format!("Code: {code}. DB::Exception: execution failure (test)")
    }

    fn envelope(error: &str, rejected: bool) -> String {
        query_error_response_with_rejection("analytics", error, rejected)
    }

    fn domain(text: &str) -> DomainOutcome {
        QueryDatasourceTool.result_domain_outcome(text)
    }

    #[tokio::test]
    async fn known_clickhouse_rejections_reach_model_without_a_dry_run() {
        for code in [47u16, 62, 184] {
            let error = analysis_error(code);
            for sql in [
                "SELECT page_url FROM events",
                "WITH v AS (SELECT 1) SELECT * FROM v",
                "SELECT page_url FROM events;",
                " \nselect\tpage_url FROM events;  ",
            ] {
                let probe = DryRunProbe::new(None);
                assert!(
                    query_error_is_recoverable(&probe, "clickhouse", sql, &error).await,
                    "{sql}"
                );
                assert_eq!(probe.calls(), 0, "known rejection must not dry-run: {sql}");
                let text = envelope(&error, true);
                assert_eq!(domain(&text), DomainOutcome::Rejected);
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                assert_eq!(value["error"], error);
                assert_eq!(value["datasource"], "analytics");
            }
        }
    }

    #[tokio::test]
    async fn query_recovery_returns_illegal_aggregation_and_cte_rejections_to_model() {
        let error = "Code: 184. DB::Exception: Aggregate function any(sw) is found in GROUP BY in query. (ILLEGAL_AGGREGATION)";
        for sql in [
            "SELECT any(sw) FROM events GROUP BY screen",
            "WITH v AS (SELECT visitor_id, any(screen_width) AS sw FROM events GROUP BY visitor_id)\nSELECT sw FROM v",
            " \nwith v as (select 1) select * from v;  ",
        ] {
            let probe = DryRunProbe::new(None);
            assert!(
                query_error_is_recoverable(&probe, "clickhouse", sql, error).await,
                "{sql}"
            );
            assert_eq!(probe.calls(), 0);
            let text = envelope(error, true);
            assert_eq!(domain(&text), DomainOutcome::Rejected, "{sql}");
        }
    }

    #[tokio::test]
    async fn query_recovery_preserves_uncertainty_for_batches_and_mutations() {
        let identifier = analysis_error(47);
        for (kind, sql, error) in [
            (
                "clickhouse",
                "SELECT 1",
                "timeout after possible commit".to_string(),
            ),
            (
                "clickhouse",
                "SELECT 1",
                "Failed to parse ClickHouse response".to_string(),
            ),
            (
                "clickhouse",
                "SELECT 1",
                "wrapped: Code: 47. DB::Exception: unknown".to_string(),
            ),
            ("postgres", "SELECT 1", identifier.clone()),
            (
                "clickhouse",
                "INSERT INTO t VALUES (1)",
                identifier.clone(),
            ),
            (
                "clickhouse",
                "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
                identifier.clone(),
            ),
            ("clickhouse", "SELECTED page_url", identifier.clone()),
            (
                "clickhouse",
                "INSERT INTO t VALUES (1); SELECT page_url FROM events",
                identifier.clone(),
            ),
            (
                "clickhouse",
                "INSERT INTO t VALUES (1); SELECT page_url FROM events;",
                identifier.clone(),
            ),
            (
                "clickhouse",
                "WITH v AS (SELECT 1) SELECT * FROM v; INSERT INTO t VALUES (1)",
                identifier.clone(),
            ),
        ] {
            let probe = DryRunProbe::new(None);
            assert!(
                !query_error_is_recoverable(&probe, kind, sql, &error).await,
                "{sql}: {error}"
            );
            assert_eq!(probe.calls(), 0, "must not dry-run {sql}: {error}");
            assert_eq!(
                domain(&envelope(&error, false)),
                DomainOutcome::Unknown
            );
        }

        // A mutation that starts with WITH must be rejected even when its
        // error is the 184 fast-path analysis code.
        let illegal_aggregation = analysis_error(184);
        let probe = DryRunProbe::new(None);
        assert!(
            !query_error_is_recoverable(
                &probe,
                "clickhouse",
                "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
                &illegal_aggregation,
            )
            .await
        );
        assert_eq!(probe.calls(), 0);
    }

    #[tokio::test]
    async fn dry_run_confirms_only_analysis_errors() {
        // An unknown analysis code is confirmed by a dry run that repeats it.
        let original = analysis_error(43);
        let probe =
            DryRunProbe::new(Some(Ok(DryRunResult::failure(original.as_str(), None, None))));
        assert!(
            query_error_is_recoverable(&probe, "clickhouse", "SELECT 1", &original).await
        );
        assert_eq!(probe.calls(), 1);

        // The dry run may discover a different analysis error.
        let probe = DryRunProbe::new(Some(Ok(DryRunResult::failure(
            analysis_error(47),
            None,
            None,
        ))));
        assert!(
            query_error_is_recoverable(
                &probe,
                "clickhouse",
                "SELECT 1",
                &analysis_error(43)
            )
            .await
        );
        assert_eq!(probe.calls(), 1);

        // A successful dry run means the original failure was execution-phase.
        let probe = DryRunProbe::new(Some(Ok(DryRunResult::success("Query valid"))));
        assert!(
            !query_error_is_recoverable(
                &probe,
                "clickhouse",
                "SELECT 1",
                &execution_error(159)
            )
            .await
        );
        assert_eq!(probe.calls(), 1);

        // Server-shaped execution, admission and resource dry-run failures are
        // not proof that the original query was rejected before execution.
        for dry_run_code in [159u16, 202, 209, 241, 80] {
            let probe = DryRunProbe::new(Some(Ok(DryRunResult::failure(
                execution_error(dry_run_code),
                None,
                None,
            ))));
            assert!(
                !query_error_is_recoverable(
                    &probe,
                    "clickhouse",
                    "SELECT 1",
                    &execution_error(159)
                )
                .await,
                "dry-run code {dry_run_code} must stay uncertain"
            );
            assert_eq!(probe.calls(), 1);
        }

        // A dry run that discovers an analysis error is still proof even when
        // the original failure looked execution-phase.
        let probe = DryRunProbe::new(Some(Ok(DryRunResult::failure(
            analysis_error(43),
            None,
            None,
        ))));
        assert!(
            query_error_is_recoverable(
                &probe,
                "clickhouse",
                "SELECT 1",
                &execution_error(159)
            )
            .await
        );
        assert_eq!(probe.calls(), 1);

        // A dry-run transport failure is not proof.
        let probe = DryRunProbe::new(Some(Err(
            kyomi_connect_protocol::Error::Internal("connection reset".into())
        )));
        assert!(
            !query_error_is_recoverable(
                &probe,
                "clickhouse",
                "SELECT 1",
                &analysis_error(43)
            )
            .await
        );
        assert_eq!(probe.calls(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn dry_run_timeout_stays_uncertain() {
        assert!(
            !query_error_is_recoverable(
                &HangingProbe,
                "clickhouse",
                "SELECT 1",
                &analysis_error(43)
            )
            .await
        );
    }

    #[tokio::test]
    async fn query_datasource_real_tool_rejects_illegal_aggregation_cte() {
        let mut ctx = query_context().await;
        let error = analysis_error(184);
        let fixture = attach_clickhouse_connect(
            &mut ctx,
            error.clone(),
            DryRunResult::failure(error.as_str(), None, None),
        )
        .await;
        let sql = "WITH v AS (SELECT visitor_id, any(screen_width) AS sw FROM events GROUP BY visitor_id) SELECT sw FROM v";
        let text = QueryDatasourceTool
            .execute(
                serde_json::json!({"sql_query": sql, "datasource": "analytics"}),
                &ctx,
            )
            .await
            .expect("tool result");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["query_rejected_before_execution"], true);
        assert_eq!(domain(&text), DomainOutcome::Rejected);
        assert_eq!(fixture.query_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.dry_run_calls.load(Ordering::SeqCst), 0);
        finish_connect(&ctx, fixture).await;
    }

    #[tokio::test]
    async fn query_datasource_real_tool_confirms_unknown_analysis_error() {
        let mut ctx = query_context().await;
        let error = analysis_error(43);
        let fixture = attach_clickhouse_connect(
            &mut ctx,
            error.clone(),
            DryRunResult::failure(error.as_str(), None, None),
        )
        .await;
        let text = QueryDatasourceTool
            .execute(
                serde_json::json!({"sql_query": "SELECT 1", "datasource": "analytics"}),
                &ctx,
            )
            .await
            .expect("tool result");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["query_rejected_before_execution"], true);
        assert_eq!(domain(&text), DomainOutcome::Rejected);
        assert_eq!(fixture.query_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.dry_run_calls.load(Ordering::SeqCst), 1);
        finish_connect(&ctx, fixture).await;
    }

    #[tokio::test]
    async fn query_datasource_real_tool_keeps_execution_failure_uncertain() {
        let mut ctx = query_context().await;
        let error = execution_error(159);
        let fixture = attach_clickhouse_connect(
            &mut ctx,
            error,
            DryRunResult::success("Query valid"),
        )
        .await;
        let text = QueryDatasourceTool
            .execute(
                serde_json::json!({"sql_query": "SELECT 1", "datasource": "analytics"}),
                &ctx,
            )
            .await
            .expect("tool result");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["query_rejected_before_execution"], false);
        assert_eq!(domain(&text), DomainOutcome::Unknown);
        assert_eq!(fixture.query_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.dry_run_calls.load(Ordering::SeqCst), 1);
        finish_connect(&ctx, fixture).await;
    }

    #[tokio::test]
    async fn query_datasource_real_tool_keeps_with_insert_uncertain() {
        let mut ctx = query_context().await;
        let error = analysis_error(184);
        let fixture = attach_clickhouse_connect(
            &mut ctx,
            error.clone(),
            DryRunResult::failure(error.as_str(), None, None),
        )
        .await;
        let text = QueryDatasourceTool
            .execute(
                serde_json::json!({
                    "sql_query": "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
                    "datasource": "analytics",
                }),
                &ctx,
            )
            .await
            .expect("tool result");
        let value: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["query_rejected_before_execution"], false);
        assert_eq!(domain(&text), DomainOutcome::Unknown);
        assert_eq!(fixture.query_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.dry_run_calls.load(Ordering::SeqCst), 0);
        finish_connect(&ctx, fixture).await;
    }
}

// ---------------------------------------------------------------------------
// Arrow → JSON conversion
// ---------------------------------------------------------------------------

/// Convert an Arrow [`RecordBatch`] to a row-major `Vec<Vec<Value>>`.
///
/// This is the single authoritative place where Arrow columnar data is
/// converted to JSON values for the tool→LLM text boundary. It handles all
/// concrete array types that datasource providers produce. Unknown types fall
/// back to `Value::Null` rather than panicking.
///
/// **Only call this at the tool output boundary.** The Arrow pipeline must
/// stay intact through query execution; JSON conversion belongs here, not
/// inside providers or the query executor.
pub fn record_batch_to_rows(
    batch: &arrow_array::RecordBatch,
) -> Vec<Vec<Value>> {
    let num_rows = batch.num_rows();
    let num_cols = batch.num_columns();
    let mut rows = Vec::with_capacity(num_rows);
    for row_idx in 0..num_rows {
        let mut row = Vec::with_capacity(num_cols);
        for col_idx in 0..num_cols {
            let col = batch.column(col_idx);
            let val = arrow_cell_to_json(col.as_ref(), row_idx);
            row.push(val);
        }
        rows.push(row);
    }
    rows
}

/// Convert a single cell from an Arrow array to a [`serde_json::Value`].
fn arrow_cell_to_json(col: &dyn Array, row_idx: usize) -> Value {
    if col.is_null(row_idx) {
        return Value::Null;
    }
    if let Some(arr) = col.as_any().downcast_ref::<Float64Array>() {
        return serde_json::Number::from_f64(arr.value(row_idx))
            .map(Value::Number)
            .unwrap_or(Value::Null);
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int64Array>() {
        return Value::Number(arr.value(row_idx).into());
    }
    if let Some(arr) = col.as_any().downcast_ref::<Int32Array>() {
        return Value::Number(arr.value(row_idx).into());
    }
    if let Some(arr) = col.as_any().downcast_ref::<UInt64Array>() {
        return Value::Number(arr.value(row_idx).into());
    }
    if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
        return Value::String(arr.value(row_idx).to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<LargeStringArray>() {
        return Value::String(arr.value(row_idx).to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<BooleanArray>() {
        return Value::Bool(arr.value(row_idx));
    }
    if let Some(arr) = col.as_any().downcast_ref::<Date32Array>() {
        // Date32 stores days since Unix epoch (1970-01-01)
        let days = arr.value(row_idx);
        let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).expect("valid epoch");
        let date = epoch + chrono::Duration::days(i64::from(days));
        return Value::String(date.format("%Y-%m-%d").to_string());
    }
    if let Some(arr) = col.as_any().downcast_ref::<TimestampMicrosecondArray>() {
        let micros = arr.value(row_idx);
        let secs = micros.div_euclid(1_000_000);
        let sub_micros = micros.rem_euclid(1_000_000);
        let nsec = (sub_micros * 1_000) as u32;
        if let Some(dt) = chrono::DateTime::from_timestamp(secs, nsec) {
            return Value::String(dt.format("%Y-%m-%dT%H:%M:%S%.6fZ").to_string());
        }
        return Value::Null;
    }
    Value::Null
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum rows returned for chart data queries.
pub const CHART_QUERY_MAX_ROWS: u32 = 5000;

// ---------------------------------------------------------------------------
// Provider creation
// ---------------------------------------------------------------------------

/// Create a [`DatasourceProvider`] for a datasource, handling both direct and
/// Connect connection types.
///
/// For `connection_type == "connect"`: creates a [`ConnectProvider`] that routes
/// queries through the Kyomi Connect WebSocket agent.
///
/// For `connection_type == "direct"` (or any other value): resolves credentials,
/// then creates a direct provider via the datasource factory.
///
/// This is the single source of truth for provider creation in agent tools.
/// All agent tool code should use this instead of calling
/// `kyomi_datasource_server::factory::create_provider()` directly.
pub use kyomi_auth::chartml_validation::create_provider_for_datasource;

// ---------------------------------------------------------------------------
// Query execution
// ---------------------------------------------------------------------------

/// Result of executing a datasource query: column names + rows as dicts.
pub struct QueryRows {
    /// Column names in order.
    pub columns: Vec<String>,
    /// Each row as a `{column_name: value}` dict.
    pub rows: Vec<serde_json::Map<String, Value>>,
}

/// Execute a SQL query against a datasource and return structured results.
///
/// Handles the full lifecycle: resolve datasource → decrypt credentials →
/// create provider → execute → close provider.
///
/// # Errors
///
/// Returns a user-facing error string (not a `kyomi_core::Error`) so callers
/// can include it directly in tool responses.
pub async fn execute_datasource_query(
    ctx: &QueryContext,
    datasource_slug: &str,
    sql: &str,
    max_rows: Option<u32>,
) -> Result<QueryRows, String> {
    // 1. Resolve datasource
    let ds = kyomi_auth::datasource_service::resolve_datasource(
        &ctx.db,
        datasource_slug,
        &ctx.workspace_id,
        false,
    )
    .await
    .map_err(|e| format!("Failed to resolve datasource '{datasource_slug}': {e}"))?;

    // 2. Create provider (handles both direct and Connect datasources)
    let provider = create_provider_for_datasource(ctx, &ds).await?;

    // 3. Execute query
    let limit = max_rows.unwrap_or(CHART_QUERY_MAX_ROWS);
    let result = provider
        .execute_query(sql, Some(limit), None, false, None)
        .await
        .map_err(|e| {
            tracing::warn!(raw_error = %e, "datasource query error (sanitized for caller)");
            format!("Query execution failed: {}", kyomi_core::sanitize_error(&e.to_string()))
        })?;
    provider.close().await;

    // 4. Check status
    match result.status {
        kyomi_datasource_server::provider::QueryStatus::Error => {
            let msg = result.error.unwrap_or_else(|| "Unknown error".into());
            tracing::warn!(raw_error = %msg, "datasource query status error (sanitized for caller)");
            return Err(format!("Query failed: {}", kyomi_core::sanitize_error(&msg)));
        }
        kyomi_datasource_server::provider::QueryStatus::Success => {}
    }

    // 5. Convert to column names + dict rows
    let columns = result.columns.unwrap_or_default();
    let col_names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();

    let positional_rows = result
        .record_batch
        .as_ref()
        .map(record_batch_to_rows)
        .unwrap_or_default();

    let dict_rows: Vec<serde_json::Map<String, Value>> = positional_rows
        .into_iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            for (col_name, value) in col_names.iter().zip(row) {
                map.insert(col_name.clone(), value);
            }
            map
        })
        .collect();

    tracing::debug!(
        datasource = %datasource_slug,
        rows = dict_rows.len(),
        cols = col_names.len(),
        "Query executed successfully"
    );

    Ok(QueryRows {
        columns: col_names,
        rows: dict_rows,
    })
}

// ---------------------------------------------------------------------------
// ChartML SQL validation
// ---------------------------------------------------------------------------

/// Extract `(block_number, sql, datasource_slug)` triples from ChartML blocks
/// in markdown content. Only returns entries that have both `data.query` and
/// `data.datasource`.
pub fn extract_chartml_queries(text: &str) -> Vec<(usize, String, String)> {
    kyomi_core::chartml_validation::markdown_blocks(text).iter().enumerate()
        .filter_map(|(i, b)| kyomi_core::chartml_validation::validate_block(b, i + 1).ok().map(|v| (i + 1, v)))
        .flat_map(|(i, v)| kyomi_core::chartml_validation::sql_sources(&v, i))
        .filter_map(|s| s.datasource.map(|d| (s.block, s.sql, d))).collect()
}

/// Per-block SQL dry-run validation errors.
///
/// The primitive [`validate_chartml_sql`] (aggregate) wraps. Each entry's
/// `usize` is the block's **0-based position** in `chartml_re()`'s
/// capture-iteration order — `extract_chartml_queries`'s 1-based
/// `block_number` minus one. This is the same 0-based indexing contract
/// `agent.rs`'s `chartml_block_errors` and `strip_chartml_blocks` use; the
/// `Block N` text embedded in each message stays 1-based for readability,
/// but the index used for stripping is always 0-based. A block that passes
/// validation (or has no SQL source) contributes no entry.
///
/// All unavailable or failed dry-runs produce an error, including credential and connection failures.
pub async fn chartml_sql_block_errors(
    ctx: &QueryContext,
    content: &str,
) -> Vec<(usize, String)> {
    validate_chartml_complete(ctx, &kyomi_core::chartml_validation::markdown_blocks(content)).await
        .into_iter().map(|e| (e.block - 1, e.to_string())).collect()
}

/// Validate all SQL queries inside ChartML blocks via dry-run.
///
/// Returns `None` if all queries are valid (or there are no queries).
/// Returns `Some(error_message)` with details of any invalid SQL. Thin
/// aggregate wrapper over [`chartml_sql_block_errors`] — see that function
/// for the per-block primitive.
pub async fn validate_chartml_sql(
    ctx: &QueryContext,
    content: &str,
) -> Option<String> {
    let errors = chartml_sql_block_errors(ctx, content).await;
    if errors.is_empty() {
        None
    } else {
        let message = errors.iter().map(|(_, msg)| msg.as_str()).collect::<Vec<_>>().join("; ");
        Some(message)
    }
}

pub async fn validate_chartml_complete(ctx: &QueryContext, blocks: &[&str]) -> Vec<kyomi_core::chartml_validation::Diagnostic> {
    kyomi_core::chartml_validation::validate_blocks(blocks, |source| async move {
        kyomi_auth::chartml_validation::dry_run_datasource_query_detailed(ctx, source.datasource.as_deref().expect("orchestrator checked datasource"), &source.sql).await
    }).await
}

// ---------------------------------------------------------------------------
// Single-query dry-run
// ---------------------------------------------------------------------------

/// Dry-run a SQL query against a datasource to validate syntax.
///
/// Returns `Ok(())` on success or a user-facing error string on failure.
pub use kyomi_auth::chartml_validation::dry_run_datasource_query;

pub(super) use kyomi_auth::chartml_validation::log_dry_run_issue;

/// Preserve the existing internal-error classification while removing
/// connection details from transport errors returned by agent query tools.
pub(super) fn sanitize_query_transport_error(
    error: kyomi_connect_protocol::Error,
) -> kyomi_core::Error {
    tracing::warn!(error = %error, "Datasource query transport failed");
    kyomi_core::Error::Internal(kyomi_core::sanitize_error(&error.to_string()))
}

/// Sanitize both provider validation messages and transport errors before
/// either can reach an agent tool result or its caller.
pub(super) use kyomi_auth::chartml_validation::sanitize_dry_run_result;

#[cfg(test)]
mod dry_run_redaction_tests {
    use super::{sanitize_dry_run_result, sanitize_query_transport_error};

    const URL: &str = "http://clickhouse.example:8123/?database=analytics&password=secret123";

    #[test]
    fn sanitizes_provider_validation_message() {
        let driver = kyomi_datasource_server::DryRunResult::failure(
            format!("ClickHouse request failed for {URL}"),
            Some(2),
            Some(4),
        );
        let result = sanitize_dry_run_result(Ok(driver)).expect("provider returned a result");
        assert!(!result.valid);
        assert_eq!((result.line, result.column), (Some(2), Some(4)));
        assert!(result.message.contains("[connection details redacted]"));
        assert!(!result.message.contains("secret123"));
        assert!(!result.message.contains("password="));
    }

    #[test]
    fn sanitizes_provider_error() {
        let error = kyomi_connect_protocol::Error::Provider(format!(
            "ClickHouse request failed for {URL}"
        ));
        let message = sanitize_dry_run_result(Err(error)).expect_err("provider failed");
        assert!(message.starts_with("Dry-run failed:"));
        assert!(message.contains("[connection details redacted]"));
        assert!(!message.contains("secret123"));
        assert!(!message.contains("password="));
    }

    #[test]
    fn sanitizes_query_transport_error() {
        let error = kyomi_connect_protocol::Error::Provider(format!(
            "ClickHouse request failed for {URL}"
        ));
        let output = sanitize_query_transport_error(error).to_string();
        assert!(output.contains("[connection details redacted]"));
        assert!(!output.contains("secret123"));
        assert!(!output.contains("password="));
    }
}
