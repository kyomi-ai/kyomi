// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-498: end-to-end regression coverage for "a new chat session must
//! appear in the chats list without a refresh."
//!
//! A prod user reported that starting a conversation, navigating away, and
//! opening the chats list did not show the new chat until a hard refresh.
//! Two root causes were found and fixed separately:
//!
//! - **KYO-490** — `sort_sessions_by_recency` (`pages::chat::chat_list`)
//!   compared `updated_at` as raw bytes, and the two textual formats the
//!   client sees (RFC 3339 from `sync_log` delta replay, Postgres's
//!   `CAST(... AS TEXT)` rendering from bootstrap/live-broadcast) sort
//!   inconsistently byte-for-byte.
//! - **KYO-491** — the JSON snapshot different producers attached to a
//!   `chat_session` sync action was unified so every producer (bootstrap,
//!   delta replay, live broadcast) emits the same complete shape via
//!   `chat_service::fetch_session_snapshot` / `session_snapshot_json`,
//!   rather than each hand-rolling its own partial one.
//!
//! Neither fix shipped with a test that drives the actual path a real
//! browser exercises: server creates a session, broadcasts it over the
//! WebSocket the creator is connected to, and the client's list page sorts
//! its local store including that new entry. That gap is why a prod user
//! found this rather than CI — this module is the coverage the ticket
//! requires "regardless of outcome."
//!
//! Two tests:
//!
//! 1. [`new_session_broadcasts_a_complete_insert_snapshot_to_its_creator_only`]
//!    drives `kyomi_auth::chat_service::prepare_chat_dispatch` for a
//!    brand-new session and asserts the `SyncAction` frame that reaches the
//!    creating user's WebSocket receiver — exactly one, `Insert`, carrying a
//!    complete `ChatSessionItem` — and that a second workspace member never
//!    sees it (a brand-new session is always private).
//! 2. [`new_session_insert_snapshot_sorts_to_top_of_a_populated_store`] takes
//!    the literal `ChatSessionItem` the server just emitted (never a
//!    hand-written fixture — see [`dispatch_new_session_and_capture_insert_snapshot`])
//!    and confirms it sorts first in a `SyncStore` pre-populated with four
//!    older sessions, all anchored to the wire session's own calendar day
//!    and mixing both textual formats, using the page's real
//!    `sort_sessions_by_recency` (`pages::chat::chat_list`, widened to
//!    `pub(crate)` for this test). It asserts both that the new session
//!    sorts first *and* that the four older, mixed-format sessions land in
//!    correct relative order among themselves — the second assertion is
//!    what actually pins the KYO-490 regression; see that test's own doc
//!    comment for why.

use chrono::TimeZone;
use kyomi_auth::chat_service::{ChatDispatchOutcome, ChatDispatchParams, prepare_chat_dispatch};
use kyomi_auth::websocket::WebSocketManager;
use kyomi_core::{DbPool, MessageType, WebSocketMessage};
use kyomi_types::sync::{SyncAction, SyncActionType, entity_types};
use leptos::prelude::*;
use sqlx::sqlite::SqlitePoolOptions;
use tokio::sync::mpsc;

use crate::cache::store::SyncStore;
use crate::pages::chat::chat_list::sort_sessions_by_recency;
use crate::server_fns::chat::ChatSessionItem;

// ── Fixture scaffolding ─────────────────────────────────────────────────────
//
// This is the THIRD independent copy of this scaffolding, not the second.
// `kyomi_auth::test_support::{test_pool, seed_user, seed_workspace,
// seed_membership}` (KYO-271/KYO-368 consolidated it there specifically so
// *kyomi-auth's own* test modules stop hand-rolling it) and
// `kyomi_agent::test_support::test_pool` (KYO-537) already match this
// byte-for-byte in shape — `max_connections(1)`, `PRAGMA foreign_keys=ON`,
// `sqlx::migrate!("../../apps/server/migrations-sqlite")` — and both are
// `pub(crate)` to their own crates, so neither can be imported from here.
// Per
// docs/standards/code-organization/third-copy-of-test-helper-is-extraction-trigger.md
// a third copy is the trigger to extract a shared test-support crate — that
// trigger condition is met by this file. Extracting one spanning
// kyomi-auth, kyomi-agent and kyomi-ui is a real three-crate refactor,
// well outside a test-coverage ticket, so it is declined here as
// out of scope for KYO-498 and tracked separately as KYO-751 ("Third copy
// of the sqlite `test_pool` fixture — kyomi-auth, kyomi-agent and now
// kyomi-ui each hand-roll the identical scaffolding").

