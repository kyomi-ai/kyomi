//! BigQuery kyomi_oauth access gating: the KYO-408/KYO-499 "beta access"
//! notice and its Save/Create attestation gate
//! (`bq_kyomi_oauth_access_gate_satisfied`), and the KYO-427/KYO-477
//! Connect/Reconnect-only gate (`bq_kyomi_oauth_connect_allowed`) that
//! deliberately does NOT accept an already-connected account the way the
//! Save/Create gate does — an account-level `oauth_connected` must never
//! let a returning user skip confirming access for a *new* datasource.
//! Covers both predicates directly and their wiring into the actual
//! Connect/Reconnect buttons.
//!
//! See
//! `docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md`.

use super::{appears_shortly_before, extract_between, SRC};

// ── KYO-408: BigQuery kyomi_oauth access-request notice + gate ─────────
//
// Kyomi's shared Google OAuth app only accepts Google accounts a Kyomi
// admin has manually added as testers in the Cloud Console — Kyomi has
// no programmatic access to that list, so Google is the only
// enforcement layer. The notice/checkbox added here are NOT a security
// control; they exist purely to make the user request access before
// burning a doomed OAuth round-trip. Three things are covered below:
// the notice renders only for kyomi_oauth, the gate itself (a pure
// predicate, `bq_kyomi_oauth_access_gate_satisfied`) is exercised
// directly, and its wiring into the Save/Create buttons (but
// deliberately NOT the Next button) is pinned by source-text checks.
//
// Copy and link target were rewritten in KYO-499 to restore parity with
// the React original (`AuthModeSelector.jsx` at `ee16f48a^`) — the
// version that shipped in KYO-408/435/477 diverged into a heading + two
// explanatory paragraphs + a standalone <Button> opening the in-app
// FeedbackModal, none of which React did. See `utils::beta_access` for
// the shared copy/persistence/link-target module both this notice and
// the login page's equivalent (`pages/auth/login.rs`) now read through.

use super::super::bq_kyomi_oauth_access_gate_satisfied;

/// The KYO-408/KYO-499 notice (Alert + inline "Request beta access" link +
/// confirmation checkbox) must render inside the kyomi_oauth `<Show>` block
/// and must not leak into enterprise_oauth or service_account — neither of
/// those modes has a Kyomi-side Google-account allowlist to request
/// access to (enterprise_oauth uses the customer's own OAuth app;
/// service_account has no OAuth at all).
#[test]
fn kyomi_oauth_notice_renders_only_for_kyomi_oauth_mode() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        kyomi_block.contains("This authentication method requires beta access."),
        "the kyomi_oauth block must render the KYO-499 access notice sentence"
    );
    assert!(
        kyomi_block.contains("\"Request beta access\""),
        "the kyomi_oauth notice must include a \"Request beta access\" link (KYO-499 \
         copy, restoring the React original)"
    );
    assert!(
        kyomi_block.contains("beta_access::BETA_ACCESS_REQUEST_HREF"),
        "the \"Request beta access\" link must point at the shared \
         utils::beta_access::BETA_ACCESS_REQUEST_HREF target (KYO-499) — not a \
         hardcoded href or the removed FeedbackAccessRequestHandle context, which \
         login.rs can never reach pre-auth"
    );
    assert!(
        kyomi_block.contains("\"I have beta access\""),
        "the kyomi_oauth block must render the KYO-499 confirmation checkbox with the \
         exact copy \"I have beta access\", restoring the React original's wording \
         (KYO-408's \"I have requested access and had it confirmed\" was rejected as a \
         divergence from React — see KYO-499)"
    );

    let enterprise_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"service_account\">",
    );
    assert!(
        !enterprise_block.contains("requires beta access"),
        "the KYO-408/499 notice must not leak into enterprise_oauth — that mode uses \
         the customer's own Google Cloud OAuth app, which has no Kyomi allowlist to \
         request access to"
    );
    assert!(
        !enterprise_block.contains("\"I have beta access\""),
        "the KYO-499 confirmation checkbox must not leak into enterprise_oauth"
    );

    let service_account_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"service_account\">",
        "fn SnowflakeAuthModeSection(",
    );
    assert!(
        !service_account_block.contains("requires beta access"),
        "the KYO-408/499 notice must not leak into service_account — that mode has no \
         OAuth flow at all, so an OAuth-allowlist notice would be nonsensical there"
    );
    assert!(
        !service_account_block.contains("\"I have beta access\""),
        "the KYO-499 confirmation checkbox must not leak into service_account"
    );
}

