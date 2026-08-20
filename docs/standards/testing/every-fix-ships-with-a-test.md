# Every bug fix / behavior change ships with a test asserting the exact changed behavior

A fix is not done until a test locks in the precise behavior the ticket changed — not "related functionality," the exact assertion. If the target module has no `#[cfg(test)] mod tests`, add one; these are usually one-liners.

**Rule:** Before calling a fix complete, ask "what one assertion, if it regressed, would silently reintroduce this bug?" and write that test. A `Display`-format fix gets `assert_eq!(Error::X(msg).to_string(), msg)`; a security/validation fix gets one test per rejection path (wrong user, expired, wrong state); a query-identity fix asserts the row count / archived count directly.

Flagged repeatedly in review: KYO-145 (missing `Display` prefix-free assertion blocked sign-off until added), KYO-143 (new status-mapping branch shipped untested), KYO-140 (exemplary — the archival fix shipped with a test running the real query template against an in-memory pool asserting `tables_archived == 0`).
