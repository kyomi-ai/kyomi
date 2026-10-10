// SPDX-License-Identifier: AGPL-3.0-or-later

//! Database-discovered web chat execution. Local tasks are execution aids; acceptance,
//! ownership, cancellation and terminal outcomes remain in the shared database.
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use agent_runtime::{ConversationId, Payload, PublicPayload, RunId, RunState};
use kyomi_auth::conversation_events::{ConversationStore, QueuedRun, discover_queue};
use kyomi_auth::websocket::WebSocketManager;
use kyomi_core::{DbPool, KVPool};
use kyomi_embed::LazyEmbedding;
use tokio_util::sync::CancellationToken;

use crate::adapter::DurableRun;
use crate::{
    AgentExecutionConfig, AgentExecutionEnv, AssistantMessagePersistence, UserMessagePersistence,
};

const LEASE_MS: i64 = 30_000;
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const MAX_ACTIVE: usize = 32;

#[derive(Clone)]
pub struct DurableChatWorker {
    pub db: DbPool,
    pub kv: KVPool,
    pub encryption_key: Arc<[u8; 32]>,
    pub embedding: LazyEmbedding,
    pub ws_manager: WebSocketManager,
    pub app_config: Arc<kyomi_core::Config>,
    pub connect_registry: Option<kyomi_datasource_server::ConnectRegistry>,
    pub platforms: Arc<kyomi_core::platform::PlatformRegistry>,
    pub owner: String,
    pub shutdown: CancellationToken,
}