// ── KYO-499: "Request beta access" is an inline link, not a <Button> ───
//
// KYO-435 had promoted this control from an inline text link to a
// standalone <Button> on its own line, reasoning the inline link was
// "effectively invisible" against the amber Warning background. KYO-499
// reverts that: React's original rendered it as a plain inline link
// spliced mid-sentence (`text-primary hover:underline font-medium`), and
// restoring exact parity with React was this ticket's explicit goal —
// see the KYO-499 ticket for the full "how we diverged" writeup. This
// test pins the reverted shape so a future edit can't silently
// reintroduce the standalone-<Button> layout without a deliberate
// decision to do so.

/// Scoped to the same kyomi_oauth `<Show>` block as
/// `kyomi_oauth_notice_renders_only_for_kyomi_oauth_mode` above — bounded
/// by the next mode's `<Show>` opening tag — so a match here can only be
/// the production markup for this one panel, not a sibling panel's
/// textually similar markup.
#[test]
fn request_beta_access_control_is_an_inline_link_not_a_button() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        kyomi_block.contains("<a\n") || kyomi_block.contains("<a "),
        "KYO-499: \"Request beta access\" must render as a plain inline <a> link \
         (React's own shape — text-primary hover:underline font-medium, spliced \
         mid-sentence), not a <Button>/<ButtonLink> — found no <a> tag in the \
         kyomi_oauth block"
    );
    assert!(
        !kyomi_block.contains("<Button") && !kyomi_block.contains("<ButtonLink"),
        "KYO-499: the standalone <Button>/<ButtonLink> control KYO-435 introduced must \
         be gone — the request-access control reverted to an inline link matching React"
    );
    assert!(
        kyomi_block.contains("beta_access::write_beta_access(v)"),
        "KYO-499: ticking the checkbox must persist to localStorage via \
         beta_access::write_beta_access, not merely update the in-memory signal — found \
         no call in the kyomi_oauth block"
    );
}

/// `bq_kyomi_oauth_access_gate_satisfied` — the pure predicate behind
/// the KYO-408 Save/Create gate — exercised directly rather than via
/// the view tree. Covers: blocked when unchecked and unconnected;
/// released by ticking the checkbox; released by an already-successful
/// OAuth connection even with the checkbox unchecked (so a returning,
/// already-authorized user is never nagged); and a no-op (always
/// satisfied) for every other datasource type / BigQuery auth mode.
#[test]
fn bq_kyomi_oauth_access_gate_blocks_unchecked_and_releases_when_checked() {
    // Blocked: BigQuery kyomi_oauth, not connected, checkbox unchecked.
    assert!(
        !bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", false, false),
        "the gate must block Save/Create for BigQuery kyomi_oauth when neither \
         connected nor confirmed"
    );

    // Released by the checkbox alone.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", false, true),
        "ticking the confirmation checkbox must release the gate even without a live \
         OAuth connection"
    );

    // Released by a real OAuth connection alone — the whole point of
    // not nagging a user who already has proven access.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", true, false),
        "a successful OAuth connection must release the gate on its own — it is itself \
         proof the account was already allowlisted, so an unchecked checkbox must not \
         re-block Save/Create"
    );

    // Unaffected: BigQuery service_account and enterprise_oauth.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "service_account", false, false),
        "service_account has no Kyomi Google-account allowlist — the gate must never \
         block it, regardless of connected/confirmed"
    );
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "enterprise_oauth", false, false),
        "enterprise_oauth uses the customer's own OAuth app, not Kyomi's — the gate must \
         never block it"
    );

    // Unaffected: a non-BigQuery provider, even if (hypothetically) its
    // auth mode string were literally "kyomi_oauth".
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("snowflake", "kyomi_oauth", false, false),
        "the gate is BigQuery-specific — ds_type must be checked, not auth_mode alone"
    );
}

