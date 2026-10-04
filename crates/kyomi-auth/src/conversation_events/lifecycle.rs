use super::*;
use agent_runtime::{
    IdempotencyKey, Lease, MessageId, PublicPayload, RunId, RunSnapshot, SubmitCommand, Text,
};

#[derive(sqlx::FromRow)]
pub(super) struct RunRow {
    state: String,
    assistant_message_id: String,
    encrypted_submission: String,
    fence: i64,
    lease_owner: Option<String>,
    lease_expires_at: Option<i64>,
    cancellation_requested: bool,
    queued_at: i64,
    started_at: Option<i64>,
    terminal_at: Option<i64>,
}
impl RunRow {
    pub(super) fn snapshot(&self) -> Result<RunSnapshot> {
        Ok(RunSnapshot {
            state: self.state.parse()?,
            lease: self.lease_owner.as_ref().zip(self.lease_expires_at).map(
                |(owner, expires_at)| Lease {
                    owner: owner.clone(),
                    fence: self.fence,
                    expires_at,
                },
            ),
            cancellation_requested: self.cancellation_requested,
            assistant_message_id: MessageId(self.assistant_message_id.clone()),
            fence: self.fence,
            queued_at: self.queued_at,
            started_at: self.started_at,
            terminal_at: self.terminal_at,
        })
    }
}
#[derive(Debug)]
pub struct SubmissionReceipt {
    pub run_id: RunId,
    pub user_message_id: MessageId,
    pub assistant_message_id: MessageId,
    pub duplicate: bool,
}
#[derive(Debug)]
pub struct ClaimedRun {
    pub snapshot: RunSnapshot,
    pub submission: SubmitCommand,
    pub context: ClaimedConversationContext,
}
#[derive(Debug, Clone)]
pub struct ClaimedConversationContext {
    pub messages: Vec<crate::chat_service::AgentMessage>,
    pub config: Option<serde_json::Value>,
    pub shared: bool,
}
#[derive(Debug, sqlx::FromRow)]
pub struct QueuedRun {
    pub conversation_id: String,
    pub run_id: String,
    pub actor_id: String,
    pub workspace_id: String,
    pub expired: bool,
}
/// Queue discovery is not authorization. Each claim/expiry rechecks its stored scope.
pub async fn discover_queue(db: &DbPool, limit: usize) -> Result<Vec<QueuedRun>> {
    if limit == 0 || limit > agent_runtime::MAX_REPLAY_EVENTS {
        return Err(EventStoreError::InvalidReplay);
    }
    let now = chrono::Utc::now().timestamp_millis();
    let query = "SELECT r.session_id AS conversation_id,r.run_id,r.actor_id,r.workspace_id,(r.state='running' OR NOT EXISTS(SELECT 1 FROM workspace_users m JOIN users u ON u.user_id=m.user_id JOIN chat_sessions s ON s.session_id=r.session_id WHERE m.workspace_id=r.workspace_id AND m.user_id=r.actor_id AND m.active=true AND u.active=true AND (s.user_id=r.actor_id OR s.shared=true))) AS expired FROM conversation_runs r WHERE r.request_id IS NOT NULL AND ((r.state='running' AND r.lease_expires_at <= $1) OR (r.state='queued' AND NOT EXISTS(SELECT 1 FROM conversation_runs active WHERE active.session_id=r.session_id AND active.state='running' AND active.request_id IS NOT NULL) AND NOT EXISTS(SELECT 1 FROM conversation_runs earlier WHERE earlier.session_id=r.session_id AND earlier.state='queued' AND earlier.request_id IS NOT NULL AND (earlier.queued_at<r.queued_at OR (earlier.queued_at=r.queued_at AND earlier.run_id<r.run_id))))) ORDER BY CASE WHEN r.state='running' THEN 0 ELSE 1 END,r.queued_at,r.run_id LIMIT $2";
    Ok(kyomi_core::db_fetch_all!(
        db,
        QueuedRun,
        query,
        now,
        limit as i64
    )?)
}
fn event(submission: &SubmitCommand, payload: PublicPayload, key: &str) -> AppendCommand {
    AppendCommand {
        version: VERSION,
        conversation_id: submission.submitted.conversation_id.clone(),
        run_id: submission.submitted.run_id.clone(),
        event_id: EventId(uuid::Uuid::new_v4().to_string()),
        idempotency_key: IdempotencyKey(hex::encode(Sha256::digest(format!(
            "{}:{key}",
            submission.submitted.run_id.as_str()
        )))),
        payload: Payload::Public(payload),
        detail: None,
    }
}
fn command_fingerprint(command: &AppendCommand) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(command)?)))
}
fn receipt(command: &SubmitCommand, duplicate: bool) -> Result<SubmissionReceipt> {
    let Payload::Public(PublicPayload::Submitted { message_id, .. }) = &command.submitted.payload
    else {
        return Err(agent_runtime::PolicyError::InvalidSubmission.into());
    };
    Ok(SubmissionReceipt {
        run_id: command.submitted.run_id.clone(),
        user_message_id: message_id.clone(),
        assistant_message_id: command.assistant_message_id.clone(),
        duplicate,
    })
}
macro_rules! write_snapshot {
    ($tx:ident,$run:expr,$snapshot:expr) => {{
        let next = $snapshot;
        let changed = sqlx::query("UPDATE conversation_runs SET state=$1,fence=$2,lease_owner=$3,lease_expires_at=$4,cancellation_requested=$5,started_at=$6,terminal_at=$7 WHERE run_id=$8")
            .bind(next.state.as_str()).bind(next.fence).bind(next.lease.as_ref().map(|l|l.owner.as_str())).bind(next.lease.as_ref().map(|l|l.expires_at)).bind(next.cancellation_requested).bind(next.started_at).bind(next.terminal_at).bind($run).execute(&mut *$tx).await?;
        if changed.rows_affected()!=1 { return Err(EventStoreError::MissingProjection); }
    }};
}
// Same canonical snapshot and sync insert builders as the legacy chat service,
// executed inside the mutation transaction so rollback cannot publish a phantom delta.
macro_rules! write_session_sync {
    ($store:expr, $tx:ident, $conversation:expr, $action:expr) => {{
        let query = crate::chat_service::session_snapshot_query($store.db.is_postgres());
        let row: crate::chat_service::SessionSnapshotRow = sqlx::query_as(&query)
            .bind($conversation).fetch_one(&mut *$tx).await?;
        let counts: (i64, i64) = sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(CASE WHEN pinned=true THEN 1 ELSE 0 END),0) FROM chat_messages WHERE session_id=$1")
            .bind($conversation).fetch_one(&mut *$tx).await?;
        let snapshot = crate::chat_service::session_snapshot_json(&row, &crate::chat_service::SessionCounts { message_count:counts.0, pinned_count:counts.1 });
        let params = crate::sync_log_service::SyncEntryParams {
            entity_type: kyomi_types::sync::entity_types::CHAT_SESSION,
            entity_id:$conversation, workspace_id:&row.workspace_id, action:$action,
            data:Some(snapshot), owner_user_id:Some(&row.user_id), is_workspace_visible:row.shared,
        };
        let (sql,data) = crate::sync_log_service::build_insert_sql_and_data($store.db.is_postgres(), kyomi_core::sql_compat::now($store.db.is_postgres()), &params)?;
        let action = match params.action { kyomi_types::sync::SyncActionType::Insert => "insert", kyomi_types::sync::SyncActionType::Update => "update", kyomi_types::sync::SyncActionType::Delete => "delete" };
        sqlx::query(&sql).bind(params.entity_type).bind(params.entity_id).bind(params.workspace_id).bind(action).bind(data).bind(params.owner_user_id).execute(&mut *$tx).await?;
    }};
}
macro_rules! write_terminal {
    ($store:expr,$tx:ident,$submission:expr,$plan:expr) => {{
        let plan = $plan;
        if let Some(projection) = &plan.terminal_projection {
            let text=Text {preview:projection.content.chars().take(8000).collect(),detail:if projection.content.chars().count()>8000 {Some(DetailId(uuid::Uuid::new_v4().to_string()))}else{None}};
            let payload=match plan.snapshot.state {
                RunState::Completed=>PublicPayload::ApprovedAnswer {message_id:projection.message_id.clone(),text},
                RunState::Failed=>PublicPayload::Failed {text},
                RunState::Interrupted=>PublicPayload::Interrupted {text},
                RunState::Cancelled=>PublicPayload::Cancelled,
                _=>return Err(agent_runtime::PolicyError::InvalidState.into()),
            };
            let mut command=event($submission,payload,"terminal");
            if projection.content.chars().count()>8000 && plan.snapshot.state!=RunState::Cancelled {command.detail=Some(projection.content.clone());}
            agent_runtime::validate(&command)?;
            let fingerprint = command_fingerprint(&command)?;
            persist_append!($store,$tx,command,fingerprint,false)?;
            if plan.snapshot.state != RunState::Completed {
                let encrypted = encryption::encrypt(&projection.content,$store.key)?;
                let updated = sqlx::query("UPDATE chat_messages SET content=$1,status=$2,created_at=$3 WHERE message_id=$4 AND session_id=$5 AND status='in_progress' AND role='assistant'")
                    .bind(encrypted).bind(projection.status).bind(projection_time!($tx,$submission.submitted.conversation_id.as_str())).bind(projection.message_id.as_str()).bind($submission.submitted.conversation_id.as_str()).execute(&mut *$tx).await?;
                if updated.rows_affected()!=1 { return Err(EventStoreError::MissingProjection); }
            }
            write_snapshot!($tx,$submission.submitted.run_id.as_str(),&plan.snapshot);
            write_session_sync!($store,$tx,$submission.submitted.conversation_id.as_str(),kyomi_types::sync::SyncActionType::Update);
        }
    }};
}
impl<'a> ConversationStore<'a> {
    fn for_lifecycle(&self) -> ConversationStore<'a> {
        ConversationStore {
            db: self.db,
            key: self.key,
            actor_id: self.actor_id,
            workspace_id: self.workspace_id,
            lease: self.lease,
            reconcile_expired: false,
            lifecycle_scope: true,
            deterministic_lease_clock: self.deterministic_lease_clock,
        }
    }
}
impl ConversationStore<'_> {
    pub async fn submit(&self, command: &SubmitCommand) -> Result<SubmissionReceipt> {
        agent_runtime::plan_submission(command, None)?;
        if command.actor.actor_id != self.actor_id {
            return Err(EventStoreError::Unauthorized);
        }
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            // PostgreSQL request-scoped lock serializes cross-conversation retries without
            // reversing the session -> membership locking order used by every operation.
            if self.db.is_postgres() {
                let scope = serde_json::to_string(&(
                    self.workspace_id,
                    self.actor_id,
                    command.request_id.as_str(),
                ))?;
                sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
                    .bind(scope)
                    .execute(&mut *tx)
                    .await?;
            }
            let existing:Option<(String,)> = sqlx::query_as("SELECT encrypted_submission FROM conversation_runs WHERE workspace_id=$1 AND actor_id=$2 AND request_id=$3")
                .bind(self.workspace_id).bind(self.actor_id).bind(command.request_id.as_str()).fetch_optional(&mut *tx).await?;
            if let Some((encrypted,)) = existing {
                let saved: SubmitCommand =
                    serde_json::from_str(&encryption::decrypt(&encrypted, self.key)?)?;
                authorize!(&scoped, tx, saved.submitted.conversation_id.as_str(), true);
                agent_runtime::plan_submission(command, Some(&saved))?;
                return receipt(&saved, true);
            }
            let session = command.submitted.conversation_id.as_str();
            if command.new_conversation {
                if session.is_empty() || uuid::Uuid::parse_str(session).is_err() {
                    return Err(agent_runtime::PolicyError::InvalidIdentity.into());
                }
                sqlx::query("INSERT INTO chat_sessions(session_id,user_id,workspace_id,session_type,shared) VALUES($1,$2,$3,'chat',false)")
                    .bind(session).bind(self.actor_id).bind(self.workspace_id).execute(&mut *tx).await?;
            }
            authorize!(&scoped, tx, session, true);
            let submitted = &command.submitted;
            let fingerprint = command_fingerprint(submitted)?;
            persist_append!(&scoped, tx, submitted, fingerprint, false)?;
            let empty = encryption::encrypt("", self.key)?;
            sqlx::query("INSERT INTO chat_messages(message_id,session_id,role,content,status,pinned,created_at) VALUES($1,$2,'assistant',$3,'in_progress',false,$4)")
                .bind(command.assistant_message_id.as_str()).bind(session).bind(empty).bind(chrono::Utc::now()).execute(&mut *tx).await?;
            let encrypted = encryption::encrypt(&serde_json::to_string(command)?, self.key)?;
            sqlx::query("UPDATE conversation_runs SET request_id=$1,actor_id=$2,workspace_id=$3,assistant_message_id=$4,encrypted_submission=$5,queued_at=$6,user_message_id=$8 WHERE run_id=$7")
                .bind(command.request_id.as_str()).bind(self.actor_id).bind(self.workspace_id).bind(command.assistant_message_id.as_str()).bind(encrypted).bind(command.submitted_at).bind(submitted.run_id.as_str()).bind(match &submitted.payload {Payload::Public(PublicPayload::Submitted {message_id,..})=>message_id.as_str(),_=>""}).execute(&mut *tx).await?;
            let Payload::Public(PublicPayload::Submitted { message_id, .. }) = &submitted.payload
            else {
                return Err(agent_runtime::PolicyError::InvalidSubmission.into());
            };
            sqlx::query("UPDATE chat_messages SET sent_by_user_id=$1,current_time_user_tz=$2,message_source=$3 WHERE message_id=$4")
                .bind(self.actor_id).bind(command.context.get("current_time_user_tz").and_then(|v|v.as_str())).bind(&command.actor.source).bind(message_id.as_str()).execute(&mut *tx).await?;
            let queued = event(
                command,
                PublicPayload::RunState {
                    state: RunState::Queued,
                },
                "queued",
            );
            // Queued is the submission's initial state; writer policy already has it.
            let fingerprint = command_fingerprint(&queued)?;
            persist_append!(&scoped, tx, queued, fingerprint, false)?;
            write_session_sync!(
                &scoped,
                tx,
                session,
                if command.new_conversation {
                    kyomi_types::sync::SyncActionType::Insert
                } else {
                    kyomi_types::sync::SyncActionType::Update
                }
            );
            receipt(command, false)
        })
    }
    pub async fn claim(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        owner: &str,
        now: i64,
        ttl: i64,
    ) -> Result<ClaimedRun> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            let active:(i64,) = sqlx::query_as("SELECT COUNT(*) FROM conversation_runs WHERE session_id=$1 AND request_id IS NOT NULL AND (state='running' OR (state='queued' AND (queued_at<$2 OR (queued_at=$2 AND run_id<$3))))")
                .bind(conversation.as_str()).bind(row.queued_at).bind(run.as_str()).fetch_one(&mut *tx).await?;
            let next = agent_runtime::plan_claim(&row.snapshot()?, owner, now, ttl, active.0 > 0)?;
            let submission: SubmitCommand =
                serde_json::from_str(&encryption::decrypt(&row.encrypted_submission, self.key)?)?;
            let command = event(
                &submission,
                PublicPayload::RunState {
                    state: RunState::Running,
                },
                "started",
            );
            let fingerprint = command_fingerprint(&command)?;
            persist_append!(&scoped, tx, command, fingerprint, false)?;
            write_snapshot!(tx, run.as_str(), &next);
            let created_at = projection_time!(tx, conversation.as_str());
            let Payload::Public(PublicPayload::Submitted { message_id, .. }) =
                &submission.submitted.payload
            else {
                return Err(agent_runtime::PolicyError::InvalidSubmission.into());
            };
            let updated=sqlx::query("UPDATE chat_messages SET created_at=$1 WHERE message_id=$2 AND session_id=$3 AND role='user'").bind(created_at).bind(message_id.as_str()).bind(conversation.as_str()).execute(&mut *tx).await?;
            if updated.rows_affected() != 1 {
                return Err(EventStoreError::MissingProjection);
            }
            let (config, shared): (Option<serde_json::Value>, bool) = sqlx::query_as(
                "SELECT config,COALESCE(shared,false) FROM chat_sessions WHERE session_id=$1",
            )
            .bind(conversation.as_str())
            .fetch_one(&mut *tx)
            .await?;
            let rows=sqlx::query_as::<_,crate::chat_service::AgentMessage>("SELECT message_id,role,content,tool_calls,tool_call_id,tool_name,sent_by_user_id,current_time_user_tz,message_source FROM chat_messages WHERE session_id=$1 AND message_id<>$2 AND NOT EXISTS(SELECT 1 FROM conversation_runs future WHERE (future.user_message_id=chat_messages.message_id OR future.assistant_message_id=chat_messages.message_id) AND future.request_id IS NOT NULL AND (future.queued_at>$3 OR (future.queued_at=$3 AND future.run_id>$4))) ORDER BY created_at,message_id")
                .bind(conversation.as_str()).bind(message_id.as_str()).bind(row.queued_at).bind(run.as_str()).fetch_all(&mut *tx).await?;
            let mut messages = Vec::with_capacity(rows.len());
            for mut message in rows {
                message.content = encryption::decrypt(&message.content, self.key)?;
                message.tool_calls = message
                    .tool_calls
                    .as_ref()
                    .map(|value| encryption::restore_json_field(value, self.key))
                    .transpose()?;
                if message.role == "assistant"
                    && message.content.trim().is_empty()
                    && message.tool_calls.is_none()
                {
                    continue;
                }
                messages.push(message);
            }
            write_session_sync!(
                &scoped,
                tx,
                conversation.as_str(),
                kyomi_types::sync::SyncActionType::Update
            );
            Ok(ClaimedRun {
                snapshot: next,
                submission,
                context: ClaimedConversationContext {
                    messages,
                    config: config
                        .as_ref()
                        .map(|value| encryption::restore_chat_config(value, self.key))
                        .transpose()?,
                    shared,
                },
            })
        })
    }
    pub async fn heartbeat(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        ttl: i64,
    ) -> Result<RunSnapshot> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            let next = agent_runtime::plan_heartbeat(&row.snapshot()?, lease, now, ttl)?;
            write_snapshot!(tx, run.as_str(), &next);
            Ok(next)
        })
    }
    pub async fn cancel(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        now: i64,
    ) -> Result<RunSnapshot> {
        Ok(self.cancel_plan(conversation, run, now).await?.snapshot)
    }
    async fn cancel_plan(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        now: i64,
    ) -> Result<agent_runtime::LifecyclePlan> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            let plan = agent_runtime::plan_cancel(&row.snapshot()?, now)?;
            let submission: SubmitCommand =
                serde_json::from_str(&encryption::decrypt(&row.encrypted_submission, self.key)?)?;
            if plan.changed {
                let command = event(
                    &submission,
                    PublicPayload::CancellationRequested,
                    "cancel_requested",
                );
                let fingerprint = command_fingerprint(&command)?;
                persist_append!(&scoped, tx, command, fingerprint, false)?;
                write_terminal!(&scoped, tx, &submission, &plan);
                write_snapshot!(tx, run.as_str(), &plan.snapshot);
                if plan.terminal_projection.is_none() {
                    write_session_sync!(
                        &scoped,
                        tx,
                        conversation.as_str(),
                        kyomi_types::sync::SyncActionType::Update
                    );
                }
            }
            Ok(plan)
        })
    }
    pub async fn finish(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        state: RunState,
        content: &str,
    ) -> Result<RunSnapshot> {
        self.finish_with_metadata(conversation, run, lease, now, (state, content), None)
            .await
    }
    pub async fn finish_with_metadata(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        outcome: (RunState, &str),
        metadata: Option<&serde_json::Value>,
    ) -> Result<RunSnapshot> {
        Ok(self
            .finish_plan_with_metadata(conversation, run, lease, now, outcome, metadata)
            .await?
            .snapshot)
    }
    async fn finish_plan_with_metadata(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        outcome: (RunState, &str),
        metadata: Option<&serde_json::Value>,
    ) -> Result<agent_runtime::LifecyclePlan> {
        let (state, content) = outcome;
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            let plan = agent_runtime::plan_finalize(&row.snapshot()?, lease, now, state, content)?;
            let submission: SubmitCommand =
                serde_json::from_str(&encryption::decrypt(&row.encrypted_submission, self.key)?)?;
            write_terminal!(&scoped, tx, &submission, &plan);
            if let Some(metadata) = metadata {
                let encrypted = encryption::encrypt(&serde_json::to_string(metadata)?, self.key)?;
                let updated=sqlx::query("UPDATE chat_messages SET extra_metadata=$1 WHERE message_id=$2 AND session_id=$3 AND role='assistant'").bind(encrypted).bind(row.assistant_message_id.as_str()).bind(conversation.as_str()).execute(&mut *tx).await?;
                if updated.rows_affected() != 1 {
                    return Err(EventStoreError::MissingProjection);
                }
            }
            Ok(plan)
        })
    }
    pub async fn expire(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        now: i64,
    ) -> Result<RunSnapshot> {
        let clock_started = std::time::Instant::now();
        let recovery = ConversationStore {
            db: self.db,
            key: self.key,
            actor_id: self.actor_id,
            workspace_id: self.workspace_id,
            lease: None,
            reconcile_expired: true,
            lifecycle_scope: true,
            deterministic_lease_clock: self.deterministic_lease_clock,
        };
        transaction!(&recovery, tx, {
            authorize!(&recovery, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            let snapshot = row.snapshot()?;
            let authorized:Option<(String,)>=sqlx::query_as("SELECT m.role FROM workspace_users m JOIN users u ON u.user_id=m.user_id JOIN chat_sessions s ON s.session_id=$1 WHERE m.workspace_id=$2 AND m.user_id=$3 AND m.active=true AND u.active=true AND (s.user_id=$3 OR s.shared=true)").bind(conversation.as_str()).bind(self.workspace_id).bind(self.actor_id).fetch_optional(&mut *tx).await?;
            let plan = if snapshot.state == RunState::Queued && authorized.is_none() {
                agent_runtime::plan_interrupt_queued(&snapshot, now, "Authorization revoked")?
            } else {
                agent_runtime::plan_expire(&snapshot, now)?
            };
            let submission: SubmitCommand =
                serde_json::from_str(&encryption::decrypt(&row.encrypted_submission, self.key)?)?;
            write_terminal!(&recovery, tx, &submission, &plan);
            Ok(plan.snapshot)
        })
    }
    pub async fn fenced_append(
        &self,
        command: &AppendCommand,
        lease: &Lease,
    ) -> Result<CommitReceipt> {
        ConversationStore {
            db: self.db,
            key: self.key,
            actor_id: self.actor_id,
            workspace_id: self.workspace_id,
            lease: Some(lease),
            reconcile_expired: false,
            lifecycle_scope: true,
            deterministic_lease_clock: self.deterministic_lease_clock,
        }
        .append(command)
        .await
    }
    pub async fn find_active_run(&self, conversation: &ConversationId) -> Result<Option<RunId>> {
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row:Option<(String,)> = sqlx::query_as("SELECT run_id FROM conversation_runs WHERE session_id=$1 AND request_id IS NOT NULL AND state IN ('queued','running') AND actor_id=$2 ORDER BY queued_at,run_id LIMIT 1").bind(conversation.as_str()).bind(self.actor_id).fetch_optional(&mut *tx).await?;
            Ok(row.map(|r| RunId(r.0)))
        })
    }
}

