//! `save_action`'s admin vs non-admin branches (MAJOR 2, KYO-184).

use super::{extract_between, SRC};

// ── MAJOR 2: non-admin save branch ──────────────────────────────────

/// The non-admin edit branch of `save_action` must call
/// `save_datasource_credentials` and must NEVER call
/// `update_datasource_settings` — the latter is workspace-admin-gated
/// server-side, so a non-admin's edits would error there. Prior to
/// KYO-184 this branch didn't exist at all: `save_action` always called
/// `update_datasource_settings` first, which meant credential-only saves
/// were completely broken for non-admins.
#[test]
fn non_admin_save_branch_calls_credentials_not_settings() {
    let branch = extract_between(
        SRC,
        "// Non-admin edit — `update_datasource_settings` is",
        "\n            }\n        }\n    });",
    );
    assert!(
        branch.contains("save_datasource_credentials("),
        "non-admin save branch must call save_datasource_credentials"
    );
    assert!(
        !branch.contains("update_datasource_settings("),
        "non-admin save branch must NOT call the admin-gated update_datasource_settings"
    );
}

/// MINOR 3: opening the edit modal and clicking "Save Credentials"
/// without typing anything must not insert an empty credential row —
/// the non-admin branch must guard the call the same way the create-mode
/// branch guards its own credentials save with `has_creds`.
#[test]
fn non_admin_save_branch_skips_empty_credentials() {
    let branch = extract_between(
        SRC,
        "// Non-admin edit — `update_datasource_settings` is",
        "\n            }\n        }\n    });",
    );
    assert!(
        branch.contains("has_creds") && branch.contains("if has_creds {"),
        "non-admin save branch must guard save_datasource_credentials on non-empty creds"
    );
}

/// Review finding on PR #232: the admin edit branch discarded the
/// `save_datasource_credentials` result with `let _ = ...`, so a
/// credential-save failure after a successful `update_datasource_settings`
/// call was invisible — the modal closed as if everything had saved. The
/// fix must not swallow the error (`let _ =`) and must not propagate it
/// with a blanket `?` either (settings genuinely did persist, so the
/// whole save must not be reported as failed) — it must surface the
/// failure via `toast_error` while still returning `Ok(r)`.
#[test]
fn admin_save_branch_does_not_discard_credential_save_result() {
    let branch = extract_between(
        SRC,
        "Some(id) if is_admin_val => {",
        "\n                Some(id) => {",
    );
    assert!(
        !branch.contains("let _ = save_datasource_credentials"),
        "admin save branch must not silently discard the credential save result"
    );
    assert!(
        branch.contains("let Err(e) = save_datasource_credentials(id, creds).await"),
        "admin save branch must inspect the credential save result"
    );
    assert!(
        branch.contains("toast_error("),
        "admin save branch must surface a credential-save failure via toast \
         (error_msg would be invisible — on_saved closes the modal on Ok)"
    );
    assert!(
        !branch.contains("save_datasource_credentials(id, creds).await?"),
        "admin save branch must not propagate the credential error with a blanket `?` — \
         settings genuinely persisted, so the whole save must not be reported as failed"
    );
    assert!(
        branch.contains("Ok(r)"),
        "admin save branch must still return Ok(r) on a partial (settings-only) success"
    );
}
