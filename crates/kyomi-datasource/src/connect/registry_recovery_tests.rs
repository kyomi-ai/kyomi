// SPDX-License-Identifier: AGPL-3.0-or-later

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use super::*;

fn scope() -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("kyomi-recovery-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    directory
}

struct CrashOwner {
    child: Child,
    owner: String,
}
impl CrashOwner {
    fn start(directory: &Path) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "connect::registry::recovery_tests::crash_owner_process",
                "--nocapture",
            ])
            .env("KYOMI_CRASH_OWNER_FIXTURE_DIR", directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let owner = BufReader::new(stdout)
            .lines()
            .map(|line| line.unwrap())
            .find_map(|line| line.strip_prefix("OWNER=").map(str::to_owned))
            .expect("child must publish its locked owner before blocking");
        Self { child, owner }
    }
    fn crash(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}
impl Drop for CrashOwner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// Subprocess fixture, invoked explicitly by CrashOwner. Its process-lifetime
// lock remains held until SIGKILL; no graceful unregister or registry Drop runs.
#[test]
fn crash_owner_process() {
    let Some(directory) = std::env::var_os("KYOMI_CRASH_OWNER_FIXTURE_DIR") else {
        return;
    };
    let recovery = OwnerRecovery::for_process(Path::new(&directory)).unwrap();
    println!("OWNER={}", recovery.new_socket_owner());
    use std::io::{Read, Write};
    std::io::stdout().flush().unwrap();
    let mut byte = [0];
    let _ = std::io::stdin().read(&mut byte);
}

async fn redis() -> (RedisPool, String) {
    redis_on_database(0).await
}

async fn redis_on_database(database: u8) -> (RedisPool, String) {
    let url = format!("redis://localhost:6380/{database}");
    let pool = kyomi_core::redis::create_pool(&url)
        .await
        .expect("recovery tests require test Redis on localhost:6380");
    (pool, url)
}

async fn count(redis: &RedisPool, key: &str) -> i64 {
    redis::cmd("SCARD")
        .arg(key)
        .query_async(&mut redis.clone())
        .await
        .unwrap()
}

async fn add_crash_owner(redis: &RedisPool, key: &str, owner: &str) {
    redis::cmd("SADD")
        .arg(key)
        .arg(owner)
        .query_async::<i64>(&mut redis.clone())
        .await
        .unwrap();
}

#[tokio::test]
async fn crash_on_remote_scope_recovers_automatically_without_writer_clearing_live_replica() {
    // This test exercises the real one-page-per-second startup worker. Reserve
    // an empty database so unrelated active sets cannot consume its deadline;
    // notably the pagination regression deliberately creates >1000 owners.
    // Never FLUSH a shared Redis: fail the fixture precondition instead.
    let (redis, url) = redis_on_database(15).await;
    let keys: i64 = redis::cmd("DBSIZE")
        .query_async(&mut redis.clone())
        .await
        .unwrap();
    assert_eq!(
        keys, 0,
        "automatic worker test requires reserved empty Redis database 15"
    );
    let crashed_scope = scope();
    let writer_scope = scope();
    let mut crashed = CrashOwner::start(&crashed_scope);
    let dsid = format!("ds-recovery-{}", uuid::Uuid::new_v4());
    let key = active_generation_key(&dsid, "old");
    add_crash_owner(&redis, &key, &crashed.owner).await;
    let live = ConnectRegistry::new(redis.clone(), url.clone())
        .with_owner_recovery(&writer_scope)
        .unwrap();
    let (tx, _blocked_rx) = mpsc::channel(1);
    let (live_id, _watch) = live.register_authenticated(&dsid, "old", tx).await.unwrap();
    crashed.crash();
    // A cannot verify B, even after B died. Both owners remain observable.
    live.recover_owner_page(&key, 0).await.unwrap();
    assert_eq!(count(&redis, &key).await, 2);
    // A replacement in B's scope starts its worker; no mutation is called there.
    let replacement = ConnectRegistry::new(redis.clone(), url)
        .with_owner_recovery(&crashed_scope)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while count(&redis, &key).await != 1 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("local scope worker must recover the crashed remote owner");
    assert!(
        replacement.revoke_generation(&dsid, "old").await.is_err(),
        "live A socket cannot be erased by B's worker"
    );
    assert_eq!(count(&redis, &key).await, 1);
    live.unregister(&dsid, live_id).await;
    replacement.revoke_generation(&dsid, "old").await.unwrap();
    assert!(replacement.is_revoked(&dsid, "old").await.unwrap());
    redis::cmd("DEL")
        .arg(revoked_key(&dsid, "old"))
        .query_async::<i64>(&mut redis.clone())
        .await
        .unwrap();
    let remaining: i64 = redis::cmd("DBSIZE")
        .query_async(&mut redis.clone())
        .await
        .unwrap();
    assert_eq!(remaining, 0, "fixture must clean only its own keys");
}