/// Build an in-memory SQLite pool with the full server migration chain
/// applied and foreign-key enforcement on. A single connection
/// (`max_connections(1)`) is used because SQLite `:memory:` databases are
/// per-connection — every query in a test must hit the same pool.
async fn test_pool() -> DbPool {
    let _ = kyomi_core::constants::load_with_fallback();

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("connect in-memory sqlite");

    sqlx::query("PRAGMA foreign_keys=ON")
        .execute(&pool)
        .await
        .expect("enable foreign keys");

    // sqlx::migrate! resolves its path relative to CARGO_MANIFEST_DIR
    // (crates/kyomi-ui), not to this source file, so this resolves to
    // apps/server/migrations-sqlite at the repo root.
    sqlx::migrate!("../../apps/server/migrations-sqlite")
        .run(&pool)
        .await
        .expect("run sqlite migrations");

    DbPool::Sqlite(pool)
}

/// Extract the inner `SqlitePool` from a `DbPool` built by [`test_pool`].
fn sqlite_pool(db: &DbPool) -> &sqlx::SqlitePool {
    match db {
        DbPool::Sqlite(sq) => sq,
        DbPool::Postgres(_) => panic!("test requires sqlite pool"),
    }
}

async fn seed_user(sq: &sqlx::SqlitePool, user_id: &str, email: &str) {
    sqlx::query("INSERT INTO users (user_id, email, active) VALUES ($1, $2, 1)")
        .bind(user_id)
        .bind(email)
        .execute(sq)
        .await
        .expect("insert user");
}

async fn seed_workspace(sq: &sqlx::SqlitePool, workspace_id: &str, owner_user_id: &str) {
    sqlx::query("INSERT INTO workspaces (workspace_id, name, owner_user_id) VALUES ($1, $2, $3)")
        .bind(workspace_id)
        .bind(format!("Workspace {workspace_id}"))
        .bind(owner_user_id)
        .execute(sq)
        .await
        .expect("insert workspace");
}

async fn seed_membership(sq: &sqlx::SqlitePool, workspace_id: &str, user_id: &str, role: &str) {
    sqlx::query(
        "INSERT INTO workspace_users (workspace_id, user_id, role, active) \
         VALUES ($1, $2, $3, 1)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .bind(role)
    .execute(sq)
    .await
    .expect("insert membership");
}

/// A fixed AES-256 test key — deliberately not random so encryption is
/// reproducible byte-for-byte. `prepare_chat_dispatch`'s `skip_ai` path
/// stores the user message with it and nothing in these tests reads that
/// message back, so any fixed key is fine.
fn test_key() -> [u8; 32] {
    let mut key = [0u8; 32];
    key[..16].copy_from_slice(b"test-key-1234567");
    key[16..].copy_from_slice(b"8901234567890123");
    key
}

/// Connect `user_id` to `manager` and drain the immediate heartbeat every
/// `connect()` sends, so callers start from a clean receiver.
fn connect_and_drain_heartbeat(manager: &WebSocketManager, user_id: &str) -> mpsc::Receiver<String> {
    let (_conn, mut rx) = manager.connect(user_id).expect("connect");
    assert!(
        rx.try_recv()
            .expect("connect() must send an immediate heartbeat")
            .contains("heartbeat")
    );
    rx
}

/// Drain every currently-buffered message on `rx`, parse each as a
/// `WebSocketMessage`, and return only the `SyncAction` payloads. A
/// `session_created` message is also sent on the new-session path
/// (`chat_service.rs`, immediately before the sync broadcast), so callers
/// must never assume the next message on the channel is the sync action —
/// this filters on `message_type` explicitly instead.
fn drain_sync_actions(rx: &mut mpsc::Receiver<String>) -> Vec<SyncAction> {
    let mut actions = Vec::new();
    while let Ok(raw) = rx.try_recv() {
        let envelope: WebSocketMessage = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("invalid WebSocketMessage JSON: {e}: {raw}"));
        if envelope.message_type != MessageType::SyncAction {
            continue;
        }
        let data = envelope
            .data
            .expect("a sync_action message must carry a `data` field");
        let action: SyncAction = serde_json::from_value(data)
            .expect("sync_action `data` must deserialize as a SyncAction");
        actions.push(action);
    }
    actions
}

fn assert_no_sync_action(rx: &mut mpsc::Receiver<String>, context: &str) {
    let actions = drain_sync_actions(rx);
    assert!(
        actions.is_empty(),
        "{context}: expected no sync action, got {actions:?}"
    );
}

