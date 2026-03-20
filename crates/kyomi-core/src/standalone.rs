// SPDX-License-Identifier: AGPL-3.0-or-later

//! Standalone mode configuration for single-binary deployments.
//!
//! When `DATABASE_URL` is not set, Kyomi operates in standalone mode using
//! SQLite and auto-generated secrets stored in a `config.toml` file.

use std::path::{Path, PathBuf};

/// Returns the data directory path from `DATA_DIR` env or defaults to `./data/`.
pub fn data_dir() -> PathBuf {
    std::env::var("DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./data"))
}

/// Auto-generated secrets for standalone mode, persisted in `config.toml`.
pub struct StandaloneConfig {
    pub jwt_secret: String,
    pub encryption_key: String,
}

/// Loads `config.toml` from the data directory, or creates it with random secrets.
///
/// The data directory must already exist (caller should `create_dir_all` first).
/// File permissions are set to `0o600` (owner read/write only) for security.
pub fn load_or_create_config(
    data_dir: &Path,
) -> Result<StandaloneConfig, Box<dyn std::error::Error>> {
    let config_path = data_dir.join("config.toml");

    if config_path.exists() {
        let content = std::fs::read_to_string(&config_path)?;
        let table: toml::Table = content.parse()?;

        let jwt_secret = table
            .get("jwt_secret")
            .and_then(|v| v.as_str())
            .ok_or("missing jwt_secret in config.toml")?
            .to_string();

        let encryption_key = table
            .get("encryption_key")
            .and_then(|v| v.as_str())
            .ok_or("missing encryption_key in config.toml")?
            .to_string();

        Ok(StandaloneConfig {
            jwt_secret,
            encryption_key,
        })
    } else {
        use base64::Engine;
        use rand::Rng;

        let mut rng = rand::rng();

        let jwt_bytes: [u8; 32] = rng.random();
        let enc_bytes: [u8; 32] = rng.random();
        let jwt_secret = base64::engine::general_purpose::URL_SAFE.encode(jwt_bytes);
        let encryption_key = base64::engine::general_purpose::URL_SAFE.encode(enc_bytes);

        let content = format!(
            "# Auto-generated secrets for Kyomi standalone mode\n\
             # WARNING: Do not share this file or commit it to version control\n\n\
             jwt_secret = \"{jwt_secret}\"\n\
             encryption_key = \"{encryption_key}\"\n"
        );

        std::fs::write(&config_path, &content)?;

        // Set file permissions to 0o600 (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(
                &config_path,
                std::fs::Permissions::from_mode(0o600),
            )?;
        }

        Ok(StandaloneConfig {
            jwt_secret,
            encryption_key,
        })
    }
}
