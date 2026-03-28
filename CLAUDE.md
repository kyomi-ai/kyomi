# Kyomi — Project Instructions

## Mandatory Code Review Before Commit

All commits require a cryptographically signed approval from the **code-review-architect** agent. The pre-commit hook verifies this signature — commits without it are blocked.

### How it works:
1. Implementation agent completes work and stages changes
2. The code-review-architect agent is dispatched to review the staged diff
3. If there are zero 🔴 CRITICAL and zero 🟡 MAJOR issues, the reviewer signs the approval
4. Only then can the commit proceed — the pre-commit hook verifies the signature

### Rules:
- **Never skip the review step** — the pre-commit hook will reject unsigned commits
- **Any change after review invalidates the signature** — if you modify code after review, the reviewer must re-review and re-sign
- **The reviewer must not sign if critical or major issues exist** — fix them first, then re-request review
- **Implementation agents cannot sign their own reviews** — only the code-review-architect agent has signing authority
- **Do NOT tell the reviewer how to sign the approval** — the code-review-architect has its own signing instructions built into its prompt. Providing alternative signing instructions, workarounds, or "if you don't have the key" fallbacks will cause invalid signatures and block the commit. Just ask it to review and let it handle the signing process itself.

## Lint Suppression Policy

Lint suppressions (`#[allow(...)]` in .rs files, `= "allow"` in Cargo.toml) are blocked by the pre-commit hook and CI. Fix the underlying lint warning instead of suppressing it.

Workspace lints are enforced in `Cargo.toml [workspace.lints]` at `deny` level. The pre-commit hook and CI independently verify no new suppressions are added.