fn expect_single_sync_action(rx: &mut mpsc::Receiver<String>, context: &str) -> SyncAction {
    let mut actions = drain_sync_actions(rx);
    assert_eq!(
        actions.len(),
        1,
        "{context}: expected exactly one sync action, got {actions:?}"
    );
    actions.remove(0)
}

/// Bundled parameters for [`dispatch_new_session_and_capture_insert_snapshot`]
/// — grouped into a struct (mirroring `chat_service::ChatDispatchParams`
/// itself) rather than a long parameter list, per clippy's
/// `too_many_arguments`.
struct NewSessionDispatch<'a> {
    db: &'a DbPool,
    key: &'a [u8; 32],
    manager: &'a WebSocketManager,
    user_id: &'a str,
    workspace_id: &'a str,
    display_name: &'a str,
}

/// Drive the real new-session path — `prepare_chat_dispatch` with
/// `is_new_session: true, skip_ai: true` — and return the `ChatSessionItem`
/// snapshot carried by the resulting `Insert` `SyncAction` sent to
/// `p.user_id`.
///
/// Shared by both tests in this module so the sort test (test 2) exercises
/// the literal wire payload the server emits rather than a hand-written
/// fixture that could drift from it — the exact defect class KYO-491
/// closed (divergent snapshot producers).
async fn dispatch_new_session_and_capture_insert_snapshot(
    p: NewSessionDispatch<'_>,
    rx: &mut mpsc::Receiver<String>,
    session_id: &str,
) -> ChatSessionItem {
    let outcome = prepare_chat_dispatch(ChatDispatchParams {
        db: p.db,
        encryption_key: p.key,
        ws_manager: Some(p.manager),
        user_id: p.user_id,
        workspace_id: p.workspace_id,
        user_display_name: p.display_name,
        session_id,
        is_new_session: true,
        message: "hello",
        current_time_user_tz: None,
        message_source: Some("web"),
        skip_ai: true,
        client_msg_id: None,
    })
    .await
    .expect("prepare_chat_dispatch should succeed for a brand-new session");

    assert!(
        matches!(outcome, ChatDispatchOutcome::SkippedAi { .. }),
        "skip_ai=true must return SkippedAi"
    );

    let action = expect_single_sync_action(rx, "creator's connection after a new-session dispatch");
    assert_eq!(action.entity_type, entity_types::CHAT_SESSION);
    assert!(
        matches!(action.action, SyncActionType::Insert),
        "a brand-new session must broadcast an Insert, got {:?}",
        action.action
    );

    let data = action
        .data
        .expect("an Insert sync action must never carry data: None (KYO-218) — the client can't tell it apart from a Delete");
    serde_json::from_value(data)
        .expect("the Insert snapshot must deserialize as a ChatSessionItem")
}

// ── Test 1 — the server emits the Insert to the creator, complete ─────────

