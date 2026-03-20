// SPDX-License-Identifier: AGPL-3.0-or-later

//! Desktop app mode persistence and selector.
//!
//! On first launch, the user picks "Run Locally" (personal/local) or
//! "Connect to Server" (thin client to a remote URL). The choice
//! is saved to `mode.json` in the app data directory.

use std::path::PathBuf;

/// How the desktop app should operate.
#[derive(Debug, Clone)]
pub enum AppMode {
    /// First launch — no mode chosen yet. Show the selector.
    FirstLaunch,
    /// Personal mode — run the full embedded backend locally.
    Personal,
    /// Remote mode — thin webview client pointed at a server URL.
    Remote { url: String },
}

/// Path to the mode config file.
fn mode_file() -> PathBuf {
    let data_dir = dirs::data_dir()
        .map(|d| d.join("ai.kyomi.desktop"))
        .unwrap_or_else(|| PathBuf::from("./data"));
    data_dir.join("mode.json")
}

/// Load the saved mode, or return `FirstLaunch` if none exists.
///
/// If `mode.json` doesn't exist but `config.toml` does (existing personal
/// install from before mode selector was added), default to Personal.
pub fn load_mode() -> AppMode {
    let path = mode_file();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            // Check for existing personal install (has config.toml but no mode.json)
            if let Some(parent) = path.parent() {
                if parent.join("config.toml").exists() {
                    return AppMode::Personal;
                }
            }
            return AppMode::FirstLaunch;
        }
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return AppMode::FirstLaunch,
    };

    match json.get("mode").and_then(|v| v.as_str()) {
        Some("personal") => AppMode::Personal,
        Some("remote") => {
            let url = json
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("https://app.kyomi.ai")
                .to_string();
            AppMode::Remote { url }
        }
        _ => AppMode::FirstLaunch,
    }
}

/// Save the chosen mode to disk.
pub fn save_mode(mode: &AppMode) {
    let path = mode_file();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let json = match mode {
        AppMode::Personal => serde_json::json!({ "mode": "personal" }),
        AppMode::Remote { url } => serde_json::json!({ "mode": "remote", "url": url }),
        AppMode::FirstLaunch => return,
    };

    let _ = std::fs::write(&path, serde_json::to_string_pretty(&json).unwrap());
}

/// Reset mode — writes a "choose" sentinel so next launch shows the selector,
/// even if config.toml exists from a previous personal mode install.
pub fn reset_mode() {
    let path = mode_file();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, r#"{"mode":"choose"}"#);
}