// ── KYO-477: Connect/Reconnect gets its OWN predicate ───────────────────
//
// KYO-427 pointed the "Connect BigQuery" button at `bq_kyomi_oauth_access_ok`
// — the Save/Create gate above, which OR's in `oauth_connected`. That
// shipped in v2.6.5 with a green review and a mutation-proved test
// (`bigquery_kyomi_oauth_connect_button_reads_the_shared_access_ok_signal`,
// below) — and did not fix the reported behavior, because `oauth_connected`
// for kyomi_oauth is account-level: true forever, for every datasource,
// once a user has linked Google to Kyomi even once. The prior test proved
// the button *read a signal*; it never proved the signal computed the
// right answer for a returning, previously-linked user trying a *new*
// datasource without having confirmed access for it. That is the gap this
// section closes.

use super::super::bq_kyomi_oauth_connect_allowed;

/// `bq_kyomi_oauth_connect_allowed` — the pure predicate behind the
/// KYO-477 Connect/Reconnect gate — exercised directly. Unlike
/// `bq_kyomi_oauth_access_gate_satisfied` above, this predicate does not
/// even accept an `oauth_connected` argument: Connect must always require
/// its own explicit, in-the-moment confirmation.
#[test]
fn bq_kyomi_oauth_connect_allowed_blocks_unchecked_and_releases_when_checked() {
    // Blocked: BigQuery kyomi_oauth, checkbox unchecked.
    assert!(
        !bq_kyomi_oauth_connect_allowed("bigquery", "kyomi_oauth", false),
        "the Connect gate must block BigQuery kyomi_oauth when access_confirmed is false"
    );

    // Released by the checkbox alone.
    assert!(
        bq_kyomi_oauth_connect_allowed("bigquery", "kyomi_oauth", true),
        "ticking the confirmation checkbox must release the Connect gate"
    );

    // Unaffected: BigQuery service_account and enterprise_oauth.
    assert!(
        bq_kyomi_oauth_connect_allowed("bigquery", "service_account", false),
        "service_account has no Kyomi Google-account allowlist — the Connect gate must \
         never block it"
    );
    assert!(
        bq_kyomi_oauth_connect_allowed("bigquery", "enterprise_oauth", false),
        "enterprise_oauth uses the customer's own OAuth app, not Kyomi's — the Connect \
         gate must never block it"
    );

    // Unaffected: a non-BigQuery provider, even if (hypothetically) its
    // auth mode string were literally "kyomi_oauth".
    assert!(
        bq_kyomi_oauth_connect_allowed("snowflake", "kyomi_oauth", false),
        "the Connect gate is BigQuery-specific — ds_type must be checked, not auth_mode \
         alone"
    );
}

/// Wiring companion to the pure-predicate test above: the
/// `bq_kyomi_oauth_connect_ok` `Signal::derive` definition itself in
/// `DatasourceModal` (not merely its `connect_blocked` call site, covered
/// by `bigquery_kyomi_oauth_connect_button_reads_the_connect_only_signal`
/// below) must never read any spelling of `oauth_connected` — a `||
/// modal_oauth_connected.get()` folded in at the definition site would
/// reintroduce KYO-477 without changing anything the connect_blocked
/// wiring test inspects.
#[test]
fn bq_kyomi_oauth_connect_ok_definition_never_reads_oauth_connected() {
    let definition = extract_between(
        SRC,
        "let bq_kyomi_oauth_connect_ok: Signal<bool> = Signal::derive(move || {",
        "});",
    );
    assert!(
        definition.contains("bq_kyomi_oauth_connect_allowed("),
        "bq_kyomi_oauth_connect_ok must be defined by calling bq_kyomi_oauth_connect_allowed"
    );
    assert!(
        !definition.to_lowercase().contains("oauth_connected"),
        "KYO-477: the bq_kyomi_oauth_connect_ok Signal::derive body must not read \
         oauth_connected (in any spelling — modal_oauth_connected, oauth_connected, \
         etc.) — folding an account-level connected flag in here is the exact defect \
         this ticket fixes, found:\n{definition}"
    );
}

