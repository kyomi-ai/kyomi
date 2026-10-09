// SPDX-License-Identifier: AGPL-3.0-or-later

//! `resolve_process_instance` (KYO-493) — the pure decision
//! `apps/server/src/main.rs` calls exactly once at startup to build
//! `AppState::process_instance`. Tested as a function of explicit
//! `hostname`/`port` parameters rather than the real environment — see
//! `docs/standards/testing/a-tests-verdict-must-not-depend-on-the-ambient-environment.md`.

use super::super::{KyomiMode, resolve_process_instance};
use crate::Config;

fn personal_config() -> Config {
    Config { mode: KyomiMode::Personal, ..Config::test_config() }
}

fn server_config() -> Config {
    Config { mode: KyomiMode::Saas, ..Config::test_config() }
}

#[test]
fn personal_mode_returns_the_fixed_desktop_literal() {
    assert_eq!(resolve_process_instance(&personal_config(), None, 3000), "desktop");
}

#[test]
fn personal_mode_ignores_a_present_hostname_and_port() {
    // is_personal() is a structural, compile-time-knowable deployment fact —
    // a HOSTNAME happening to be set (e.g. inherited from a dev shell) must
    // not change the answer for a single-process desktop deployment, and
    // there is no meaningful "port" to disambiguate a process that can
    // never share a machine with another instance of itself.
    assert_eq!(
        resolve_process_instance(&personal_config(), Some("some-laptop"), 3000),
        "desktop"
    );
}

#[test]
fn server_mode_combines_hostname_and_port() {
    assert_eq!(
        resolve_process_instance(&server_config(), Some("kyomi-api-7f8b9-x2k4p"), 3000),
        "kyomi-api-7f8b9-x2k4p:3000"
    );
}

#[test]
fn server_mode_same_hostname_different_ports_are_distinct_identities() {
    // The exact scenario this format exists for: dev.kyomi.ai and a
    // worktree verifier server on the same machine (HOSTNAME=nuc), on
    // different ports, sharing the same Postgres — each must own only its
    // own in_progress rows, not each other's.
    let a = resolve_process_instance(&server_config(), Some("nuc"), 3000);
    let b = resolve_process_instance(&server_config(), Some("nuc"), 3100);
    assert_ne!(a, b);
    assert_eq!(a, "nuc:3000");
    assert_eq!(b, "nuc:3100");
}

#[test]
#[should_panic(expected = "HOSTNAME environment variable is required")]
fn server_mode_without_a_hostname_panics_instead_of_inventing_one() {
    resolve_process_instance(&server_config(), None, 3000);
}