#[tokio::test]
async fn verified_crash_cleanup_allows_revocation_and_retains_revoked_marker() {
    let (redis, url) = redis().await;
    let directory = scope();
    let mut crashed = CrashOwner::start(&directory);
    let dsid = format!("ds-recovery-{}", uuid::Uuid::new_v4());
    let key = active_generation_key(&dsid, "old");
    add_crash_owner(&redis, &key, &crashed.owner).await;
    let mut writer = ConnectRegistry::new(redis.clone(), url);
    // Direct wiring isolates the targeted recovery path from the periodic worker.
    writer.owner_recovery = Some(OwnerRecovery::for_process(&directory).unwrap());
    writer.recover_owner_page(&key, 0).await.unwrap();
    assert_eq!(
        count(&redis, &key).await,
        1,
        "a blocked live process retains membership"
    );
    crashed.crash();
    writer.revoke_generation(&dsid, "old").await.unwrap();
    assert_eq!(count(&redis, &key).await, 0);
    assert!(writer.is_revoked(&dsid, "old").await.unwrap());
    let (tx, _rx) = mpsc::channel(1);
    assert!(
        writer
            .register_authenticated(&dsid, "old", tx)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn blocked_send_and_same_generation_reconnect_remain_visible_after_presence_loss() {
    let (redis, url) = redis().await;
    let directory = scope();
    let mut holder = ConnectRegistry::new(redis.clone(), url.clone());
    holder.owner_recovery = Some(OwnerRecovery::for_process(&directory).unwrap());
    let dsid = format!("ds-recovery-{}", uuid::Uuid::new_v4());
    let key = active_generation_key(&dsid, "old");
    let (tx, _blocked_old_send) = mpsc::channel(1);
    let (old_id, _old_watch) = holder
        .register_authenticated(&dsid, "old", tx)
        .await
        .unwrap();
    let (tx, _blocked_new_send) = mpsc::channel(1);
    let (new_id, _new_watch) = holder
        .register_authenticated(&dsid, "old", tx)
        .await
        .unwrap();
    redis::cmd("DEL")
        .arg(presence_key(&dsid))
        .query_async::<i64>(&mut redis.clone())
        .await
        .unwrap();
    let ttl: i64 = redis::cmd("TTL")
        .arg(&key)
        .query_async(&mut redis.clone())
        .await
        .unwrap();
    assert_eq!(ttl, -1);
    let mut writer = ConnectRegistry::new(redis.clone(), url);
    writer.owner_recovery = holder.owner_recovery.clone();
    tokio::time::sleep(Duration::from_secs(61)).await;
    writer.recover_owner_page(&key, 0).await.unwrap();
    assert_eq!(
        count(&redis, &key).await,
        2,
        "live owners outlast the presence heartbeat lease"
    );
    // Longer than the complete revocation confirmation window: no time-based
    // disappearance can allow this socket to flush a command after success.
    assert!(writer.revoke_generation(&dsid, "old").await.is_err());
    assert_eq!(count(&redis, &key).await, 2);
    holder.unregister(&dsid, old_id).await;
    assert!(holder.connections.contains_key(&dsid));
    assert_eq!(count(&redis, &key).await, 1);
    holder.unregister(&dsid, new_id).await;
    writer.revoke_generation(&dsid, "old").await.unwrap();
}

#[tokio::test]
async fn delayed_dead_owner_cleanup_never_removes_replacement_generation_or_presence() {
    let (redis, url) = redis().await;
    let directory = scope();
    let mut crashed = CrashOwner::start(&directory);
    let dsid = format!("ds-recovery-{}", uuid::Uuid::new_v4());
    let old_key = active_generation_key(&dsid, "old");
    add_crash_owner(&redis, &old_key, &crashed.owner).await;
    let mut holder = ConnectRegistry::new(redis.clone(), url);
    holder.owner_recovery = Some(OwnerRecovery::for_process(&directory).unwrap());
    let (tx, _rx) = mpsc::channel(1);
    let (new_id, _watch) = holder
        .register_authenticated(&dsid, "new", tx)
        .await
        .unwrap();
    let replacement_owner = holder.connections.get(&dsid).unwrap().owner.clone();
    crashed.crash();
    holder.revoke_generation(&dsid, "old").await.unwrap();
    // Delayed SADD from the crashed process (e.g. lost response before Redis
    // outage) still carries its old process identity, so remains recoverable.
    add_crash_owner(&redis, &old_key, &crashed.owner).await;
    holder.recover_owner_page(&old_key, 0).await.unwrap();
    assert_eq!(count(&redis, &old_key).await, 0);
    assert_eq!(count(&redis, &active_generation_key(&dsid, "new")).await, 1);
    let presence: String = redis::cmd("GET")
        .arg(presence_key(&dsid))
        .query_async(&mut redis.clone())
        .await
        .unwrap();
    assert_eq!(presence, presence_value(&replacement_owner, "new"));
    assert!(!holder.is_revoked(&dsid, "new").await.unwrap());
    holder.unregister(&dsid, new_id).await;
}

#[tokio::test]
async fn redis_outage_cannot_hide_live_owner_and_recovery_resumes_after_reconnection() {
    let (redis, url) = redis().await;
    let directory = scope();
    let mut holder = ConnectRegistry::new(redis.clone(), url);
    holder.owner_recovery = Some(OwnerRecovery::for_process(&directory).unwrap());
    let dsid = format!("ds-recovery-{}", uuid::Uuid::new_v4());
    let key = active_generation_key(&dsid, "old");
    let (tx, _blocked_send) = mpsc::channel(1);
    let (id, _watch) = holder
        .register_authenticated(&dsid, "old", tx)
        .await
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_url = format!("redis://{}", listener.local_addr().unwrap());
    let proxy = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            connections.spawn(async move {
                let mut upstream = tokio::net::TcpStream::connect("127.0.0.1:6380")
                    .await
                    .unwrap();
                let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
            });
        }
    });
    let client = redis::Client::open(proxy_url.as_str()).unwrap();
    let config = redis::aio::ConnectionManagerConfig::new()
        .set_number_of_retries(0)
        .set_connection_timeout(Duration::from_millis(100))
        .set_response_timeout(Duration::from_millis(100));
    let unavailable = redis::aio::ConnectionManager::new_with_config(client, config)
        .await
        .unwrap();
    let mut writer = ConnectRegistry::new(unavailable, proxy_url);
    writer.owner_recovery = holder.owner_recovery.clone();
    proxy.abort();
    assert!(proxy.await.unwrap_err().is_cancelled());
    assert!(writer.revoke_generation(&dsid, "old").await.is_err());
    assert!(writer.recover_owner_page(&key, 0).await.is_err());
    assert_eq!(count(&redis, &key).await, 1);
    // Restore transport: reconnect alone is not proof the blocked send is gone.
    writer.redis = Some(redis.clone());
    writer.recover_owner_page(&key, 0).await.unwrap();
    assert_eq!(count(&redis, &key).await, 1);
    holder.unregister(&dsid, id).await;
    writer.revoke_generation(&dsid, "old").await.unwrap();
}