/// Inline HTML for the mode selector page.
///
/// This is served by a tiny local HTTP server on a random port,
/// then the Tauri webview navigates to it. When the user picks a mode,
/// JavaScript POSTs to `/select` with the choice.
pub fn selector_html(port: u16) -> String {
    format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Kyomi — Get Started</title>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@400;500;600;700&display=swap" rel="stylesheet">
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{
    font-family: 'Inter', -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
    background: #1e1e1e; color: #f1f5f9;
    display: flex; align-items: center; justify-content: center;
    min-height: 100vh; padding: 2rem;
  }}
  .container {{ max-width: 480px; width: 100%; text-align: center; }}
  h1 {{ font-size: 1.75rem; font-weight: 700; margin-bottom: 0.25rem; color: #f1f5f9; }}
  .subtitle {{ color: #94a3b8; margin-bottom: 2rem; font-size: 0.9rem; }}
  .cards {{ display: flex; flex-direction: column; gap: 0.75rem; }}
  .card {{
    background: #262626; border: 1px solid #383838; border-radius: 10px;
    padding: 1.25rem 1.5rem; text-align: left; cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }}
  .card:hover {{ border-color: #d97706; background: #2a2a2a; }}
  .card h2 {{ font-size: 1rem; font-weight: 600; margin-bottom: 0.25rem; color: #f1f5f9; }}
  .card p {{ color: #94a3b8; font-size: 0.8125rem; line-height: 1.5; }}
  .url-form {{
    margin-top: 1.25rem; display: none; text-align: left;
  }}
  .url-form.show {{ display: block; }}
  .url-form label {{ display: block; font-size: 0.8125rem; font-weight: 500; margin-bottom: 0.5rem; color: #f1f5f9; }}
  .url-form input {{
    width: 100%; padding: 0.5rem 0.75rem; font-size: 0.8125rem;
    font-family: inherit;
    background: #1e1e1e; border: 1px solid #383838; border-radius: 6px;
    color: #f1f5f9; outline: none;
  }}
  .url-form input:focus {{ border-color: #d97706; }}
  .url-form .error {{ color: #ef4444; font-size: 0.75rem; margin-top: 0.5rem; display: none; }}
  .url-form button {{
    margin-top: 0.75rem; width: 100%; padding: 0.5rem;
    background: #d97706; color: #ffffff; border: none; border-radius: 6px;
    font-family: inherit;
    font-size: 0.8125rem; font-weight: 600; cursor: pointer;
    transition: background 0.15s;
  }}
  .url-form button:hover {{ background: #b45309; }}
  .url-form button:disabled {{ opacity: 0.5; cursor: not-allowed; }}
  .hint {{ color: #64748b; font-size: 0.6875rem; margin-top: 2rem; }}
  .hint code {{ background: #262626; padding: 2px 6px; border-radius: 4px; font-size: 0.6875rem; }}
</style>
</head>
<body>
<div class="container">
  <h1>Kyomi</h1>
  <p class="subtitle">The Data Intelligence Platform</p>
  <div class="cards">
    <div class="card" onclick="selectPersonal()">
      <h2>Run Locally</h2>
      <p>Run everything locally on this machine. Connect your database and use AI tools via MCP.</p>
    </div>
    <div class="card" onclick="showServerForm()">
      <h2>Connect to Server</h2>
      <p>Connect to Kyomi Cloud or your team's self-hosted server.</p>
    </div>
  </div>
  <div class="url-form" id="serverForm">
    <label for="url">Server URL</label>
    <input type="url" id="url" value="https://app.kyomi.ai" placeholder="https://app.kyomi.ai" />
    <div class="error" id="error"></div>
    <button onclick="selectRemote()" id="connectBtn">Connect</button>
  </div>
  <p class="hint">To switch modes later: <code>kyomi-desktop --switch-mode</code></p>
</div>
<script>
  function selectPersonal() {{
    fetch('http://localhost:{port}/select', {{
      method: 'POST',
      headers: {{ 'Content-Type': 'application/json' }},
      body: JSON.stringify({{ mode: 'personal' }})
    }});
  }}
  function showServerForm() {{
    document.getElementById('serverForm').classList.add('show');
    document.getElementById('url').focus();
  }}
  async function selectRemote() {{
    const url = document.getElementById('url').value.trim().replace(/\/+$/, '');
    const btn = document.getElementById('connectBtn');
    const err = document.getElementById('error');
    if (!url) return;
    btn.disabled = true; btn.textContent = 'Connecting...';
    err.style.display = 'none';
    try {{
      const resp = await fetch(url + '/api/health', {{ mode: 'no-cors' }}).catch(() => null);
      // no-cors won't give us status, so just try to connect
      await fetch('http://localhost:{port}/select', {{
        method: 'POST',
        headers: {{ 'Content-Type': 'application/json' }},
        body: JSON.stringify({{ mode: 'remote', url: url }})
      }});
    }} catch (e) {{
      err.textContent = 'Could not reach server. Check the URL and try again.';
      err.style.display = 'block';
      btn.disabled = false; btn.textContent = 'Connect';
    }}
  }}
  document.getElementById('url').addEventListener('keydown', function(e) {{
    if (e.key === 'Enter') selectRemote();
  }});
</script>
</body>
</html>"#, port = port)
}
