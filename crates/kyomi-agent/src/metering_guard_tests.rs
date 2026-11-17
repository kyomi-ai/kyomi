// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-974 guard: every LLM call is metered through the one path
//! (`MeteredClient` + `UsageLedger::bill`). The compiler already guarantees it from
//! outside this crate: `create_provider_from_workspace` returns `MeteredClient`, and the
//! raw factory and every client constructor are `pub(crate)`. Inside this crate it does
//! not, so this scan catches a new module constructing a provider client directly. That each call site bills is
//! covered by real-call-site tests in `anthropic_billing_tests.rs`.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("workspace root")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if !matches!(path.file_name().and_then(|n| n.to_str()), Some("target" | "node_modules" | ".git")) {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Provider clients can only be constructed inside the provider layer, which
/// hands them out wrapped in `MeteredClient` (`create_provider_from_workspace`).
/// Constructing one anywhere else would give an unmetered call path.
#[test]
fn llm_clients_are_only_constructed_in_the_provider_layer() {
    let root = workspace_root();
    let allowed = [
        // The provider layer: the factory wraps what it builds in `MeteredClient`.
        "crates/kyomi-agent/src/provider.rs",
        // Each client's own file (constructors and their unit tests).
        "crates/kyomi-agent/src/anthropic.rs",
        "crates/kyomi-agent/src/openai.rs",
        "crates/kyomi-agent/src/gemini.rs",
        // `#[ignore]`d harness that calls the real API directly on purpose (never runs in CI).
        "crates/kyomi-agent/src/anthropic_live_tests.rs",
        // Test-only: builds clients to check external-tool policy, never calls them.
        "crates/kyomi-agent/src/external_tool_policy.rs",
        // This file (the patterns appear in the scan itself).
        "crates/kyomi-agent/src/metering_guard_tests.rs",
    ];
    let patterns = [
        "AnthropicClient::new(",
        "AnthropicClient::with_base_url(",
        "OpenAIProvider::new(",
        "GeminiProvider::new(",
        "GeminiProvider::with_base_url(",
    ];
    let mut files = Vec::new();
    for dir in ["crates", "apps", "enterprise"] {
        rust_files(&root.join(dir), &mut files);
    }
    assert!(files.len() > 100, "the scan must actually see the workspace, saw {}", files.len());
    let mut offenders = Vec::new();
    for file in files {
        let rel = file.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
        if allowed.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for p in patterns {
            if text.contains(p) {
                offenders.push(format!("{rel}: {p}"));
            }
        }
    }
    assert!(offenders.is_empty(), "unmetered LLM client construction: {offenders:?}");
}