/// The exact regression KYO-477 was filed against, and the acceptance
/// criterion the ticket names explicitly: a Kyomi user who has linked
/// Google to their account before (`oauth_connected == true`,
/// account-level and permanently true from here on) must still be
/// blocked from clicking Connect on a datasource they have not
/// confirmed access for. Save/Create's own gate is contrasted directly
/// alongside it to make the point unmissable: the same
/// `oauth_connected = true, access_confirmed = false` inputs must
/// release ONE gate and hold the OTHER — that is the entire fix.
#[test]
fn connect_gate_stays_blocked_when_oauth_connected_but_not_confirmed() {
    // Save/Create: unaffected by this fix, correctly stays satisfied.
    assert!(
        bq_kyomi_oauth_access_gate_satisfied("bigquery", "kyomi_oauth", true, false),
        "Save/Create must remain satisfied once oauth_connected is true, regardless of \
         access_confirmed — this is existing, correct, unchanged Save/Create behavior"
    );

    // Connect: must NOT reuse that same permissive path. This predicate
    // structurally cannot read `oauth_connected` at all — it only takes
    // `access_confirmed` — so an account-level "linked somewhere, some
    // time" can never silently satisfy it.
    assert!(
        !bq_kyomi_oauth_connect_allowed("bigquery", "kyomi_oauth", false),
        "KYO-477: Connect must stay BLOCKED when access_confirmed is false, even though \
         oauth_connected is (permanently, account-wide) true — this is the exact defect \
         that let a previously-linked user bypass the checkbox and reach a doomed OAuth \
         round-trip on every subsequent datasource"
    );
}

/// The gate must be wired into the edit-mode Save button and the
/// create-mode Catalog-tab Create button, but deliberately NOT into
/// the create-mode Connection-tab Next button — matching the React
/// reference (`DatasourceModal.jsx`), whose `requiresBetaAccess` check
/// lived only on the Create/Save actions. By the time a create-mode
/// user can even reach the Catalog tab for BigQuery kyomi_oauth, a
/// successful OAuth connection has already set `test_result` (KYO-404)
/// — so gating Next too would be redundant, and gating Next instead of
/// Create would let the user pass the notice, uncheck the box, and
/// still complete the create.
#[test]
fn bq_kyomi_oauth_access_gate_wired_into_save_and_create_not_next() {
    // Bounded to just the `disabled=Signal::derive(...)` body — NOT the
    // explanatory comment directly above it, which also names
    // `bq_kyomi_oauth_access_ok` and would otherwise make this
    // assertion pass even if the real read were deleted from the
    // expression itself.
    let save_button = extract_between(
        SRC,
        "disabled=Signal::derive(move || {",
        "on:click=move |_| do_save()",
    );
    assert!(
        save_button.contains("bq_kyomi_oauth_access_ok"),
        "the edit-mode Save button's disabled expression must read \
         bq_kyomi_oauth_access_ok"
    );

    let next_button = extract_between(
        SRC,
        "if is_connection_tab {",
        "\"Next\"",
    );
    assert!(
        !next_button.contains("bq_kyomi_oauth_access_ok"),
        "the create-mode Next button must NOT read bq_kyomi_oauth_access_ok — the KYO-408 \
         gate only ever blocks Save/Create, matching the React reference and avoiding a \
         redundant second gate (BigQuery kyomi_oauth's Next is already gated by \
         connection_step_satisfied's test_result requirement, which cannot be true \
         without a successful — therefore already-allowlisted — OAuth connection)"
    );

    // Bounded to the Create button's own opening-tag attributes: starts
    // at this button's own KYO-408 comment (unique — the Save button's
    // parallel comment reads differently) and ends at the button's own
    // children text, just before `>` closes the opening tag — so a
    // later, unrelated bq_kyomi_oauth_access_ok read elsewhere can't
    // make this pass vacuously.
    let create_button = extract_between(
        SRC,
        "// KYO-408: does not gate \"Next\" above, only the",
        "\"Creating...\"",
    );
    assert!(
        create_button.contains("bq_kyomi_oauth_access_ok"),
        "the create-mode Catalog-tab Create button's disabled expression must read \
         bq_kyomi_oauth_access_ok"
    );
}

