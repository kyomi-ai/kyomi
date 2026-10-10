//! One connection owns its read subscription and bounded unacknowledged page.
//! Redis is a wake-up hint; periodic reads recover silent notification loss.
use agent_runtime::{
    ConversationId, ConversationReadRequest, ConversationReadResponse as Response, DetailId,
    ReadErrorCode, ReadIdentity, RunId,
};
use kyomi_auth::conversation_events::{ConversationStore, EventStoreError};
use serde::Deserialize;

const DELIVERY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub(super) const CATCH_UP_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ReadCommand {
    #[serde(rename = "conversation_subscribe")]
    Subscribe {
        #[serde(flatten)]
        request: ConversationReadRequest,
    },
    #[serde(rename = "conversation_snapshot")]
    Snapshot {
        #[serde(flatten)]
        request: ConversationReadRequest,
    },
    #[serde(rename = "conversation_replay")]
    Replay {
        #[serde(flatten)]
        request: ConversationReadRequest,
    },
    #[serde(rename = "conversation_ack")]
    Ack {
        connection_generation: String,
        request_generation: u64,
        through_cursor: i64,
    },
    #[serde(rename = "conversation_unsubscribe")]
    Unsubscribe { request_generation: u64 },
    #[serde(rename = "conversation_detail")]
    Detail {
        session_id: String,
        #[serde(default)]
        run_id: Option<RunId>,
        detail_id: DetailId,
        request_generation: u64,
    },
}
struct Subscription {
    request: ConversationReadRequest,
    cursor: Option<i64>,
    snapshot_through: Option<i64>,
    pending: Option<i64>,
    live: bool,
}
pub(super) struct ConnectionReads {
    generation: String,
    newest: Option<u64>,
    subscription: Option<Subscription>,
}
impl ConnectionReads {
    pub fn new(generation: String) -> Self {
        Self {
            generation,
            newest: None,
            subscription: None,
        }
    }
    fn identity(&self, request: u64) -> ReadIdentity {
        ReadIdentity {
            connection_generation: self.generation.clone(),
            request_generation: request,
        }
    }
    pub async fn command(
        &mut self,
        command: ReadCommand,
        store: &ConversationStore<'_>,
    ) -> Option<(ConversationId, Response)> {
        match command {
            ReadCommand::Subscribe { request } => self.start(request, true, store).await,
            ReadCommand::Snapshot { mut request } => {
                request.after = None;
                self.start(request, false, store).await
            }
            ReadCommand::Replay { request } => self.start(request, false, store).await,
            ReadCommand::Ack {
                connection_generation,
                request_generation,
                through_cursor,
            } => {
                if connection_generation != self.generation {
                    return None;
                }
                let sub = self.subscription.as_mut()?;
                if sub.request.request_generation != request_generation
                    || sub.pending != Some(through_cursor)
                {
                    return None;
                }
                sub.pending = None;
                sub.cursor = Some(through_cursor);
                if sub.snapshot_through == Some(through_cursor) {
                    sub.snapshot_through = None;
                }
                if !sub.live && sub.snapshot_through.is_none() {
                    self.subscription = None;
                    return None;
                }
                self.catch_up(store).await
            }
            ReadCommand::Unsubscribe { request_generation } => {
                if self
                    .subscription
                    .as_ref()
                    .is_some_and(|sub| sub.request.request_generation == request_generation)
                {
                    self.subscription = None;
                }
                None
            }
            ReadCommand::Detail {
                session_id,
                run_id,
                detail_id,
                request_generation,
            } => {
                let conversation = ConversationId(session_id);
                let identity = self.identity(request_generation);
                let response = match store
                    .read_detail(&conversation, run_id.as_ref(), &detail_id)
                    .await
                {
                    Ok(text) => Response::Detail {
                        identity,
                        detail_id,
                        text,
                    },
                    Err(error) => error_response(identity, error),
                };
                Some((conversation, response))
            }
        }
    }
    async fn start(
        &mut self,
        request: ConversationReadRequest,
        live: bool,
        store: &ConversationStore<'_>,
    ) -> Option<(ConversationId, Response)> {
        if self
            .newest
            .is_some_and(|generation| request.request_generation <= generation)
        {
            return None;
        }
        self.newest = Some(request.request_generation);
        // Install before capturing the DB snapshot. Events committed while capture
        // blocks are found by catch-up after the snapshot watermark is acknowledged.
        self.subscription = Some(Subscription {
            cursor: request.after,
            request,
            snapshot_through: None,
            pending: None,
            live,
        });
        self.catch_up(store).await
    }
    pub async fn catch_up(
        &mut self,
        store: &ConversationStore<'_>,
    ) -> Option<(ConversationId, Response)> {
        let sub = self.subscription.as_ref()?;
        let conversation = ConversationId(sub.request.session_id.clone());
        let identity = self.identity(sub.request.request_generation);
        // Even stalled clients lose access promptly. Do not read another page while
        // application acknowledgement is outstanding (one page staging maximum).
        if let Err(error) = store.authorize_read(&conversation).await {
            self.subscription = None;
            return Some((conversation, error_response(identity, error)));
        }
        let sub = self.subscription.as_mut()?;
        if sub.pending.is_some() {
            return None;
        }
        let result = if let Some(cursor) = sub.cursor {
            store
                .read_replay(
                    &conversation,
                    sub.request.run_id.as_ref(),
                    cursor,
                    sub.snapshot_through,
                    sub.request.limit,
                    sub.request.byte_limit,
                )
                .await
                .map(|page| Response::Replay {
                    identity: identity.clone(),
                    conversation_id: conversation.clone(),
                    page,
                })
        } else {
            store
                .snapshot(
                    &conversation,
                    sub.request.run_id.as_ref(),
                    sub.request.limit,
                    sub.request.byte_limit,
                )
                .await
                .map(|snapshot| Response::Snapshot {
                    identity: identity.clone(),
                    snapshot,
                })
        };
        let response = match result {
            Ok(response) => {
                match &response {
                    Response::Snapshot { snapshot, .. } => {
                        sub.snapshot_through = Some(snapshot.through_cursor);
                        sub.pending = Some(snapshot.page.through_cursor);
                    }
                    Response::Replay { page, .. } => {
                        if page.through_cursor == page.from_cursor && sub.live {
                            return None;
                        }
                        sub.pending = Some(page.through_cursor);
                    }
                    _ => {}
                }
                response
            }
            Err(error) => {
                self.subscription = None;
                error_response(identity, error)
            }
        };
        Some((conversation, response))
    }
}
fn error_response(identity: ReadIdentity, error: EventStoreError) -> Response {
    match error {
        EventStoreError::CursorReset(reason) => Response::Reset { identity, reason },
        error => {
            let code = match error {
                EventStoreError::Unauthorized => ReadErrorCode::Unauthorized,
                EventStoreError::NoRun => ReadErrorCode::NoRun,
                EventStoreError::RunMismatch => ReadErrorCode::RunMismatch,
                EventStoreError::InvalidReplay => ReadErrorCode::InvalidRequest,
                _ => {
                    tracing::error!(%error, "Durable conversation read failed");
                    ReadErrorCode::Unavailable
                }
            };
            Response::Error { identity, code }
        }
    }
}