#[derive(sqlx::FromRow)]
struct MessageProjectionRow {
    content: String,
    role: String,
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_calls: Option<serde_json::Value>,
}
/// Complete compatibility message and its typed event commit in the ownership transaction.
#[derive(Debug, Clone)]
pub struct MessageWrite {
    pub message_id: String,
    pub role: String,
    pub content: String,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    pub tool_calls: Option<serde_json::Value>,
}
impl ConversationStore<'_> {
    pub async fn fenced_message(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        message: &MessageWrite,
    ) -> Result<CommitReceipt> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            agent_runtime::validate_ownership(&row.snapshot()?, lease, now)?;
            if message.message_id.is_empty()
                || message.message_id.len() > 128
                || !matches!(message.role.as_str(), "assistant" | "tool")
            {
                return Err(agent_runtime::PolicyError::InvalidIdentity.into());
            }
            let submission: SubmitCommand =
                serde_json::from_str(&encryption::decrypt(&row.encrypted_submission, self.key)?)?;
            let payload = PublicPayload::MessageRecorded {
                message_id: MessageId(message.message_id.clone()),
                role: if message.role == "tool" {
                    agent_runtime::RecordedRole::Tool
                } else {
                    agent_runtime::RecordedRole::Assistant
                },
                text: Text {
                    preview: message.content.chars().take(8000).collect(),
                    detail: None,
                },
                tool_call_id: message.tool_call_id.clone().map(agent_runtime::ToolCallId),
                name: message.name.clone(),
                tool_calls: message.tool_calls.clone(),
            };
            let mut command = event(&submission, payload, "message");
            command.idempotency_key = IdempotencyKey(hex::encode(Sha256::digest(format!(
                "message:{}",
                message.message_id
            ))));
            // Preserve the complete text via encrypted lazy detail when the preview is bounded.
            if message.content.chars().count() > 8000 {
                let detail = DetailId(uuid::Uuid::new_v4().to_string());
                if let Payload::Public(PublicPayload::MessageRecorded { text, .. }) =
                    &mut command.payload
                {
                    text.detail = Some(detail);
                }
                command.detail = Some(message.content.clone());
            }
            agent_runtime::validate(&command)?;
            // Stable complete message retry identities do not depend on generated event/detail IDs.
            let prior:Option<MessageProjectionRow> = sqlx::query_as("SELECT content,role,tool_call_id,tool_name,tool_calls FROM chat_messages WHERE session_id=$1 AND message_id=$2").bind(conversation.as_str()).bind(&message.message_id).fetch_optional(&mut *tx).await?;
            let existing:Option<(String,i64)>=sqlx::query_as("SELECT event_id,sequence FROM conversation_events WHERE session_id=$1 AND idempotency_key=$2").bind(conversation.as_str()).bind(command.idempotency_key.as_str()).fetch_optional(&mut *tx).await?;
            if let Some((event_id, sequence)) = existing {
                if let Some(prior) = &prior {
                    if encryption::decrypt(&prior.content, self.key)? != message.content
                        || prior.role != message.role
                        || prior.tool_call_id != message.tool_call_id
                        || prior.tool_name != message.name
                        || prior
                            .tool_calls
                            .as_ref()
                            .map(|value| encryption::restore_json_field(value, self.key))
                            .transpose()?
                            != message.tool_calls
                    {
                        return Err(EventStoreError::IdempotencyConflict);
                    }
                    return Ok(CommitReceipt {
                        event_id: EventId(event_id),
                        sequence,
                        duplicate: true,
                    });
                }
                return Err(EventStoreError::MissingProjection);
            }
            if prior.is_some() && message.message_id != row.assistant_message_id {
                return Err(EventStoreError::IdempotencyConflict);
            }
            let fingerprint = command_fingerprint(&command)?;
            let receipt = persist_append!(&scoped, tx, command, fingerprint, false)?;
            if receipt.duplicate {
                return Ok(receipt);
            }
            let encrypted = encryption::encrypt(&message.content, self.key)?;
            let tool_calls = message
                .tool_calls
                .as_ref()
                .map(|value| encryption::protect_json_field(value, self.key))
                .transpose()?;
            if message.message_id == row.assistant_message_id {
                let updated=sqlx::query("UPDATE chat_messages SET content=$1,tool_call_id=$2,tool_name=$3,tool_calls=$4 WHERE message_id=$5 AND session_id=$6 AND status='in_progress' AND role='assistant'")
                    .bind(encrypted).bind(&message.tool_call_id).bind(&message.name).bind(&tool_calls).bind(&message.message_id).bind(conversation.as_str()).execute(&mut *tx).await?;
                if updated.rows_affected() != 1 {
                    return Err(EventStoreError::MissingProjection);
                }
            } else {
                sqlx::query("INSERT INTO chat_messages(message_id,session_id,role,content,tool_call_id,tool_name,tool_calls,status,pinned,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,'complete',false,$8)")
                    .bind(&message.message_id).bind(conversation.as_str()).bind(&message.role).bind(encrypted).bind(&message.tool_call_id).bind(&message.name).bind(&tool_calls).bind(chrono::Utc::now()).execute(&mut *tx).await?;
            }
            write_session_sync!(
                &scoped,
                tx,
                conversation.as_str(),
                kyomi_types::sync::SyncActionType::Update
            );
            Ok(receipt)
        })
    }
    pub async fn fenced_session_metadata(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        metadata: &serde_json::Value,
    ) -> Result<()> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            agent_runtime::validate_ownership(&row.snapshot()?, lease, now)?;
            let previous: Option<serde_json::Value> =
                sqlx::query_scalar("SELECT config FROM chat_sessions WHERE session_id=$1")
                    .bind(conversation.as_str())
                    .fetch_one(&mut *tx)
                    .await?;
            if previous
                .as_ref()
                .map(|value| encryption::restore_chat_config(value, self.key))
                .transpose()?
                .as_ref()
                == Some(metadata)
            {
                return Ok(());
            }
            let encrypted_config = encryption::protect_chat_config(metadata, self.key)?;
            sqlx::query("UPDATE chat_sessions SET config=$1,updated_at=$2 WHERE session_id=$3")
                .bind(encrypted_config)
                .bind(chrono::Utc::now())
                .bind(conversation.as_str())
                .execute(&mut *tx)
                .await?;
            write_session_sync!(
                &scoped,
                tx,
                conversation.as_str(),
                kyomi_types::sync::SyncActionType::Update
            );
            Ok(())
        })
    }
    pub async fn find_run_by_assistant(
        &self,
        conversation: &ConversationId,
        message_id: &str,
    ) -> Result<Option<RunId>> {
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row:Option<(String,)> = sqlx::query_as("SELECT run_id FROM conversation_runs WHERE session_id=$1 AND assistant_message_id=$2 AND request_id IS NOT NULL AND actor_id=$3").bind(conversation.as_str()).bind(message_id).bind(self.actor_id).fetch_optional(&mut *tx).await?;
            Ok(row.map(|r| RunId(r.0)))
        })
    }
}