/// KYO-498 / KYO-491: the creator of a brand-new session must receive
/// exactly one `chat_session` `Insert` `SyncAction`, carrying a complete
/// snapshot — and a second workspace member, since a brand-new session is
/// always private, must receive none at all.
///
/// "Complete" here means the specific fields KYO-491's producer unification
/// exists to guarantee. Every field this struct doesn't receive from the
/// wire is `Option`/`#[serde(default)]`
/// (except `created_at`/`updated_at`, checked separately below), so a
/// partial, pre-KYO-491-style snapshot would deserialize *successfully*
/// into a degraded `ChatSessionItem` rather than fail loudly — that silent
/// degradation is exactly why the field-by-field assertions below are the
/// point of this test, not a formality on top of "it deserialized".
#[tokio::test]
async fn new_session_broadcasts_a_complete_insert_snapshot_to_its_creator_only() {
    let db = test_pool().await;
    let sq = sqlite_pool(&db);
    let key = test_key();
    seed_user(sq, "creator", "creator@test.local").await;
    seed_user(sq, "other", "other@test.local").await;
    seed_workspace(sq, "ws-1", "creator").await;
    seed_membership(sq, "ws-1", "creator", "workspace_admin").await;
    seed_membership(sq, "ws-1", "other", "user").await;

    let manager = WebSocketManager::new(None, db.clone());
    let mut rx_creator = connect_and_drain_heartbeat(&manager, "creator");
    let mut rx_other = connect_and_drain_heartbeat(&manager, "other");

    let client_sid = uuid::Uuid::new_v4().to_string();
    let item = dispatch_new_session_and_capture_insert_snapshot(
        NewSessionDispatch {
            db: &db,
            key: &key,
            manager: &manager,
            user_id: "creator",
            workspace_id: "ws-1",
            display_name: "Creator",
        },
        &mut rx_creator,
        &client_sid,
    )
    .await;

    assert_no_sync_action(
        &mut rx_other,
        "a brand-new session is always private; a second workspace member must not see it",
    );

    assert_eq!(
        item.session_id, client_sid,
        "the snapshot must carry the client-generated session id, not a server-assigned one \
         (KYO-494 — the client has no other identity to filter its own WS stream on)"
    );
    assert_eq!(
        item.session_type.as_deref(),
        Some("chat"),
        "session_type must be populated, not dropped by a partial snapshot"
    );
    assert!(!item.shared, "a brand-new session must never start out shared");

    // The Insert broadcast (chat_service.rs, ~line 2107) fires immediately
    // after session creation and *before* the user message row is written
    // (add_message runs afterward, ~line 2149) — so message_count == 0 here
    // is legitimately correct, not a bug. Asserting a nonzero count would
    // pin behavior the code cannot have produced at this point.
    assert_eq!(
        item.message_count, 0,
        "message_count must be 0 at Insert time — the broadcast happens before the user \
         message is persisted"
    );

    let created_by = item.created_by.as_ref().expect(
        "KYO-491: created_by must be Some — a None here is exactly the pre-unification \
         degradation this ticket guards against",
    );
    assert_eq!(
        created_by.user_id, "creator",
        "created_by must name the actual creating user"
    );

    assert!(!item.created_at.is_empty(), "created_at must not be empty");
    assert!(!item.updated_at.is_empty(), "updated_at must not be empty");
    assert!(
        crate::utils::time::parse_timestamp(&item.created_at).is_some(),
        "created_at must parse via crate::utils::time::parse_timestamp: {:?}",
        item.created_at
    );
    assert!(
        crate::utils::time::parse_timestamp(&item.updated_at).is_some(),
        "updated_at must parse via crate::utils::time::parse_timestamp — an unparseable \
         value sorts the session to the bottom of the list (see sort_sessions_by_recency), \
         which reads to a user as \"my new chat isn't there\": {:?}",
        item.updated_at
    );
}

// ── Test 2 — that wire snapshot sorts to the top of a populated store ─────

/// A minimal older `ChatSessionItem` with only `session_id`/`updated_at`
/// set — the two fields `sort_sessions_by_recency` reads.
fn make_older_session(session_id: &str, updated_at: &str) -> ChatSessionItem {
    ChatSessionItem {
        session_id: session_id.to_string(),
        title: None,
        model: None,
        session_type: Some("chat".to_string()),
        shared: false,
        shared_at: None,
        created_at: updated_at.to_string(),
        updated_at: updated_at.to_string(),
        message_count: 0,
        pinned_count: 0,
        unread_count: 0,
        created_by: None,
        slack_channel_id: None,
    }
}