// ── KYO-427/KYO-477: gate "Connect BigQuery" on its OWN attestation gate ─

/// The KYO-408 checkbox previously gated only Save/Create, which in
/// create mode is unreachable until OAuth has already succeeded — so it
/// gated nothing where it mattered. KYO-427 requires the kyomi_oauth
/// `ModalOAuthStatusPanel`'s "Connect BigQuery" button itself to read a
/// gate driven by the same checkbox — but KYO-477 corrects *which*
/// signal that must be: `bq_kyomi_oauth_connect_ok`, NOT
/// `bq_kyomi_oauth_access_ok` (the Save/Create signal, which OR's in the
/// account-level `oauth_connected` and therefore defeats the checkbox
/// for any previously-linked user — see `bq_kyomi_oauth_connect_allowed`'s
/// doc comment). This test also asserts the negative: the connect_blocked
/// derive must NOT read `bq_kyomi_oauth_access_ok` at all, so a
/// regression back to the KYO-427 shape fails loudly here rather than
/// silently passing a "some gate exists" check.
///
/// Scoped to `kyomi_block`, the `<Show when=bq_auth_mode == "kyomi_oauth">`
/// body used by the sibling KYO-404 tests above — this is the same
/// isolation those tests rely on to keep the textually-similar
/// enterprise_oauth block (which must NOT gain this gate, see the next
/// test) from making either assertion pass vacuously.
#[test]
fn bigquery_kyomi_oauth_connect_button_reads_the_connect_only_signal() {
    let kyomi_block = extract_between(
        SRC,
        "<Show when=move || bq_auth_mode.get() == \"kyomi_oauth\">",
        "<Show when=move || bq_auth_mode.get() == \"enterprise_oauth\">",
    );
    assert!(
        kyomi_block.contains("connect_blocked=Signal::derive(move || {"),
        "the kyomi_oauth ModalOAuthStatusPanel must pass a connect_blocked prop \
         derived fresh from a signal, not a plain bool literal"
    );
    assert!(
        appears_shortly_before(
            kyomi_block,
            "!bq_kyomi_oauth_connect_ok.get()",
            "on_disconnect=on_google_disconnect",
            200,
        ),
        "the kyomi_oauth ModalOAuthStatusPanel's connect_blocked derive must read \
         bq_kyomi_oauth_connect_ok — the Connect-only gate (KYO-477) — not \
         bq_kyomi_oauth_access_ok, the Save/Create gate"
    );
    // Narrowly scoped to the connect_blocked derive itself — NOT the whole
    // kyomi_block, which legitimately still mentions bq_kyomi_oauth_access_ok
    // in an explanatory comment a few lines above (contrasting the two
    // gates). Only the derive's own body must never read it.
    let connect_blocked_derive = extract_between(
        kyomi_block,
        "connect_blocked=Signal::derive(move || {",
        "on_disconnect=on_google_disconnect",
    );
    assert!(
        !connect_blocked_derive.contains("bq_kyomi_oauth_access_ok"),
        "KYO-477: the connect_blocked derive must not read bq_kyomi_oauth_access_ok — \
         that would be the exact defect this ticket fixes"
    );

    // Exact-body check, not just "contains": a `.contains()` check alone
    // would still pass if the derive were diluted to e.g.
    // `!bq_kyomi_oauth_connect_ok.get() && !oauth_connected.get()` — which
    // reintroduces exactly the KYO-477 defect (an account-level
    // `oauth_connected` bypassing the Connect gate) without removing the
    // literal substring the checks above look for. Pin the derive's body
    // to precisely one expression.
    // `extract_between` includes the start marker itself in its result
    // (see its doc comment in `tests/mod.rs`), so strip it back off here
    // to leave just the derive's inner expression.
    let derive_body = extract_between(
        kyomi_block,
        "connect_blocked=Signal::derive(move || {",
        "})",
    )
    .trim_start_matches("connect_blocked=Signal::derive(move || {")
    .trim();
    assert_eq!(
        derive_body, "!bq_kyomi_oauth_connect_ok.get()",
        "KYO-477: the connect_blocked derive's body must be exactly \
         `!bq_kyomi_oauth_connect_ok.get()` — no additional `||`/`&&` clause folding in \
         oauth_connected or any other signal, which would silently dilute the gate back \
         toward the KYO-427 defect"
    );
}