impl ConversationStore<'_> {
    pub async fn is_legacy_conversation(&self, conversation: &ConversationId) -> Result<bool> {
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row: (String,) =
                sqlx::query_as("SELECT session_type FROM chat_sessions WHERE session_id=$1")
                    .bind(conversation.as_str())
                    .fetch_one(&mut *tx)
                    .await?;
            Ok(row.0 != "chat")
        })
    }
    pub async fn fenced_thinking_details(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        message_id: &str,
        full_texts: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            agent_runtime::validate_ownership(&row.snapshot()?, lease, now)?;
            if row.assistant_message_id != message_id {
                return Err(EventStoreError::Unauthorized);
            }
            let submission: SubmitCommand =
                serde_json::from_str(&encryption::decrypt(&row.encrypted_submission, self.key)?)?;
            let mut changed = false;
            for (id, body) in full_texts {
                let saved:Option<(String,)> = sqlx::query_as("SELECT full_text FROM thinking_event_details WHERE message_id=$1 AND event_id=$2").bind(message_id).bind(id).fetch_optional(&mut *tx).await?;
                if let Some((encrypted,)) = saved {
                    if encryption::decrypt(&encrypted, self.key)? != *body {
                        return Err(EventStoreError::IdempotencyConflict);
                    }
                    continue;
                }
                let mut command = event(
                    &submission,
                    PublicPayload::Planning {
                        text: Text {
                            preview: body.chars().take(200).collect(),
                            detail: Some(DetailId(uuid::Uuid::new_v4().to_string())),
                        },
                    },
                    "thinking",
                );
                command.idempotency_key =
                    IdempotencyKey(hex::encode(Sha256::digest(format!("thinking:{id}"))));
                command.detail = Some(body.clone());
                agent_runtime::validate(&command)?;
                changed = true;
                let fingerprint = command_fingerprint(&command)?;
                persist_append!(&scoped, tx, command, fingerprint, false)?;
                let encrypted = encryption::encrypt(body, self.key)?;
                sqlx::query("INSERT INTO thinking_event_details(id,message_id,event_id,full_text) VALUES($1,$2,$3,$4)")
                    .bind(uuid::Uuid::new_v4().to_string()).bind(message_id).bind(id).bind(encrypted).execute(&mut *tx).await?;
            }
            if changed {
                write_session_sync!(
                    &scoped,
                    tx,
                    conversation.as_str(),
                    kyomi_types::sync::SyncActionType::Update
                );
            }
            Ok(())
        })
    }
    async fn conversation_for_run(&self, run: &RunId) -> Result<ConversationId> {
        let row: Option<(String,)> = kyomi_core::db_fetch_optional!(
            self.db,
            (String,),
            "SELECT session_id FROM conversation_runs WHERE run_id=$1 AND actor_id=$2 AND workspace_id=$3",
            run.as_str(),
            self.actor_id,
            self.workspace_id
        )?;
        Ok(ConversationId(row.ok_or(EventStoreError::Unauthorized)?.0))
    }
}
/// Billing fields recorded only while the durable run still owns its valid lease.
pub struct ApiUsageWrite {
    pub provider: String,
    pub model: String,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub total_tokens: i32,
    pub cost_estimate: f64,
    pub component: String,
    pub provider_cost_usd: Option<f64>,
}
impl ConversationStore<'_> {
    pub async fn fenced_api_usage(
        &self,
        conversation: &ConversationId,
        run: &RunId,
        lease: &Lease,
        now: i64,
        usage: &ApiUsageWrite,
    ) -> Result<()> {
        let clock_started = std::time::Instant::now();
        let scoped = self.for_lifecycle();
        transaction!(&scoped, tx, {
            authorize!(&scoped, tx, conversation.as_str(), true);
            let row = load_run!(self, tx, conversation.as_str(), run.as_str());
            let now = self.locked_now(now, clock_started)?;
            agent_runtime::validate_ownership(&row.snapshot()?, lease, now)?;
            let timestamp = chrono::DateTime::from_timestamp_millis(now)
                .ok_or(agent_runtime::LifecycleError::InvalidLease)?;
            sqlx::query("INSERT INTO api_usage_log (user_id,workspace_id,session_id,timestamp,provider,model,input_tokens,output_tokens,total_tokens,cache_creation_input_tokens,cache_read_input_tokens,cost_estimate,component,provider_cost_usd) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,0,0,$10,$11,$12)")
                .bind(self.actor_id).bind(self.workspace_id).bind(conversation.as_str()).bind(timestamp).bind(&usage.provider).bind(&usage.model).bind(usage.input_tokens).bind(usage.output_tokens).bind(usage.total_tokens).bind(usage.cost_estimate).bind(&usage.component).bind(usage.provider_cost_usd).execute(&mut *tx).await?;
            Ok(())
        })
    }
}
#[async_trait::async_trait]
impl agent_runtime::AtomicLifecyclePersistence for ConversationStore<'_> {
    type Error = EventStoreError;
    async fn submit(&self, command: &SubmitCommand) -> Result<agent_runtime::SubmissionPlan> {
        let result = ConversationStore::submit(self, command).await?;
        Ok(agent_runtime::SubmissionPlan {
            run_id: result.run_id,
            duplicate: result.duplicate,
        })
    }
    async fn claim(&self, command: &agent_runtime::ClaimCommand) -> Result<RunSnapshot> {
        let conversation = self.conversation_for_run(&command.run_id).await?;
        Ok(ConversationStore::claim(
            self,
            &conversation,
            &command.run_id,
            &command.owner,
            command.now,
            command.ttl,
        )
        .await?
        .snapshot)
    }
    async fn heartbeat(&self, command: &agent_runtime::HeartbeatCommand) -> Result<RunSnapshot> {
        let conversation = self.conversation_for_run(&command.run_id).await?;
        ConversationStore::heartbeat(
            self,
            &conversation,
            &command.run_id,
            &command.lease,
            command.now,
            command.ttl,
        )
        .await
    }
    async fn cancel(
        &self,
        command: &agent_runtime::CancelCommand,
    ) -> Result<agent_runtime::LifecyclePlan> {
        let conversation = self.conversation_for_run(&command.run_id).await?;
        self.cancel_plan(&conversation, &command.run_id, command.now)
            .await
    }
    async fn finish(
        &self,
        command: &agent_runtime::FinishCommand,
    ) -> Result<agent_runtime::LifecyclePlan> {
        let conversation = self.conversation_for_run(&command.run_id).await?;
        self.finish_plan_with_metadata(
            &conversation,
            &command.run_id,
            &command.lease,
            command.now,
            (command.state, &command.content),
            None,
        )
        .await
    }
}