impl DurableChatWorker {
    /// Scan independently of request handlers, including after a crash between submit
    /// and dispatch. Bound concurrent claims; each conversation is serialized by SQL.
    pub fn start(mut self) -> tokio::task::JoinHandle<()> {
        self.owner = format!("{}:{}", self.owner, uuid::Uuid::new_v4());
        tokio::spawn(async move {
            let mut tasks = tokio::task::JoinSet::new();
            let mut active = HashSet::new();
            let mut scan = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    _ = self.shutdown.cancelled() => break,
                    Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                        match result {
                            Ok(id) => { active.remove(&id); }
                            Err(error) => {
                                active.clear();
                                tracing::error!(%error, "Durable chat task panicked; lease reconciliation will interrupt it");
                            }
                        }
                    }
                    _ = scan.tick() => {
                        let rows = match discover_queue(&self.db, agent_runtime::MAX_REPLAY_EVENTS).await {
                            Ok(rows) => rows,
                            Err(error) => {
                                tracing::error!(%error, "Durable chat discovery failed; accepted jobs remain queued");
                                continue;
                            }
                        };
                        for queued in rows {
                            if tasks.len() >= MAX_ACTIVE { break; }
                            if !active.insert(queued.run_id.clone()) { continue; }
                            let worker = self.clone();
                            tasks.spawn(async move {
                                let id = queued.run_id.clone();
                                if let Err(error) = worker.execute(queued).await {
                                    tracing::error!(run_id = %id, %error, "Durable chat execution stopped");
                                }
                                id
                            });
                        }
                    }
                }
            }
            // Do not finalize shutdown as a model error. Dropping awaits releases local
            // resources; the expiring durable lease produces an interrupted outcome.
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        })
    }

    async fn execute(&self, queued: QueuedRun) -> kyomi_core::Result<()> {
        let conversation = ConversationId(queued.conversation_id);
        let run_id = RunId(queued.run_id);
        let store = ConversationStore::new(
            &self.db,
            &self.encryption_key,
            &queued.actor_id,
            &queued.workspace_id,
        );
        let unavailable = |error: kyomi_auth::conversation_events::EventStoreError| {
            kyomi_core::Error::ServiceUnavailable(error.to_string())
        };
        if queued.expired {
            store
                .expire(&conversation, &run_id, now())
                .await
                .map_err(unavailable)?;
            return Ok(());
        }
        let claimed = match store
            .claim(&conversation, &run_id, &self.owner, now(), LEASE_MS)
            .await
        {
            Ok(claimed) => claimed,
            // Another scanner may have claimed this row or its preceding turn.
            Err(kyomi_auth::conversation_events::EventStoreError::Lifecycle(
                agent_runtime::LifecycleError::NotClaimable
                | agent_runtime::LifecycleError::ConversationBusy,
            )) => return Ok(()),
            Err(error) => return Err(unavailable(error)),
        };
        let lease = claimed
            .snapshot
            .lease
            .ok_or_else(|| kyomi_core::Error::Internal("Claim returned no lease".into()))?;
        let cancel = CancellationToken::new();
        let claimed_context = Arc::new(claimed.context);
        let submitted = &claimed.submission;
        let Payload::Public(PublicPayload::Submitted { message_id, text }) =
            &submitted.submitted.payload
        else {
            return Err(kyomi_core::Error::Internal(
                "Invalid stored submission".into(),
            ));
        };
        let execution = async {
            let membership = kyomi_auth::user_service::get_workspace_user(
                &self.db,
                &queued.workspace_id,
                &queued.actor_id,
            )
            .await?
            .ok_or_else(|| {
                kyomi_core::Error::Forbidden("Workspace membership is no longer active".into())
            })?;
            let config = AgentExecutionConfig {
                is_shared_conversation: claimed_context.shared,
                session_id: conversation.0.clone(),
                user_id: queued.actor_id.clone(),
                workspace_id: queued.workspace_id.clone(),
                message: submitted
                    .submitted
                    .detail
                    .clone()
                    .unwrap_or_else(|| text.preview.clone()),
                model_name: submitted
                    .context
                    .get("model_name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                workspace_roles: vec![membership.role],
                user_display_name: submitted
                    .context
                    .get("user_display_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&queued.actor_id)
                    .to_string(),
                current_time_user_tz: submitted
                    .context
                    .get("current_time_user_tz")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                message_source: Some(submitted.actor.source.clone()),
                cancel_token: cancel.clone(),
                max_iterations: 50,
                max_tokens: 8192,
                user_message_persistence: UserMessagePersistence::CallerPersisted(
                    message_id.0.clone(),
                ),
                assistant_message_persistence: AssistantMessagePersistence::Durable {
                    message_id: submitted.assistant_message_id.0.clone(),
                    run: DurableRun {
                        conversation_id: conversation.clone(),
                        run_id: run_id.clone(),
                        lease: lease.clone(),
                        context: claimed_context.clone(),
                    },
                },
                ..AgentExecutionConfig::default()
            };
            crate::execute_agent_chat(
                config,
                AgentExecutionEnv {
                    db: &self.db,
                    kv: &self.kv,
                    encryption_key: &self.encryption_key,
                    embedding: &self.embedding,
                    ws_manager: &self.ws_manager,
                    app_config: &self.app_config,
                    connect_registry: self.connect_registry.clone(),
                    platforms: self.platforms.clone(),
                },
            )
            .await
        };
        let owned = DurableRun {
            conversation_id: conversation.clone(),
            run_id: run_id.clone(),
            lease: lease.clone(),
            context: claimed_context.clone(),
        };
        let (result, lease) = drive_execution(
            &store,
            &owned,
            &cancel,
            execution,
            LeaseTiming {
                ttl_ms: LEASE_MS,
                heartbeat: HEARTBEAT_INTERVAL,
            },
        )
        .await?;
        let (state, content, metadata, model, usage) = match result {
            Ok(result) => {
                let (state, content) = terminal_outcome(&result.status, &result.response_text);
                let content = content.to_string();
                let metadata = serde_json::json!({"model": result.model, "thinking_events": result.thinking_events, "token_usage": result.token_usage, "component": "custom_agent", "error": result.error});
                (
                    state,
                    content,
                    Some(metadata),
                    result.model,
                    result.token_usage,
                )
            }
            Err(error) => (
                RunState::Failed,
                format!("Error: {}", error.user_message()),
                None,
                None,
                None,
            ),
        };
        let terminal = store
            .finish_with_metadata(
                &conversation,
                &run_id,
                &lease,
                now(),
                (state, &content),
                metadata.as_ref(),
            )
            .await
            .map_err(unavailable)?;
        // Cancellation and completion may race; the committed winner determines delivery.
        if terminal.state == RunState::Cancelled {
            kyomi_auth::websocket::helpers::send_request_cancelled(
                self.ws_manager.for_workspace(&queued.workspace_id),
                &queued.actor_id,
                conversation.as_str(),
                terminal.assistant_message_id.as_str(),
                Some("chat"),
            )
            .await;
        } else {
            crate::deliver_response(
                &self.ws_manager,
                &queued.actor_id,
                conversation.as_str(),
                terminal.assistant_message_id.as_str(),
                &content,
                model.as_deref().unwrap_or("unknown"),
                usage,
                "chat",
                &queued.workspace_id,
                None,
            )
            .await;
            if kyomi_auth::chat_service::get_session(&self.db, conversation.as_str())
                .await?
                .is_some_and(|session| session.shared)
            {
                kyomi_auth::websocket::helpers::send_shared_chat_message(
                    &self.ws_manager,
                    &queued.workspace_id,
                    conversation.as_str(),
                    terminal.assistant_message_id.as_str(),
                    "assistant",
                    &content,
                    &chrono::Utc::now().to_rfc3339(),
                    None,
                    None,
                    None,
                )
                .await;
            }
        }
        Ok(())
    }
}

/// The shared database chooses cancellation. Legacy executor text matching may
/// classify an upstream error as cancelled, so it cannot set durable authority.
pub(crate) fn terminal_outcome<'a>(status: &str, response: &'a str) -> (RunState, &'a str) {
    match status {
        "completed" => (RunState::Completed, response),
        "cancelled" => (
            RunState::Failed,
            "Execution stopped before completing a response.",
        ),
        _ => (RunState::Failed, response),
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

struct LeaseTiming {
    ttl_ms: i64,
    heartbeat: Duration,
}

/// Drive ownership checks alongside the executor, rather than from its iteration
/// boundaries: a single pending provider or tool await must not let the lease expire.
async fn drive_execution<F, T>(
    store: &ConversationStore<'_>,
    owned: &DurableRun,
    cancel: &CancellationToken,
    execution: F,
    timing: LeaseTiming,
) -> kyomi_core::Result<(kyomi_core::Result<T>, agent_runtime::Lease)>
where
    F: std::future::Future<Output = kyomi_core::Result<T>>,
{
    tokio::pin!(execution);
    let mut lease = owned.lease.clone();
    let mut heartbeat = tokio::time::interval(timing.heartbeat);
    loop {
        // Poll execution while waiting for the timer and while renewing SQL.
        // The executor may hold the only SQLite connection; it must keep
        // progressing until renewal can acquire it.
        tokio::select! {
            result = &mut execution => return Ok((result, lease)),
            _ = heartbeat.tick() => {},
        }
        let (renewed, execution_result) = {
            let renewal = store.heartbeat(
                &owned.conversation_id,
                &owned.run_id,
                &lease,
                now(),
                timing.ttl_ms,
            );
            tokio::pin!(renewal);
            let mut execution_result = None;
            let renewed = tokio::select! {
                result = &mut execution => {
                    execution_result = Some(result);
                    // Complete an already-started transaction before finalization.
                    // Dropping it mid-SQL can leave SQLite's queued rollback racing
                    // the next transaction on the same connection.
                    renewal.await
                },
                renewed = &mut renewal => renewed,
            };
            (renewed, execution_result)
        };
        let snapshot = match renewed {
            Ok(snapshot) => snapshot,
            Err(error) => {
                cancel.cancel();
                return Err(kyomi_core::Error::ServiceUnavailable(error.to_string()));
            }
        };
        lease = snapshot
            .lease
            .ok_or_else(|| kyomi_core::Error::Internal("Heartbeat returned no lease".into()))?;
        if snapshot.cancellation_requested {
            cancel.cancel();
            // Drop even pre-loop awaits (such as embedding initialization)
            // that do not observe the agent's local cancellation token.
            return Ok((
                Err(kyomi_core::Error::BadRequest("Request cancelled".into())),
                lease,
            ));
        }
        if let Some(result) = execution_result {
            return Ok((result, lease));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn submitted(db: &DbPool, key: &[u8; 32]) -> (ConversationId, RunId) {
        let conversation = uuid::Uuid::new_v4().to_string();
        let accepted = kyomi_auth::chat_service::prepare_chat_dispatch(
            kyomi_auth::chat_service::ChatDispatchParams {
                db,
                encryption_key: key,
                ws_manager: None,
                user_id: "user-a",
                workspace_id: "ws-1",
                user_display_name: "Test",
                session_id: &conversation,
                is_new_session: true,
                message: "hello",
                current_time_user_tz: None,
                message_source: Some("web"),
                skip_ai: false,
                client_msg_id: Some("heartbeat-request"),
                owner_instance: "test",
                execution_context: None,
            },
        )
        .await
        .expect("submit");
        let kyomi_auth::chat_service::ChatDispatchOutcome::Ready { run_id, .. } = accepted else {
            panic!("ready");
        };
        let conversation_id = ConversationId(conversation);
        let run_id = RunId(run_id);
        (conversation_id, run_id)
    }

    async fn claimed(db: &DbPool, key: &[u8; 32]) -> DurableRun {
        let (conversation_id, run_id) = submitted(db, key).await;
        let result = ConversationStore::new(db, key, "user-a", "ws-1")
            .claim(&conversation_id, &run_id, "worker", now(), 1_000)
            .await
            .expect("claim");
        DurableRun {
            conversation_id,
            run_id,
            lease: result.snapshot.lease.expect("lease"),
            context: Arc::new(result.context),
        }
    }

    #[tokio::test]
    async fn startup_worker_discovers_accepted_work_and_cancels_a_preloop_await() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = Arc::new([7u8; 32]);
        let (conversation, run) = submitted(&db, &key).await;
        let discovered = discover_queue(&db, agent_runtime::MAX_REPLAY_EVENTS)
            .await
            .expect("discover accepted queue");
        assert_eq!(discovered.len(), 1);
        assert!(
            !discovered[0].expired,
            "active accepted initiator remains claimable"
        );
        // No HTTP task exists: only the committed database queue remains.
        let context = crate::test_support::build_ctx(db.clone());
        let shutdown = CancellationToken::new();
        let worker = DurableChatWorker {
            db: db.clone(),
            kv: context.kv,
            encryption_key: key.clone(),
            // An intentionally unloaded embedding blocks before any provider/network
            // call, exercising cancellation before the legacy agent loop starts.
            embedding: context.embedding,
            ws_manager: context.ws_manager,
            app_config: context.config,
            connect_registry: None,
            platforms: context.platforms,
            owner: "restarted-process".into(),
            shutdown: shutdown.clone(),
        }
        .start();
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let state = kyomi_core::db_fetch_scalar!(
                    &db,
                    String,
                    "SELECT state FROM conversation_runs WHERE run_id=$1",
                    run.as_str()
                )
                .expect("run state");
                if state == "running" {
                    break;
                }
                if state != "queued" {
                    let messages = kyomi_auth::chat_service::get_session_messages(
                        &db,
                        &key,
                        conversation.as_str(),
                        100,
                    )
                    .await
                    .expect("diagnose early terminal result");
                    panic!("accepted job terminated before pre-loop await: {state}: {messages:?}");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("startup scanner discovers accepted queue");
        ConversationStore::new(&db, &key, "user-a", "ws-1")
            .cancel(&conversation, &run, now())
            .await
            .expect("another replica cancels by durable run id");
        tokio::time::timeout(Duration::from_secs(6), async {
            loop {
                let state = kyomi_core::db_fetch_scalar!(
                    &db,
                    String,
                    "SELECT state FROM conversation_runs WHERE run_id=$1",
                    run.as_str()
                )
                .expect("run state");
                if state == "cancelled" {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("durable cancellation stops embedding wait without a loaded model");
        shutdown.cancel();
        worker.await.expect("worker shutdown completes");
        let messages =
            kyomi_auth::chat_service::get_session_messages(&db, &key, conversation.as_str(), 100)
                .await
                .expect("terminal projection");
        let assistants = messages
            .iter()
            .filter(|message| message.message_type == "assistant")
            .collect::<Vec<_>>();
        assert_eq!(assistants.len(), 1);
        assert_eq!(assistants[0].status, "cancelled");
    }

    #[tokio::test]
    async fn completion_drains_an_in_flight_heartbeat_before_finalization() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = [7u8; 32];
        let owned = claimed(&db, &key).await;
        let store = ConversationStore::new(&db, &key, "user-a", "ws-1");
        let DbPool::Sqlite(pool) = &db else {
            panic!("SQLite fixture");
        };
        // claim() returns its connection asynchronously. Wait for that return
        // before observing the heartbeat's idle-to-busy transition, otherwise
        // the executor can mistake fixture cleanup for an in-flight renewal.
        tokio::time::timeout(Duration::from_secs(2), async {
            while pool.num_idle() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("claim connection returned before heartbeat observation");
        let execution = async {
            tokio::time::timeout(Duration::from_secs(2), async {
                // The executor completes precisely while renewal owns the sole
                // connection, instead of relying on a sleep to hit the race.
                loop {
                    if pool.num_idle() == 0 {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("heartbeat acquired the SQLite connection");
            Ok("complete")
        };
        let (result, lease) = drive_execution(
            &store,
            &owned,
            &CancellationToken::new(),
            execution,
            LeaseTiming {
                ttl_ms: 2_000,
                heartbeat: Duration::from_secs(10),
            },
        )
        .await
        .expect("completion drains renewal");
        assert_eq!(result.unwrap(), "complete");
        assert!(
            lease.expires_at > owned.lease.expires_at,
            "in-flight renewal was discarded"
        );
        store
            .finish(
                &owned.conversation_id,
                &owned.run_id,
                &lease,
                now(),
                RunState::Completed,
                "complete",
            )
            .await
            .expect("finalization follows the completed heartbeat transaction");
    }

    #[tokio::test]
    async fn heartbeat_remains_live_during_one_long_executor_await() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = [7u8; 32];
        let owned = claimed(&db, &key).await;
        let store = ConversationStore::new(&db, &key, "user-a", "ws-1");
        let original_expiry = owned.lease.expires_at;
        let (result, lease) = drive_execution(
            &store,
            &owned,
            &CancellationToken::new(),
            async {
                tokio::time::sleep(Duration::from_millis(1_100)).await;
                Ok("complete")
            },
            LeaseTiming {
                ttl_ms: 1_000,
                heartbeat: Duration::from_millis(10),
            },
        )
        .await
        .expect("heartbeat survives a provider wait longer than original lease");
        assert_eq!(result.expect("executor result"), "complete");
        assert!(lease.expires_at > original_expiry);
        store
            .finish(
                &owned.conversation_id,
                &owned.run_id,
                &lease,
                now(),
                RunState::Completed,
                "complete",
            )
            .await
            .expect("current owner can finalize");
    }

    #[tokio::test]
    async fn lost_ownership_stops_a_pending_executor_before_it_can_finish() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = [7u8; 32];
        let owned = claimed(&db, &key).await;
        let store = ConversationStore::new(&db, &key, "user-a", "ws-1");
        store
            .expire(
                &owned.conversation_id,
                &owned.run_id,
                owned.lease.expires_at,
            )
            .await
            .expect("replacement recovery expires lease");
        let cancel = CancellationToken::new();
        let execution = std::future::pending::<kyomi_core::Result<()>>();
        assert!(
            drive_execution(
                &store,
                &owned,
                &cancel,
                execution,
                LeaseTiming {
                    ttl_ms: 1_000,
                    heartbeat: Duration::from_millis(10),
                }
            )
            .await
            .is_err()
        );
        assert!(
            cancel.is_cancelled(),
            "local execution aid is stopped when ownership cannot be verified"
        );
    }

    #[tokio::test]
    async fn heartbeat_observes_cancellation_from_another_api_replica() {
        let db = crate::test_support::test_pool().await;
        crate::test_support::seed_user_and_workspace(&db).await;
        let key = [7u8; 32];
        let owned = claimed(&db, &key).await;
        let store = ConversationStore::new(&db, &key, "user-a", "ws-1");
        let cancel = CancellationToken::new();
        let second_replica = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            ConversationStore::new(&db, &key, "user-a", "ws-1")
                .cancel(&owned.conversation_id, &owned.run_id, now())
                .await
                .expect("other replica accepts cancellation");
        };
        let executor = async {
            cancel.cancelled().await;
            Ok("cancelled")
        };
        let (execution, ()) = tokio::time::timeout(Duration::from_secs(3), async {
            tokio::join!(
                drive_execution(
                    &store,
                    &owned,
                    &cancel,
                    executor,
                    LeaseTiming {
                        ttl_ms: 1_000,
                        heartbeat: Duration::from_millis(10),
                    }
                ),
                second_replica,
            )
        })
        .await
        .expect("durable cancellation must be observed during the provider await");
        let (_, lease) = execution.expect("owner observes durable cancel");
        let terminal = store
            .finish(
                &owned.conversation_id,
                &owned.run_id,
                &lease,
                now(),
                RunState::Completed,
                "late result",
            )
            .await
            .expect("commit cancellation winner");
        assert_eq!(terminal.state, RunState::Cancelled);
    }
}
