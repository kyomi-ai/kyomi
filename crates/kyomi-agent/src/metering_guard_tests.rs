// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-974 guard: every LLM call is metered through the one path
//! (`MeteredClient` + `UsageLedger::bill`). These fail when a call site gets
//! around it.

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
        "crates/kyomi-agent/src/provider.rs",
        "crates/kyomi-agent/src/anthropic.rs",
        "crates/kyomi-agent/src/openai.rs",
        "crates/kyomi-agent/src/gemini.rs",
        "crates/kyomi-agent/src/anthropic_live_tests.rs",
        "crates/kyomi-agent/src/external_tool_policy.rs",
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

/// Every function in `execution.rs` that obtains a metered provider must bill it:
/// a chat run, a title, a dashboard summary and its tagging pass.
#[test]
fn every_function_that_creates_a_provider_bills_it() {
    let src = include_str!("execution.rs");
    let production = src.split("#[cfg(test)]").next().unwrap();
    let mut checked = 0;
    for chunk in production.split("\nasync fn ").chain(production.split("\npub async fn ")) {
        if chunk.contains("create_provider_from_workspace(") && !chunk.starts_with("effective_provider_kind") {
            checked += 1;
            assert!(
                chunk.contains(".bill(") || chunk.contains("bill_background_call("),
                "a function creates a provider but never bills it:\n{}",
                chunk.lines().next().unwrap_or_default()
            );
        }
    }
    assert!(checked >= 3, "expected the chat, title and summary functions, found {checked}");
}

/// The unmetered factory must not be public API.
#[test]
fn the_unmetered_provider_factory_is_crate_private() {
    let lib = include_str!("lib.rs");
    assert!(!lib.contains("create_provider,") && !lib.contains(" create_provider}"), "lib.rs re-exports create_provider");
    let provider = include_str!("provider.rs");
    assert!(provider.contains("pub(crate) fn create_provider("));
}