/// Negative-space companion to the test above: none of the other four
/// `ModalOAuthStatusPanel` call sites — BigQuery enterprise_oauth,
/// Snowflake, Databricks, and Microsoft — may gain a `connect_blocked`
/// prop as a side effect of this change. The default
/// (`#[prop(default = false.into())]`) already makes an omitted prop
/// behavior-preserving, but this test would still catch a stray
/// find-and-replace that wired the KYO-408 checkbox into an unrelated
/// provider's Connect button.
#[test]
fn connect_blocked_prop_is_bigquery_kyomi_oauth_only() {
    // KYO-455: this test used to scope to `SRC.split(MOD_TESTS_MARKER).next()`
    // because `SRC` was `include_str!`-ed from this same file, and this
    // test's own marker/assertion-message literals repeat
    // "connect_url=enterprise_oauth_url" and "connect_blocked" verbatim,
    // which would have made a whole-`SRC` scan match this test's own text
    // and either miscount the call sites or pass vacuously regardless of
    // production code. Now that the test module lives in
    // `datasources/tests/` rather than inline in `datasources.rs`, `SRC`
    // contains production code only, so no split/expect workaround is
    // needed — `production_src` is just `SRC` under its old name so the
    // rest of this test's body needs no further changes.
    let production_src = SRC;

    let other_call_sites: &[(&str, &str)] = &[
        ("Snowflake", "connect_url=sf_connect_url"),
        ("Databricks", "connect_url=db_connect_url"),
    ];
    for (name, marker) in other_call_sites {
        let call = extract_between(production_src, marker, "/>");
        assert!(
            !call.contains("connect_blocked"),
            "{name}'s ModalOAuthStatusPanel call must not pass connect_blocked — \
             only BigQuery kyomi_oauth has a KYO-408 attestation gate"
        );
    }

    // "connect_url=enterprise_oauth_url" is shared verbatim by BigQuery
    // enterprise_oauth and Microsoft, so both occurrences must be
    // checked explicitly rather than relying on extract_between's
    // first-match behavior, which would silently skip the second call.
    let enterprise_offsets: Vec<usize> = production_src
        .match_indices("connect_url=enterprise_oauth_url")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(
        enterprise_offsets.len(),
        2,
        "expected exactly two connect_url=enterprise_oauth_url call sites in \
         production code (BigQuery enterprise_oauth and Microsoft) — this test's \
         bounds assume that shape and must be revisited if it changes"
    );
    for offset in enterprise_offsets {
        let tail = &production_src[offset..];
        let end = tail
            .find("/>")
            .unwrap_or_else(|| panic!("no closing /> found after offset {offset}"));
        let call = &tail[..end];
        assert!(
            !call.contains("connect_blocked"),
            "an enterprise_oauth ModalOAuthStatusPanel call site (BigQuery or \
             Microsoft) must not pass connect_blocked — only BigQuery kyomi_oauth \
             has a KYO-408 attestation gate"
        );
    }
}

