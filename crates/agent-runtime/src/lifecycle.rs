//! Pure lifecycle policy. Every snapshot and submission passed here must be read under
//! the adapter's transaction lock; plans are committed with journal/projection writes.
use crate::{
    AppendCommand, IdempotencyKey, MessageId, Payload, PolicyError, Projection, PublicPayload,
    RunId, RunState, validate,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorIdentity {
    pub actor_id: String,
    pub source: String,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubmitCommand {
    pub submitted: AppendCommand,
    pub assistant_message_id: MessageId,
    pub request_id: IdempotencyKey,
    pub context: serde_json::Value,
    pub actor: ActorIdentity,
    pub submitted_at: i64,
    pub new_conversation: bool,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmissionPlan {
    pub run_id: RunId,
    pub duplicate: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lease {
    pub owner: String,
    pub fence: i64,
    pub expires_at: i64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub state: RunState,
    pub lease: Option<Lease>,
    pub cancellation_requested: bool,
    pub assistant_message_id: MessageId,
    /// Last allocated fence; retained after releasing ownership.
    pub fence: i64,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub terminal_at: Option<i64>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct LifecyclePlan {
    pub snapshot: RunSnapshot,
    pub terminal_projection: Option<Projection>,
    pub changed: bool,
}
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LifecycleError {
    #[error(transparent)]
    Policy(#[from] PolicyError),
    #[error("run is not claimable")]
    NotClaimable,
    #[error("conversation already has an active run")]
    ConversationBusy,
    #[error("lease ownership is stale or expired")]
    StaleLease,
    #[error("invalid lease duration, timestamp, or fence")]
    InvalidLease,
}
fn identity(id: &str) -> Result<(), LifecycleError> {
    if id.is_empty() || id.len() > 128 {
        return Err(PolicyError::InvalidIdentity.into());
    }
    Ok(())
}
/// The lookup is scoped to the authorized application scope and request ID. Generated
/// message/run/event IDs and server acceptance time are not request payload identity.
pub fn plan_submission(
    command: &SubmitCommand,
    existing: Option<&SubmitCommand>,
) -> Result<SubmissionPlan, LifecycleError> {
    validate(&command.submitted)?;
    identity(command.request_id.as_str())?;
    identity(command.assistant_message_id.as_str())?;
    identity(&command.actor.actor_id)?;
    identity(&command.actor.source)?;
    let Payload::Public(PublicPayload::Submitted { text, .. }) = &command.submitted.payload else {
        return Err(PolicyError::InvalidSubmission.into());
    };
    if serde_json::to_vec(&command.context)
        .map_err(|e| PolicyError::Serialization(e.to_string()))?
        .len()
        > crate::MAX_DETAIL_BYTES
    {
        return Err(PolicyError::TooLarge.into());
    }
    if let Some(saved) = existing {
        let Payload::Public(PublicPayload::Submitted {
            text: saved_text, ..
        }) = &saved.submitted.payload
        else {
            return Err(PolicyError::InvalidSubmission.into());
        };
        // Detail references are generated envelope IDs; compare the actual exposed body.
        let body = command.submitted.detail.as_deref().unwrap_or(&text.preview);
        let saved_body = saved
            .submitted
            .detail
            .as_deref()
            .unwrap_or(&saved_text.preview);
        if command.request_id != saved.request_id
            || command.submitted.conversation_id != saved.submitted.conversation_id
            || command.actor != saved.actor
            || command.context != saved.context
            || body != saved_body
            || command.new_conversation != saved.new_conversation
        {
            return Err(PolicyError::IdempotencyConflict.into());
        }
        return Ok(SubmissionPlan {
            run_id: saved.submitted.run_id.clone(),
            duplicate: true,
        });
    }
    Ok(SubmissionPlan {
        run_id: command.submitted.run_id.clone(),
        duplicate: false,
    })
}
fn expiry(now: i64, ttl: i64) -> Result<i64, LifecycleError> {
    if ttl <= 0 {
        return Err(LifecycleError::InvalidLease);
    }
    now.checked_add(ttl).ok_or(LifecycleError::InvalidLease)
}
pub fn plan_claim(
    snapshot: &RunSnapshot,
    owner: &str,
    now: i64,
    ttl: i64,
    conversation_active: bool,
) -> Result<RunSnapshot, LifecycleError> {
    identity(owner)?;
    let expires_at = expiry(now, ttl)?;
    if snapshot.state != RunState::Queued || snapshot.cancellation_requested {
        return Err(LifecycleError::NotClaimable);
    }
    if conversation_active {
        return Err(LifecycleError::ConversationBusy);
    }
    let fence = snapshot
        .fence
        .checked_add(1)
        .filter(|f| *f > 0)
        .ok_or(LifecycleError::InvalidLease)?;
    let mut next = snapshot.clone();
    next.state = RunState::Running;
    next.fence = fence;
    next.lease = Some(Lease {
        owner: owner.into(),
        fence,
        expires_at,
    });
    next.started_at = Some(now);
    Ok(next)
}
/// Heartbeat expiry changes do not change the fencing identity. Callers may retain
/// their original lease token while the adapter stores renewed expiration timestamps.
pub fn validate_ownership(
    snapshot: &RunSnapshot,
    lease: &Lease,
    now: i64,
) -> Result<(), LifecycleError> {
    if snapshot.state != RunState::Running
        || !snapshot.lease.as_ref().is_some_and(|current| {
            current.owner == lease.owner
                && current.fence == lease.fence
                && current.fence == snapshot.fence
                && current.expires_at > now
        })
    {
        return Err(LifecycleError::StaleLease);
    }
    Ok(())
}
pub fn plan_heartbeat(
    snapshot: &RunSnapshot,
    lease: &Lease,
    now: i64,
    ttl: i64,
) -> Result<RunSnapshot, LifecycleError> {
    validate_ownership(snapshot, lease, now)?;
    let expires_at = expiry(now, ttl)?;
    let mut next = snapshot.clone();
    if let Some(current) = &mut next.lease {
        current.expires_at = current.expires_at.max(expires_at);
    }
    Ok(next)
}
fn unchanged(snapshot: &RunSnapshot) -> LifecyclePlan {
    LifecyclePlan {
        snapshot: snapshot.clone(),
        terminal_projection: None,
        changed: false,
    }
}
fn terminal(snapshot: &RunSnapshot, state: RunState, now: i64, content: &str) -> LifecyclePlan {
    let mut next = snapshot.clone();
    next.state = state;
    next.lease = None;
    next.terminal_at = Some(now);
    LifecyclePlan {
        snapshot: next,
        terminal_projection: Some(Projection {
            message_id: snapshot.assistant_message_id.clone(),
            role: "assistant",
            content: content.into(),
            status: match state {
                RunState::Completed => "complete",
                RunState::Failed => "error",
                _ => state.as_str(),
            },
        }),
        changed: true,
    }
}
pub fn plan_cancel(snapshot: &RunSnapshot, now: i64) -> Result<LifecyclePlan, LifecycleError> {
    if snapshot.state.is_terminal() {
        return Ok(unchanged(snapshot));
    }
    let mut next = snapshot.clone();
    next.cancellation_requested = true;
    if snapshot.state == RunState::Queued {
        return Ok(terminal(&next, RunState::Cancelled, now, "Cancelled"));
    }
    Ok(LifecyclePlan {
        snapshot: next,
        terminal_projection: None,
        changed: !snapshot.cancellation_requested,
    })
}
pub fn plan_finalize(
    snapshot: &RunSnapshot,
    lease: &Lease,
    now: i64,
    state: RunState,
    content: &str,
) -> Result<LifecyclePlan, LifecycleError> {
    validate_ownership(snapshot, lease, now)?;
    if !state.is_terminal() || state == RunState::Interrupted {
        return Err(PolicyError::InvalidState.into());
    }
    if content.len() > crate::MAX_DETAIL_BYTES {
        return Err(PolicyError::TooLarge.into());
    }
    if snapshot.cancellation_requested {
        return Ok(terminal(snapshot, RunState::Cancelled, now, "Cancelled"));
    }
    Ok(terminal(snapshot, state, now, content))
}
/// The adapter may determine under its transaction lock that queued work can no
/// longer be authorized. Terminalize that work so it cannot block subsequent turns.
/// This does not revoke a live owner's lease or reinterpret an existing terminal run.
pub fn plan_interrupt_queued(
    snapshot: &RunSnapshot,
    now: i64,
    reason: &str,
) -> Result<LifecyclePlan, LifecycleError> {
    if snapshot.state != RunState::Queued {
        return Ok(unchanged(snapshot));
    }
    if reason.len() > crate::MAX_DETAIL_BYTES {
        return Err(PolicyError::TooLarge.into());
    }
    Ok(terminal(snapshot, RunState::Interrupted, now, reason))
}

/// Expired work is interrupted, never reclaimed for a second model execution. Queued
/// work remains claimable. Live ownership on another replica remains untouched.
pub fn plan_expire(snapshot: &RunSnapshot, now: i64) -> Result<LifecyclePlan, LifecycleError> {
    if snapshot.state != RunState::Running {
        return Ok(unchanged(snapshot));
    }
    let lease = snapshot
        .lease
        .as_ref()
        .ok_or(LifecycleError::InvalidLease)?;
    if lease.expires_at > now {
        return Ok(unchanged(snapshot));
    }
    if snapshot.cancellation_requested {
        return Ok(terminal(snapshot, RunState::Cancelled, now, "Cancelled"));
    }
    Ok(terminal(
        snapshot,
        RunState::Interrupted,
        now,
        "Interrupted",
    ))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClaimCommand {
    pub run_id: RunId,
    pub owner: String,
    pub now: i64,
    pub ttl: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HeartbeatCommand {
    pub run_id: RunId,
    pub lease: Lease,
    pub now: i64,
    pub ttl: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CancelCommand {
    pub run_id: RunId,
    pub now: i64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FinishCommand {
    pub run_id: RunId,
    pub lease: Lease,
    pub now: i64,
    pub state: RunState,
    pub content: String,
}
/// Lifecycle operations extend the same journal/projection atomic transaction contract
/// as AtomicPersistence; implementations must not create a separate persistence path.
#[async_trait::async_trait]
pub trait AtomicLifecyclePersistence: Send + Sync {
    type Error: Send;
    async fn submit(&self, command: &SubmitCommand) -> Result<SubmissionPlan, Self::Error>;
    async fn claim(&self, command: &ClaimCommand) -> Result<RunSnapshot, Self::Error>;
    async fn heartbeat(&self, command: &HeartbeatCommand) -> Result<RunSnapshot, Self::Error>;
    async fn cancel(&self, command: &CancelCommand) -> Result<LifecyclePlan, Self::Error>;
    async fn finish(&self, command: &FinishCommand) -> Result<LifecyclePlan, Self::Error>;
}
