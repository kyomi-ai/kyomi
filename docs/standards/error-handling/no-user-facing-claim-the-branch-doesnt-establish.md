# Never show the user a claim the branch does not establish

A status page, a toast, or an error string is read as fact, and unlike a comment the reader has no source to check it against. The recurring defect is text that hardcodes one outcome, one cause, or one location into a branch that covers several: a fallback rendered on both the success and error paths that says "Your account is linked"; an error message that names an IAM permission when the same `Err` also carries network failures, expired tokens and 5xx; a docs sentence that sends a blocked user to a surface that exists under no such name. Each one lands exactly where honesty matters most — the user is already stuck, and the text they are handed is confidently wrong about why. This is the user-facing sibling of [never claim a guarantee stronger than the code enforces](../comments-documentation/no-guarantee-stronger-than-code-enforces.md), and it is distinct from the `Error::user_message()` tag-leak family (KYO-350/380/389/397), where the text is right and only an internal variant prefix leaks.

**Rule:** Derive user-visible text from the state the branch actually established. If a piece of UI is reachable from more than one outcome, drive its title, icon and copy off the outcome value, and let any extra condition contribute only wording that is true on *every* path it can appear on. If an `Err` arm covers several causes, distinguish on the variant or status code before naming a cause — otherwise describe the failure without asserting one. If the text names a product surface, verify the literal label in the source rather than describing where you remember it living. "Same pattern as <other page>" is not a justification unless that page can represent the same set of outcomes.

```rust
// WRONG — the close-refused fallback renders unconditionally, so a user whose
// OAuth attempt actually failed is told, with a green check, that it worked
let title = Signal::derive(move || {
    if popup_close_blocked.get() {
        "You're All Set".to_string() // true only on the success path
    } else {
        match status.get() { /* ... */ }
    }
});

// RIGHT — title and icon stay outcome-driven; the extra condition only appends
// a sentence that is true whether the link succeeded or failed
let title = Signal::derive(move || match status.get() {
    LinkStatus::Processing => "Linking Google Account".to_string(),
    LinkStatus::Success => "Google Account Linked".to_string(),
    LinkStatus::Error => "Link Failed".to_string(),
});
let subtitle = Signal::derive(move || {
    let base = match status.get() { /* ... outcome-driven ... */ };
    if popup_close_blocked.get() {
        format!("{base} You can close this window.")
    } else {
        base
    }
});
```

Mined from the `2026-08-21` and `2026-08-22` review logs — each phrase quoted below was located mechanically and appears in that day's log and in no other. Cited by log entry rather than file:line because two of the three were logged against in-flight branch state whose line numbers do not resolve against `main`.

- **KYO-436, cycle 2 (`15:05` entry, 2026-08-22)** — 🟡, blocked signing. The new `popup_close_blocked` fallback rendered "You're All Set" / "Your account is linked" with a green check regardless of `LinkStatus`, on a page that can represent either outcome; the close-popup block runs identically after `send_oauth_error_to_opener`. The implementer's "same pattern as `OAuthCompletePage`" defence did not transfer — that page is success-only with no error variant. Cycle 3 fixed it by making title and icon purely status-driven and appending only the outcome-neutral "You can close this window."
- **KYO-444 (`08:10` entry, 2026-08-22)** — 🟢. `record_failure` tells the user their IAM role lacks `resourcemanager.projects.list`, but the same `Err` also covers network failures, malformed JSON, 401 on an expired token, and generic 5xx — all of which surface "reconfigure your IAM role" in the Catalog tab, to a customer already burned by opaque failures.
- **KYO-416 (`16:31` entry, 2026-08-21)** — 🟡, the docs-side echo. New prerequisite text told a blocked user the feedback form is "reached from the datasource connection page"; no surface exists under that name. Fixed by naming the **Send Feedback** form itself, verified against the literal label in `crates/kyomi-ui/src/components/layout.rs` — which stays true across all three places that entry point has lived.