#[tokio::test]
async fn failed_owner_probe_retains_owner_without_starving_later_scan_pages() {
    let (redis, url) = redis().await;
    let directory = scope();
    let mut crashed = CrashOwner::start(&directory);
    let mut writer = ConnectRegistry::new(redis.clone(), url);
    writer.owner_recovery = Some(OwnerRecovery::for_process(&directory).unwrap());
    let recovery = writer.owner_recovery.as_ref().unwrap();
    let dsid = format!("ds-recovery-{}", uuid::Uuid::new_v4());
    let key = active_generation_key(&dsid, "old");
    let dead_prefix = crashed.owner.rsplit_once(':').unwrap().0;
    let mut error_fields: Vec<_> = crashed.owner.split(':').map(str::to_owned).collect();
    let failed_process = uuid::Uuid::new_v4().to_string();
    error_fields[2] = failed_process.clone();
    // Opening an existing directory as a writable lock file always errors,
    // including under root: no permission assumptions or mocked verifier.
    std::fs::create_dir(directory.join(&failed_process)).unwrap();
    let error_prefix = error_fields[..5].join(":");
    let failed_owner = format!("{error_prefix}:{}", uuid::Uuid::new_v4());
    assert!(recovery.prove_dead(&failed_owner).is_err());
    let mut members = Vec::new();
    for _ in 0..512 {
        members.push(format!("{dead_prefix}:{}", uuid::Uuid::new_v4()));
        members.push(format!("{error_prefix}:{}", uuid::Uuid::new_v4()));
    }
    redis::cmd("SADD")
        .arg(&key)
        .arg(&members)
        .query_async::<i64>(&mut redis.clone())
        .await
        .unwrap();
    assert_eq!(count(&redis, &key).await, 1024);
    let (first_cursor, _): (u64, Vec<String>) = redis::cmd("SSCAN")
        .arg(&key)
        .arg(0)
        .arg("COUNT")
        .arg(64)
        .query_async(&mut redis.clone())
        .await
        .unwrap();
    assert_ne!(
        first_cursor, 0,
        "fixture must exercise actual Redis pagination"
    );
    crashed.crash();
    let mut cursor = 0;
    let mut pages = 0;
    loop {
        cursor = writer.recover_owner_page(&key, cursor).await.unwrap();
        pages += 1;
        assert!(
            pages < 1000,
            "recovery cursor must complete despite persistent failed probes"
        );
        if cursor == 0 {
            break;
        }
    }
    assert!(pages > 1, "recovery must traverse subsequent SSCAN pages");
    let retained: Vec<String> = redis::cmd("SMEMBERS")
        .arg(&key)
        .query_async(&mut redis.clone())
        .await
        .unwrap();
    assert_eq!(
        retained.len(),
        512,
        "all independently dead owners must be recovered"
    );
    assert!(
        retained
            .iter()
            .all(|owner| owner.starts_with(&error_prefix)),
        "every unverified owner must remain visible"
    );
    // Repeat a complete sweep: a persistent failed probe neither disappears
    // nor makes a later sweep report failure/discard progress.
    let mut cursor = 0;
    loop {
        cursor = writer.recover_owner_page(&key, cursor).await.unwrap();
        if cursor == 0 {
            break;
        }
    }
    assert_eq!(count(&redis, &key).await, 512);
    redis::cmd("DEL")
        .arg(&key)
        .query_async::<i64>(&mut redis.clone())
        .await
        .unwrap();
}
