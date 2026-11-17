// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-969: the server binary refuses to start on a web search/fetch backend that
//! cannot work with the configured LLM provider, before it touches the database.
//! (`validate_startup`'s rules are unit-tested in `kyomi-agent`; this proves the
//! binary actually calls it.)

use std::process::Command;

fn kyomi(envs: &[(&str, &str)]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_kyomi"));
    command
        .env_clear()
        .env("DATABASE_URL", "postgres://unused:unused@127.0.0.1:1/unused")
        .env("ENCRYPTION_KEY", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        .env("JWT_SECRET_KEY", "test-jwt-secret-not-real-0123456789abcdef")
        .env("RUST_LOG", "off")
        .envs(envs.iter().copied());
    command.output().expect("run the kyomi binary")
}

#[test]
fn an_anthropic_search_backend_with_another_llm_provider_stops_startup() {
    let output = kyomi(&[
        ("LLM_PROVIDER", "openai"),
        ("LLM_API_KEY", "test-key-not-real"),
        ("WEB_SEARCH_PROVIDER", "anthropic"),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.contains("FATAL: invalid web search/fetch configuration"), "stderr: {stderr}");
    assert!(stderr.contains("anthropic"), "the message names the backend: {stderr}");
}
