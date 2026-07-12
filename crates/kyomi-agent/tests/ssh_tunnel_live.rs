//! Live end-to-end SSH-tunnel verification for the datasource pipeline.
//!
//! Proves the full chain that the KYO-124/125/133-137 work depends on:
//! plaintext key → **encrypt at rest** (`finalize_connection_config_secrets`)
//! → **decrypt before the driver** (`decrypt_connection_config_secrets`) →
//! `create_provider` opens the SSH tunnel → connects to Postgres through it.
//!
//! `#[ignore]`d because it needs local infrastructure that CI does not have:
//!   - an SSH bastion reachable at 127.0.0.1:22 that accepts the key at
//!     `/tmp/kyo_tunnel_key` (add its `.pub` to `~/.ssh/authorized_keys`)
//!   - a Postgres server at 127.0.0.1:55432 (db `tundb`, user `postgres`,
//!     password `tuntest`) — e.g. a throwaway container
//!
//! Run explicitly:
//!   cargo test -p kyomi-agent --test ssh_tunnel_live -- --ignored --nocapture

use serde_json::json;
use std::str::FromStr;

#[tokio::test]
#[ignore = "requires local sshd bastion + postgres:55432 + /tmp/kyo_tunnel_key"]
async fn ssh_tunnel_end_to_end_encrypt_decrypt_connect() {
    let enc_key = [7u8; 32]; // deterministic test encryption key
    let private_key_pem =
        std::fs::read_to_string("/tmp/kyo_tunnel_key").expect("tunnel key at /tmp/kyo_tunnel_key");
    let ssh_user = std::env::var("USER").expect("USER env var");

    // 1. connection_config exactly as the modal assembles it (plaintext key).
    let mut config = json!({
        "host": "127.0.0.1",
        "port": 55432,
        "database": "tundb",
        "ssl_mode": "disable",
        "ssh_enabled": true,
        "ssh_host": "127.0.0.1",
        "ssh_port": 22,
        "ssh_username": ssh_user,
        "ssh_private_key": private_key_pem,
    });

    // 2. Encrypt at rest — what create_datasource now does on save.
    kyomi_auth::credential_service::finalize_connection_config_secrets(&mut config, None, &enc_key)
        .expect("finalize (encrypt) should succeed");
    let at_rest = config["ssh_private_key"].as_str().unwrap();
    assert_ne!(
        at_rest, private_key_pem,
        "ssh_private_key must be encrypted at rest, not stored plaintext"
    );

    // 3. Decrypt before the driver — what every provider-build chokepoint does.
    let decrypted = kyomi_auth::credential_service::decrypt_connection_config_secrets(&config, &enc_key);
    assert_eq!(
        decrypted["ssh_private_key"].as_str().unwrap(),
        private_key_pem,
        "decrypt_connection_config_secrets must recover the original plaintext key"
    );

    // 4. Build the provider (opens the SSH tunnel) and connect through it.
    let ds_type = kyomi_core::datasource_registry::DatasourceType::from_str("postgres").unwrap();
    let credentials = json!({ "username": "postgres", "password": "tuntest" });

    let provider =
        kyomi_datasource_server::create_provider(&ds_type, &decrypted, &credentials, None)
            .await
            .expect("provider should build and the SSH tunnel should connect");

    let ok = provider
        .test_connection()
        .await
        .expect("test_connection through the tunnel");
    assert!(ok, "Postgres connection through the SSH tunnel should succeed");
    provider.close().await;
}