/// KYO-498 / KYO-490: the `ChatSessionItem` a brand-new session's real
/// `Insert` sync action carries (produced by the exact same dispatch path
/// test 1 drives — never a hand-written fixture, see
/// [`dispatch_new_session_and_capture_insert_snapshot`]) must sort to the
/// top of a `SyncStore` already holding older sessions, once the page's
/// real `sort_sessions_by_recency` (`pages::chat::chat_list`) is applied.
///
/// Every older fixture's `updated_at` is *derived from the wire session's
/// own `updated_at`* — parsed, then offset backward by a fraction of the
/// elapsed time since that day's midnight — and rendered in both textual
/// formats the client tolerates: RFC 3339 (`sync_log` delta replay, `T`
/// separator) and Postgres's `CAST(... AS TEXT)` rendering
/// (bootstrap/live-broadcast, space separator). Anchoring to the wire
/// timestamp's own calendar day is deliberate: it isolates the format
/// separator as the *only* thing that can differ first between two of the
/// fixtures' string representations. An earlier version of this test used
/// hand-written `2026-08` dates against a wire timestamp that (being
/// `Utc::now()`) always lands in the current month — so the month digit
/// alone decided the byte-wise order, the actual separator defect
/// (`0x20` vs `0x54`) was never reached, and the test kept passing after
/// `sort_sessions_by_recency` was reverted to a raw byte-wise
/// `Reverse(s.updated_at.clone())` compare.
///
/// `postgres_recent` is deliberately more recent (closer to `now`) than
/// `rfc3339_recent`, despite both sharing `now`'s calendar date — that
/// pairing is exactly what a byte-wise compare inverts, since `T` (0x54)
/// sorts above space (0x20) regardless of the actual clock time that
/// follows it. The assertions below check both that the new session sorts
/// first *and* that the four older, mixed-format fixtures land in their
/// correct real-chronological order relative to each other — the second
/// check is what actually fails under a byte-wise-compare mutation; the
/// first alone does not.
#[tokio::test]
async fn new_session_insert_snapshot_sorts_to_top_of_a_populated_store() {
    let db = test_pool().await;
    let sq = sqlite_pool(&db);
    let key = test_key();
    seed_user(sq, "creator", "creator@test.local").await;
    seed_workspace(sq, "ws-1", "creator").await;
    seed_membership(sq, "ws-1", "creator", "workspace_admin").await;

    let manager = WebSocketManager::new(None, db.clone());
    let mut rx_creator = connect_and_drain_heartbeat(&manager, "creator");

    let client_sid = uuid::Uuid::new_v4().to_string();
    let new_item = dispatch_new_session_and_capture_insert_snapshot(
        NewSessionDispatch {
            db: &db,
            key: &key,
            manager: &manager,
            user_id: "creator",
            workspace_id: "ws-1",
            display_name: "Creator",
        },
        &mut rx_creator,
        &client_sid,
    )
    .await;

    let now = crate::utils::time::parse_timestamp(&new_item.updated_at).expect(
        "the wire session's own updated_at must parse via crate::utils::time::parse_timestamp, \
         or sort_sessions_by_recency could never place it correctly in production",
    );

    // Local midnight on `now`'s own calendar day, in `now`'s own offset.
    // Every older fixture below is a fraction of the elapsed time between
    // this and `now`, which guarantees each fixture is strictly older than
    // `now` and shares its calendar date, however long this test binary has
    // been running.
    let midnight = now
        .offset()
        .from_local_datetime(
            &now.date_naive()
                .and_hms_opt(0, 0, 0)
                .expect("midnight is a valid time of day"),
        )
        .single()
        .expect("a FixedOffset local midnight always converts unambiguously");
    let elapsed = now - midnight;
    let at = |numerator: i32, denominator: i32| midnight + elapsed * numerator / denominator;

    // RFC 3339 — `sync_log` delta-replay form.
    let rfc3339 = |dt: chrono::DateTime<chrono::FixedOffset>| dt.to_rfc3339();
    // Postgres `CAST(... AS TEXT)` — bootstrap/live-broadcast form. `now`
    // (and therefore every fixture derived from it) is always UTC — the
    // server writes `row.updated_at.to_rfc3339()` from a
    // `chrono::DateTime<chrono::Utc>` (`chat_service.rs`) — so the
    // whole-hour Postgres offset is always `+00`.
    let postgres = |dt: chrono::DateTime<chrono::FixedOffset>| dt.format("%Y-%m-%d %H:%M:%S+00").to_string();

    let owner = Owner::new();
    owner.set();

    let store = SyncStore::new();
    // Four older sessions anchored to `now`'s own day. Real chronological
    // order, oldest to newest: rfc3339_oldest < postgres_older <
    // rfc3339_recent < postgres_recent < new_item (`now`).
    store.upsert_chat_session(make_older_session("rfc3339-oldest", &rfc3339(at(1, 8))));
    store.upsert_chat_session(make_older_session("postgres-older", &postgres(at(2, 8))));
    store.upsert_chat_session(make_older_session("rfc3339-recent", &rfc3339(at(6, 8))));
    store.upsert_chat_session(make_older_session("postgres-recent", &postgres(at(7, 8))));

    store.upsert_chat_session(new_item);

    let mut sessions = store.chat_sessions().get_untracked();
    sort_sessions_by_recency(&mut sessions);

    assert_eq!(
        sessions.first().map(|s| s.session_id.as_str()),
        Some(client_sid.as_str()),
        "the brand-new session must sort first among older sessions in mixed timestamp \
         formats, using the page's real sort_sessions_by_recency: {sessions:?}"
    );

    let older_ids: Vec<&str> = sessions[1..].iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(
        older_ids,
        vec!["postgres-recent", "rfc3339-recent", "postgres-older", "rfc3339-oldest"],
        "the four older, mixed-format sessions must also sort in real chronological order among \
         themselves — a byte-wise compare inverts postgres-recent/rfc3339-recent specifically, \
         since they share a calendar date and `T` (0x54) outranks space (0x20) before either \
         string's clock digits are ever compared: {sessions:?}"
    );
}
