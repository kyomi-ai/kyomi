// SPDX-License-Identifier: AGPL-3.0-or-later

//! `SmtpSettings` — the one definition of "can this deployment send mail?"
//! (KYO-685), and the guard that `Config::from_env` routes through it.

use super::super::{SMTP_ENV_VARS, SmtpSettings};
use super::SRC;

const HOST: &str = "smtp.example.com";
const USER: &str = "mailer@example.com";
const PASSWORD: &str = "s3cret";

/// Classify a presence triple `[host, user, password]`.
fn classify(present: [bool; 3]) -> SmtpSettings {
    SmtpSettings::classify(
        present[0].then_some(HOST),
        present[1].then_some(USER),
        present[2].then_some(PASSWORD),
    )
}

/// Every presence combination of the three parts, most-significant bit first.
fn all_combinations() -> impl Iterator<Item = [bool; 3]> {
    (0..8u8).map(|bits| [bits & 0b100 != 0, bits & 0b010 != 0, bits & 0b001 != 0])
}

/// The full truth table, spelled out rather than recomputed from the
/// production expression — an expectation derived the same way as the code
/// under test would agree with any bug it contains.
#[test]
fn classify_covers_all_eight_presence_combinations() {
    let cases: [([bool; 3], SmtpSettings); 8] = [
        ([false, false, false], SmtpSettings::Absent),
        (
            [true, false, false],
            SmtpSettings::Incomplete { missing: vec!["SMTP_USER", "SMTP_PASSWORD"] },
        ),
        (
            [false, true, false],
            SmtpSettings::Incomplete { missing: vec!["SMTP_HOST", "SMTP_PASSWORD"] },
        ),
        (
            [false, false, true],
            SmtpSettings::Incomplete { missing: vec!["SMTP_HOST", "SMTP_USER"] },
        ),
        (
            [true, true, false],
            SmtpSettings::Incomplete { missing: vec!["SMTP_PASSWORD"] },
        ),
        (
            [true, false, true],
            SmtpSettings::Incomplete { missing: vec!["SMTP_USER"] },
        ),
        (
            [false, true, true],
            SmtpSettings::Incomplete { missing: vec!["SMTP_HOST"] },
        ),
        ([true, true, true], SmtpSettings::Complete),
    ];

    let covered: Vec<[bool; 3]> = cases.iter().map(|(present, _)| *present).collect();
    for present in all_combinations() {
        assert!(
            covered.contains(&present),
            "presence combination {present:?} is missing from the truth table — \
             all eight must be pinned (KYO-685)"
        );
    }

    for (present, expected) in cases {
        let settings = classify(present);
        assert_eq!(
            settings, expected,
            "classify disagrees with the truth table for [host, user, password] = {present:?}"
        );
        assert_eq!(
            settings.can_send(),
            present == [true, true, true],
            "mail can be sent only when host, user AND password are all present — \
             the mailer authenticates before it can submit a message (KYO-685); \
             [host, user, password] = {present:?}"
        );
        assert_eq!(
            settings.missing_vars(),
            expected.missing_vars(),
            "missing_vars must name the absent variables for {present:?}"
        );
    }
}

/// The misconfiguration KYO-685 was filed for: the operator set two of the
/// three variables, so the app believed it could send mail and the mailer knew
/// it could not. The boot diagnostic has to name the variable — "SMTP not
/// configured" is what let this survive.
#[test]
fn host_and_user_without_password_cannot_send_and_warns_by_name() {
    let settings = SmtpSettings::classify(Some(HOST), Some(USER), None);

    assert!(
        !settings.can_send(),
        "SMTP_HOST + SMTP_USER with no SMTP_PASSWORD cannot send mail (KYO-685)"
    );

    let warning = settings
        .startup_warning()
        .expect("a partial SMTP configuration must warn at boot (KYO-685)");
    assert!(
        warning.contains("SMTP_PASSWORD"),
        "the boot warning must name the missing variable, not just report that SMTP \
         is off — got: {warning}"
    );
}

#[test]
fn startup_warning_fires_only_for_a_partial_configuration() {
    for present in all_combinations() {
        let settings = classify(present);
        let some_set = present.iter().any(|p| *p);
        let all_set = present.iter().all(|p| *p);
        let partial = some_set && !all_set;

        let warning = settings.startup_warning();
        assert_eq!(
            warning.is_some(),
            partial,
            "only a partial SMTP configuration is unambiguously operator error: all three \
             set works and none set is a supported SMTP-less deployment; \
             [host, user, password] = {present:?}"
        );

        if let Some(message) = warning {
            for (name, is_present) in SMTP_ENV_VARS.iter().zip(present) {
                assert_eq!(
                    message.contains(name),
                    !is_present,
                    "the boot warning must name exactly the absent variables for \
                     {present:?} — got: {message}"
                );
            }
        }
    }
}

/// KYO-423: a predicate with two copies drifts. `Config::from_env` must read
/// the flag off `SmtpSettings`, not re-spell the conjunction — that is the
/// exact shape of the KYO-685 bug (`SMTP_HOST` and `SMTP_USER` only).
#[test]
fn config_from_env_routes_the_flag_through_the_shared_predicate() {
    assert_eq!(
        SRC.matches("SmtpSettings::classify(").count(),
        1,
        "Config::from_env must classify the SMTP settings exactly once and derive the \
         flag from it; a second call site means a second rule that can drift (KYO-423)"
    );
    assert!(
        SRC.contains("smtp_configured: smtp.can_send(),"),
        "the smtp_configured field must be assigned from SmtpSettings::can_send(), so it \
         cannot disagree with EmailService::is_configured() (KYO-685)"
    );
    assert!(
        !SRC.contains("env::var(\"SMTP_HOST\").is_ok()"),
        "the two-variable conjunction is the KYO-685 bug: it reported smtp_configured = \
         true with SMTP_PASSWORD unset, so self-hosted signup took the SaaS email branch \
         and no account could be created"
    );
}