/// Reachability guard, not a truth-table test: this proves the wiring
/// between the KYO-408 predicate and the Connect/Reconnect buttons
/// cannot be silently deleted from ModalOAuthStatusPanel without a test
/// failing — the acceptance-criteria bar KYO-427 sets, over and above
/// the existing truth-table coverage of
/// `bq_kyomi_oauth_access_gate_satisfied` above.
///
/// Both the "Not connected" (Connect) and "Expired" (Reconnect) branches
/// must each carry `disabled=connect_blocked` — matching how
/// `disconnect_pending` is guarded in the "Connected" branch just above
/// them. `start_connect` (the shared click handler both branches wire
/// via `on:click=start_connect`, KYO-437) must additionally early-return
/// on `connect_blocked.get_untracked()` — a `disabled` attribute alone
/// does not stop a synthetic/programmatic click — and that guard must
/// live in `start_connect` itself, not be duplicated per branch: KYO-437
/// already pins that both buttons share one handler so the popup-monitor
/// arming can't drift between them, and a per-branch copy of the guard
/// would reintroduce exactly that drift risk for this new condition.
/// If a future change removes the guard from either half, the
/// exact-count assertions below fail.
#[test]
fn modal_oauth_status_panel_gates_both_connect_and_reconnect_branches() {
    let panel_fn = extract_between(
        SRC,
        "fn ModalOAuthStatusPanel(",
        "// OAuth Status Re-fetch Hook",
    );

    let disabled_count = panel_fn.matches("disabled=connect_blocked").count();
    assert_eq!(
        disabled_count, 2,
        "expected disabled=connect_blocked on exactly two buttons (Connect and \
         Reconnect) inside ModalOAuthStatusPanel — found {disabled_count}. A count \
         of 1 means one branch lost its gate (the KYO-427 half-propagation defect); \
         a count of 0 means the gate was deleted entirely"
    );

    let click_wiring_count = panel_fn.matches("on:click=start_connect").count();
    assert_eq!(
        click_wiring_count, 2,
        "expected on:click=start_connect on exactly two buttons (Connect and \
         Reconnect) — found {click_wiring_count}. Both branches must keep sharing \
         the one handler (KYO-437) rather than each getting its own inline \
         on:click body, which is exactly the regression this change must avoid"
    );

    let start_connect_body = extract_between(
        panel_fn,
        "let start_connect = move |_: leptos::ev::MouseEvent| {",
        "view! {\n        {move || {",
    );
    let guard_count = start_connect_body
        .matches("connect_blocked.get_untracked()")
        .count();
    assert_eq!(
        guard_count, 1,
        "expected exactly one connect_blocked.get_untracked() early-return guard, \
         inside the shared start_connect closure itself — found {guard_count}. Since \
         both Connect and Reconnect wire the same start_connect (KYO-437), one guard \
         there covers both buttons; 0 means the guard was deleted, and >1 suggests it \
         was duplicated instead of shared"
    );
    assert!(
        appears_shortly_before(
            start_connect_body,
            "connect_blocked.get_untracked()",
            "set_oauth_connecting.set(true);",
            80,
        ),
        "the connect_blocked early-return must gate entry to start_connect — i.e. sit \
         before set_oauth_connecting.set(true) — not merely be present somewhere in the \
         closure"
    );

    // The "Connected" branch's Disconnect button must remain ungated by
    // connect_blocked — this predicate answers "can the user start a
    // NEW connection", which is meaningless once already connected.
    let connected_branch = extract_between(
        panel_fn,
        "// Connected state.",
        "// Expired state.",
    );
    assert!(
        !connected_branch.contains("connect_blocked"),
        "the Disconnect button (Connected branch) must not read connect_blocked — \
         that gate governs starting a new connection, not ending an existing one"
    );
}