pub(super) async fn send_response<S>(
    sink: &mut S,
    store: &ConversationStore<'_>,
    workspace: &str,
    response: Option<(ConversationId, Response)>,
) -> Result<(), ()>
where
    S: futures_util::Sink<axum::extract::ws::Message> + Unpin,
{
    use futures_util::SinkExt;
    let Some((conversation, mut response)) = response else {
        return Ok(());
    };
    let identity = match &response {
        Response::Snapshot { identity, .. }
        | Response::Replay { identity, .. }
        | Response::Reset { identity, .. }
        | Response::Error { identity, .. }
        | Response::Detail { identity, .. } => identity.clone(),
    };
    let message = kyomi_types::websocket::WebSocketMessage::new(
        kyomi_types::websocket::MessageType::ConversationRead,
    )
    .with_workspace(workspace)
    .with_session(conversation.as_str())
    .with_data(serde_json::to_value(&response).map_err(|_| ())?);
    let json = serde_json::to_string(&message).map_err(|_| ())?;
    // Retain the same membership/session row locks through the actual flush.
    // A completed unshare/revocation therefore wins over a stale captured page.
    // Transaction cleanup can replace the transport error (e.g. backend loss
    // makes rollback fail). Keep the delivery disposition outside that result:
    // once protected delivery was attempted, any error makes this sink unusable.
    let mut delivery_attempted = false;
    match store
        .deliver_authorized(&conversation, async {
            delivery_attempted = true;
            tokio::time::timeout(
                DELIVERY_TIMEOUT,
                sink.send(axum::extract::ws::Message::text(json)),
            )
            .await
            .map_err(|_| EventStoreError::DeliveryTimedOut)?
            .map_err(|_| EventStoreError::DeliveryFailed)
        })
        .await
    {
        Ok(()) => Ok(()),
        Err(error) if delivery_attempted => {
            tracing::warn!(%error, "durable delivery failed; dropping sink without further writes");
            Err(())
        }
        Err(error) => {
            response = error_response(identity, error);
            let message = kyomi_types::websocket::WebSocketMessage::new(
                kyomi_types::websocket::MessageType::ConversationRead,
            )
            .with_workspace(workspace)
            .with_session(conversation.as_str())
            .with_data(serde_json::to_value(response).map_err(|_| ())?);
            let json = serde_json::to_string(&message).map_err(|_| ())?;
            tokio::time::timeout(
                DELIVERY_TIMEOUT,
                sink.send(axum::extract::ws::Message::text(json)),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::{
        AppendCommand, EventId, IdempotencyKey, MAX_REPLAY_BYTES, MessageId, Payload,
        PublicPayload, RunState, Text, VERSION,
    };
    use kyomi_core::DbPool;

    const KEY: [u8; 32] = [61; 32];
    async fn fixture() -> DbPool {
        let _ = kyomi_core::constants::load_with_fallback();
        let db = DbPool::connect("sqlite::memory:").await.unwrap();
        seed_fixture(&db).await;
        db
    }
    async fn seed_fixture(db: &DbPool) {
        for user in ["reader", "shared-reader"] {
            kyomi_core::db_execute!(
                db,
                "INSERT INTO users(user_id,email,active) VALUES($1,$2,true)",
                user,
                format!("{user}@example.test")
            )
            .unwrap();
        }
        kyomi_core::db_execute!(db, "INSERT INTO workspaces(workspace_id,name,owner_user_id) VALUES('read-workspace','fixture','reader')").unwrap();
        for user in ["reader", "shared-reader"] {
            kyomi_core::db_execute!(db, "INSERT INTO workspace_users(workspace_id,user_id,role,active) VALUES('read-workspace',$1,'member',true)", user).unwrap();
        }
        kyomi_core::db_execute!(db, "INSERT INTO chat_sessions(session_id,user_id,workspace_id,shared) VALUES('conversation','reader','read-workspace',true)").unwrap();
    }
    fn command(payload: PublicPayload) -> AppendCommand {
        let id = uuid::Uuid::new_v4().to_string();
        AppendCommand {
            version: VERSION,
            conversation_id: ConversationId("conversation".into()),
            run_id: RunId("old-run".into()),
            event_id: EventId(id.clone()),
            idempotency_key: IdempotencyKey(id),
            payload: Payload::Public(payload),
            detail: None,
        }
    }
    fn request(generation: u64) -> ConversationReadRequest {
        ConversationReadRequest {
            session_id: "conversation".into(),
            run_id: Some(RunId("old-run".into())),
            request_generation: generation,
            after: None,
            limit: 1,
            byte_limit: MAX_REPLAY_BYTES,
        }
    }
    fn watermark(response: &Response) -> i64 {
        match response {
            Response::Snapshot { snapshot, .. } => snapshot.page.through_cursor,
            Response::Replay { page, .. } => page.through_cursor,
            _ => panic!("page response required"),
        }
    }
    fn ack(generation: &str, request: u64, through: i64) -> ReadCommand {
        ReadCommand::Ack {
            connection_generation: generation.into(),
            request_generation: request,
            through_cursor: through,
        }
    }
    #[tokio::test]
    async fn durable_subscription_ack_snapshot_race_lost_wakeup_and_generations() {
        let db = fixture().await;
        let store = ConversationStore::new(&db, &KEY, "reader", "read-workspace");
        store
            .append(&command(PublicPayload::Submitted {
                message_id: MessageId("question".into()),
                text: Text {
                    preview: "question".into(),
                    detail: None,
                },
            }))
            .await
            .unwrap();
        store
            .append(&command(PublicPayload::Planning {
                text: Text {
                    preview: "before snapshot".into(),
                    detail: None,
                },
            }))
            .await
            .unwrap();
        let mut reads = ConnectionReads::new("connection-a".into());
        let (_, first) = reads
            .command(
                ReadCommand::Subscribe {
                    request: request(1),
                },
                &store,
            )
            .await
            .unwrap();
        assert_eq!(watermark(&first), 1);
        assert!(
            reads.catch_up(&store).await.is_none(),
            "pending application must bound staging to one page"
        );
        // Commit during snapshot catch-up and deliberately emit no notification.
        store
            .append(&command(PublicPayload::Interrupted {
                text: Text {
                    preview: "worker lost".into(),
                    detail: None,
                },
            }))
            .await
            .unwrap();
        assert!(
            reads
                .command(ack("connection-b", 1, 1), &store)
                .await
                .is_none()
        );
        assert!(
            reads
                .command(ack("connection-a", 1, 999), &store)
                .await
                .is_none()
        );
        let (_, second) = reads
            .command(ack("connection-a", 1, 1), &store)
            .await
            .unwrap();
        let Response::Replay { page, .. } = &second else {
            panic!("snapshot continuation")
        };
        assert_eq!((page.through_cursor, page.high_watermark), (2, 2));
        // The public projection's private tail is exhausted on the next bounded
        // page. It is harmless to acknowledge an empty terminal snapshot page.
        let response = reads.command(ack("connection-a", 1, 2), &store).await;
        let (_, third) = match response {
            Some(response) => response,
            None => reads.catch_up(&store).await.unwrap(),
        };
        assert_eq!(watermark(&third), 3);
        assert!(
            matches!(&third, Response::Replay { page, .. } if matches!(page.events[0].payload, PublicPayload::Interrupted { .. }))
        );
        assert!(
            reads
                .command(ack("connection-a", 1, 3), &store)
                .await
                .is_none()
        );
        // Replace subscription; delayed old acknowledgement and request are inert.
        let (_, fresh) = reads
            .command(
                ReadCommand::Subscribe {
                    request: request(2),
                },
                &store,
            )
            .await
            .unwrap();
        assert!(
            matches!(fresh, Response::Snapshot { snapshot, .. } if snapshot.run.as_ref().unwrap().state == RunState::Interrupted)
        );
        assert!(
            reads
                .command(ack("connection-a", 1, 3), &store)
                .await
                .is_none()
        );
        assert!(
            reads
                .command(
                    ReadCommand::Subscribe {
                        request: request(1)
                    },
                    &store
                )
                .await
                .is_none()
        );
    }
    #[tokio::test]
    async fn durable_delivery_revocation_after_capture() {
        let db = fixture().await;
        let writer = ConversationStore::new(&db, &KEY, "reader", "read-workspace");
        writer
            .append(&command(PublicPayload::Submitted {
                message_id: MessageId("question".into()),
                text: Text {
                    preview: "private answer scope".into(),
                    detail: None,
                },
            }))
            .await
            .unwrap();
        let viewer = ConversationStore::new(&db, &KEY, "shared-reader", "read-workspace");
        let mut requesting = ConnectionReads::new("requesting-socket".into());
        let received = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let received_for_sink = received.clone();
        let mut sink = Box::pin(futures_util::sink::unfold(
            received_for_sink,
            |received, message: axum::extract::ws::Message| async move {
                if let axum::extract::ws::Message::Text(text) = message {
                    received.lock().unwrap().push(text.to_string());
                }
                Ok::<_, std::convert::Infallible>(received)
            },
        ));
        let response = requesting
            .command(
                ReadCommand::Subscribe {
                    request: request(1),
                },
                &viewer,
            )
            .await;
        send_response(&mut sink, &viewer, "read-workspace", response)
            .await
            .unwrap();
        assert_eq!(received.lock().unwrap().len(), 1);
        let response = requesting
            .command(
                ReadCommand::Subscribe {
                    request: request(2),
                },
                &viewer,
            )
            .await;
        kyomi_core::db_execute!(
            &db,
            "UPDATE chat_sessions SET shared=false WHERE session_id='conversation'"
        )
        .unwrap();
        send_response(&mut sink, &viewer, "read-workspace", response)
            .await
            .unwrap();
        let last = received.lock().unwrap().last().unwrap().clone();
        assert!(last.contains("unauthorized"));
        assert!(!last.contains("private answer scope"));
        assert!(
            !last.contains("old-run"),
            "revocation must hide run existence"
        );
        let (_, revoked) = requesting.catch_up(&viewer).await.unwrap();
        assert!(matches!(
            revoked,
            Response::Error {
                code: ReadErrorCode::Unauthorized,
                ..
            }
        ));
        assert!(requesting.catch_up(&viewer).await.is_none());
    }
    struct StalledSink {
        buffered: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        closed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl futures_util::Sink<axum::extract::ws::Message> for StalledSink {
        type Error = std::convert::Infallible;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(
            self: std::pin::Pin<&mut Self>,
            _: axum::extract::ws::Message,
        ) -> Result<(), Self::Error> {
            self.buffered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Pending
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            self.closed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::task::Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn durable_send_timeout_releases_authorization_lock_without_late_flush() {
        let db = fixture().await;
        let store = ConversationStore::new(&db, &KEY, "reader", "read-workspace");
        store
            .append(&command(PublicPayload::Submitted {
                message_id: MessageId("question".into()),
                text: Text {
                    preview: "answer".into(),
                    detail: None,
                },
            }))
            .await
            .unwrap();
        let mut reads = ConnectionReads::new("stalled-socket".into());
        let response = reads
            .command(
                ReadCommand::Subscribe {
                    request: request(1),
                },
                &store,
            )
            .await;
        let buffered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let closed = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut sink = StalledSink {
            buffered: buffered.clone(),
            closed: closed.clone(),
        };
        assert!(
            send_response(&mut sink, &store, "read-workspace", response)
                .await
                .is_err()
        );
        // Production drops the failed sink directly; it must not call close/flush
        // after rollback releases the revocation ordering lock.
        drop(sink);
        assert_eq!(buffered.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(closed.load(std::sync::atomic::Ordering::SeqCst), 0);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.authorize_read(&ConversationId("conversation".into())),
        )
        .await
        .unwrap()
        .unwrap();
    }
    /// First send buffers a protected frame and never flushes. A forbidden fallback
    /// send would enqueue a second frame and flush both, making the leak observable.
    struct BackendLossSink {
        buffered: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        flushed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        closed: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        started: std::sync::Arc<tokio::sync::Notify>,
    }
    impl futures_util::Sink<axum::extract::ws::Message> for BackendLossSink {
        type Error = std::convert::Infallible;
        fn poll_ready(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn start_send(
            self: std::pin::Pin<&mut Self>,
            _: axum::extract::ws::Message,
        ) -> Result<(), Self::Error> {
            self.buffered
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.started.notify_one();
            Ok(())
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            if self.buffered.load(std::sync::atomic::Ordering::SeqCst) > 1 {
                self.flushed
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::task::Poll::Ready(Ok(()))
            } else {
                std::task::Poll::Pending
            }
        }
        fn poll_close(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<Result<(), Self::Error>> {
            self.closed
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::task::Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn durable_backend_loss_and_failed_rollback_never_reuse_protected_sink() {
        use futures_util::FutureExt;
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };
        let _ = kyomi_core::constants::load_with_fallback();
        let url = kyomi_core::test_db::test_database_url();
        let (server, _) = kyomi_core::test_db::split_database_url(&url);
        let admin = match sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&format!("{server}/postgres"))
            .await
        {
            Ok(pool) => pool,
            Err(error) => {
                assert!(
                    std::env::var("KYOMI_REQUIRE_POSTGRES_TESTS").as_deref() != Ok("1"),
                    "required PostgreSQL unavailable: {error}"
                );
                eprintln!("SKIPPED durable backend-loss test: PostgreSQL unavailable");
                return;
            }
        };
        let name = format!("durable_delivery_{}", uuid::Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}"))
            .execute(&admin)
            .await
            .unwrap();
        let scratch_url = format!("{server}/{name}");
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&scratch_url)
            .await
            .unwrap();
        let revoker = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .connect(&scratch_url)
            .await
            .unwrap();
        let db = DbPool::Postgres(pool.clone());
        let outcome = std::panic::AssertUnwindSafe(async {
            sqlx::migrate!("./migrations").run(&pool).await.unwrap();
            seed_fixture(&db).await;
            let store = ConversationStore::new(&db, &KEY, "shared-reader", "read-workspace");
            ConversationStore::new(&db, &KEY, "reader", "read-workspace")
                .append(&command(PublicPayload::Submitted {
                    message_id: MessageId("question".into()),
                    text: Text {
                        preview: "protected buffered response".into(),
                        detail: None,
                    },
                }))
                .await
                .unwrap();
            let mut reads = ConnectionReads::new("backend-loss-socket".into());
            let response = reads
                .command(
                    ReadCommand::Subscribe {
                        request: request(1),
                    },
                    &store,
                )
                .await;
            let backend: (i32,) = sqlx::query_as("SELECT pg_backend_pid()")
                .fetch_one(&pool)
                .await
                .unwrap();
            let buffered = Arc::new(AtomicUsize::new(0));
            let flushed = Arc::new(AtomicUsize::new(0));
            let closed = Arc::new(AtomicUsize::new(0));
            let started = Arc::new(tokio::sync::Notify::new());
            let mut sink = BackendLossSink {
                buffered: buffered.clone(),
                flushed: flushed.clone(),
                closed: closed.clone(),
                started: started.clone(),
            };
            let lose_backend = async {
                started.notified().await;
                assert_eq!(buffered.load(Ordering::SeqCst), 1);
                let terminated: (bool,) = sqlx::query_as("SELECT pg_terminate_backend($1)")
                    .bind(backend.0)
                    .fetch_one(&admin)
                    .await
                    .unwrap();
                assert!(terminated.0);
                // Backend loss releases authorization locks while protected flush is
                // still pending. Revocation must complete before the send times out.
                tokio::time::timeout(
                    std::time::Duration::from_secs(1),
                    sqlx::query(
                        "UPDATE chat_sessions SET shared=false WHERE session_id='conversation'",
                    )
                    .execute(&revoker),
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(buffered.load(Ordering::SeqCst), 1);
                assert_eq!(flushed.load(Ordering::SeqCst), 0);
            };
            let (delivery, ()) = tokio::join!(
                send_response(&mut sink, &store, "read-workspace", response),
                lose_backend
            );
            assert!(
                delivery.is_err(),
                "failed rollback must retain fatal delivery disposition"
            );
            drop(sink);
            assert_eq!(buffered.load(Ordering::SeqCst), 1, "no fallback error send");
            assert_eq!(
                flushed.load(Ordering::SeqCst),
                0,
                "no later protected flush"
            );
            assert_eq!(
                closed.load(Ordering::SeqCst),
                0,
                "no close after lost authorization lock"
            );
            assert!(matches!(
                store
                    .authorize_read(&ConversationId("conversation".into()))
                    .await,
                Err(EventStoreError::Unauthorized)
            ));
        })
        .catch_unwind()
        .await;
        pool.close().await;
        revoker.close().await;
        sqlx::query(&format!("DROP DATABASE {name} WITH (FORCE)"))
            .execute(&admin)
            .await
            .unwrap();
        admin.close().await;
        outcome.unwrap();
    }
}
