/**
 * KYO-424 — Simulated-OAuth UI contract for create-mode datasource setup.
 *
 * Real third-party OAuth credentials (Google/Snowflake/Databricks/Microsoft)
 * make full E2E impractical here, so this asserts the *UI contract* instead:
 * dispatch the exact `postMessage` shape the real OAuth popup sends on
 * success, straight into the open modal (the listener validates
 * `event.origin === window.location.origin`, so posting from the page itself
 * is legitimate — see `install_oauth_listener` /
 * `crates/kyomi-ui/src/utils/oauth_popup.rs:522-551`), and assert the
 * create-mode "Next" gate reacts correctly.
 *
 * KYO-473 RETARGET (this revision): Arm A used to dispatch
 * `GOOGLE_OAUTH_SUCCESS` against BigQuery's `kyomi_oauth` Authentication
 * Mode. KYO-704 (`686c9a91`, PR #513) removed `kyomi_oauth` from
 * `BIGQUERY_META.auth_modes` entirely (`crates/kyomi-core/src/
 * datasource_registry.rs`) — it is no longer a selectable option in the
 * Authentication Mode dropdown at all (confirmed live: the dropdown now
 * lists exactly `"Google OAuth (Enterprise)"` and
 * `"Service Account (Recommended)"`), so the old `pickAuthMode(page,
 * 'Kyomi')` call threw a Playwright timeout before Arm A ever reached its
 * assertions. Arm A now selects BigQuery's `enterprise_oauth` mode and
 * dispatches `BIGQUERY_ENTERPRISE_OAUTH_SUCCESS` instead — see the ARM A
 * DETERMINISM and Coverage sections below for why that changes what the
 * arm's secondary assertion can honestly claim.
 *
 * ARM A DETERMINISM — KYO-429 was a confirmed app bug (via prior
 * instrumented repro, not a test defect): any recognized `*_OAUTH_SUCCESS`
 * postMessage on /settings/datasources invalidated the `datasources` query
 * cache, and when that background `list_datasources` refetch's *response*
 * resolved, the page's top-level view closure re-ran unconditionally,
 * reconstructing the whole `DatasourcesContent` subtree and remounting
 * `DatasourceModal` with default state — discarding whatever in-progress
 * create-mode state Arm A was observing, ~20ms after the message. Left
 * alone, whether Arm A observed the modal intact or the remount was a race
 * between two async events, and it was genuinely flaky: three manual runs
 * (pre-fix, against the original `GOOGLE_OAUTH_SUCCESS`/`kyomi_oauth`
 * pairing) produced FAIL / PASS / FAIL.
 *
 * KYO-429 is now FIXED, merged as `c2800fca` (PR #374): `DatasourcesPage`'s
 * view closure (`crates/kyomi-ui/src/pages/settings/datasources.rs`) now
 * branches on a `Memo<DatasourcesViewState>` that collapses every
 * `datasources_signal` write into Loading/Ready/Failed, instead of matching
 * on a raw tracked `datasources_signal.get()` read. A `Memo` only notifies
 * on a PartialEq-unequal output, so a `Some(Ok(_))` -> `Some(Ok(_))`
 * refetch — exactly what a cache invalidation with unchanged data
 * produces — no longer flips the branch, and `DatasourcesContent`/
 * `DatasourceModal` are never rebuilt out from under an open modal's
 * unsaved input. The fix is generic over *which* recognized
 * `*_OAUTH_SUCCESS` message triggered the invalidation — the list-level
 * listener (`datasources.rs`, ~L740-750) calls
 * `query_cache.invalidate("datasources")` identically for
 * `GoogleSuccess`/`SnowflakeSuccess`/`DatabricksSuccess`/`MicrosoftSuccess`/
 * `MicrosoftEnterpriseSuccess`/`BigqueryEnterpriseSuccess` — so
 * `BIGQUERY_ENTERPRISE_OAUTH_SUCCESS` exercises exactly the same
 * invalidate-then-refetch path `GOOGLE_OAUTH_SUCCESS` did and is an
 * equally valid KYO-429 regression guard.
 *
 * This spec keeps two layers of protection rather than trusting the fix
 * blindly. First, it still arms a Playwright `page.route()` interception on
 * the `list_datasources` server-fn endpoint (`armListDatasourcesDelay()`,
 * matching `/leptos-api/list_datasources*`) that holds the refetch response
 * for the duration of Arm A's observation window (or a safety-net timeout,
 * so the script can never hang) — this is timing control, not mocking: the
 * real request still reaches the real server and gets the real response,
 * only its arrival at the page is deferred. Second, and this is the AC2
 * addition, Arm A asserts directly on the regression this bug caused, not
 * just on a downstream symptom: after the postMessage, it asserts the
 * create modal is still open AND that the Name field it was seeded with
 * earlier still holds its typed value — the exact form-state-loss scenario
 * KYO-429 described. If the modal-survival guard below is ever tripped
 * again, or the Name field comes back empty, that is a KYO-429 regression,
 * not an accepted race outcome — see `regressionBanner()` at that call
 * site. Arm B is independent and unaffected — see below.
 *
 * KYO-473 hardening — the modal-survival guard is DOM-node-identity based,
 * not text/visibility based. An earlier revision asked "is `text=Connection
 * Method` visible?" after the postMessage and treated `true` as proof the
 * original modal survived untouched. Verified against a deliberately
 * reintroduced KYO-429 regression (a raw tracked `match
 * datasources_signal.get()` in `DatasourcesPage`, replacing the `Memo`
 * branch above — the exact pre-fix shape), that check is vacuous: it reads
 * PASS (modal "visible") on every run even though the Name field has
 * already lost its typed value. The reason is structural, not timing luck:
 * a remounted `DatasourceModal` starts CLOSED — `modal_datasource_id`
 * resets to its fresh-signal default of `None` (`datasources.rs` ~L628),
 * and `Modal`'s entire body is gated behind `<Show when=show>`
 * (`crates/kyomi-ui-components/src/components/modal.rs` ~L176 — a `false`
 * show renders nothing) — so a genuinely fresh post-remount instance has no
 * "Connection Method" text to find at all. A `true` visibility reading
 * during/just after a regression is therefore never a freshly-reopened
 * modal; it can only be the OLD, about-to-be-torn-down instance's DOM,
 * sampled in the async gap between the deferred `list_datasources`
 * response landing and Leptos actually swapping the subtree. A text query
 * cannot distinguish "the modal I started with, still alive" from "debris
 * of that modal, moments from being replaced by nothing" — a DOM node
 * reference can. This revision tags the Name `<input>` element itself
 * (`nameInput.elementHandle()`, captured right before the postMessage) and
 * polls that *same JS object's* `.isConnected` — true only for as long as
 * that exact node stays attached to the document. A remount necessarily
 * constructs a brand-new input element (a fresh `DatasourceModal` instance
 * re-declares `let (name, set_name) = signal(String::new())` and its own
 * view from scratch; the old element is never reused), so `.isConnected` on
 * the pre-message handle flips to `false` the moment the swap actually
 * lands, regardless of what a text query happens to read at that instant.
 * On the fixed build this same check is expected to stay connected for the
 * whole observation window: the Memo branch above means the subtree — and
 * therefore this exact input node — is never rebuilt at all for a
 * `Some(Ok(_))` -> `Some(Ok(_))` refetch, so nothing ever detaches it.
 *
 * Coverage (see the final report for the authoritative list + reasons):
 *
 *   COVERED — the KYO-429 modal-survival guard (primary), plus a
 *   remount-detecting continuity check (secondary), NOT a disabled ->
 *   enabled transition:
 *     - BigQuery + enterprise_oauth via BIGQUERY_ENTERPRISE_OAUTH_SUCCESS.
 *       `connection_step_satisfied_from` (`datasources.rs:342-353`)
 *       special-cases `enterprise_oauth` to always satisfy the create-mode
 *       gate the instant the mode is selected — no slug-scoped connect
 *       endpoint exists before the datasource is saved, so Next is already
 *       ENABLED before any OAuth message reaches the page (confirmed live:
 *       selecting "Google OAuth (Enterprise)" alone enables Next). There is
 *       therefore no disabled -> enabled transition left to assert for this
 *       pair, and the spec does not pretend otherwise. What IS asserted:
 *       (1) the KYO-429 pair — modal survives (DOM-identity check, see the
 *       KYO-473 hardening paragraph above), Name field retained — after
 *       `BIGQUERY_ENTERPRISE_OAUTH_SUCCESS` triggers the same cache
 *       invalidation + `list_datasources` refetch the original bug rode in
 *       on; and (2) a KYO-404 design check that Next *stays* enabled
 *       through that window, run ONLY when (1) confirms no remount
 *       happened. (2) is explicitly NOT a second KYO-429 detector — an
 *       earlier revision claimed a remount would "flip Next to disabled"
 *       (reasoning: a fresh instance's `bq_auth_mode` defaults back to
 *       `service_account`, whose gate needs an actual successful test).
 *       Verified against a reintroduced regression, that reasoning doesn't
 *       hold and the claim was retracted: `connection_step_satisfied_from`
 *       (`datasources.rs:342-353`) resolves the (bigquery, enterprise_oauth)
 *       pair to `OAuthStatusSource::Datasource(_)`, which returns `true`
 *       unconditionally regardless of `bq_auth_mode`'s current value — and
 *       in any case a genuine remount doesn't reopen the modal at a
 *       default auth mode with Next visibly disabled, it closes the whole
 *       modal (`modal_datasource_id` resets to `None`, gating `Modal`'s
 *       entire body via `<Show>` — see the hardening paragraph above), so
 *       there is no "Next" button left to read at all. What was actually
 *       observed reading `enabled=true` during a live regression was the
 *       OLD, about-to-be-torn-down instance's DOM, not survived state —
 *       exactly the same artifact the text-visibility check suffered from.
 *       This is why (2) only runs once (1) has confirmed the modal wasn't
 *       replaced: without that guard its reading is meaningless noise, not
 *       evidence either way.
 *
 *   COVERED, but as "already enabled by design" (KYO-404) — NOT a
 *   transition, and the spec does not pretend it is one (Arm B):
 *     - BigQuery + enterprise_oauth, mode-selection alone, no postMessage:
 *       the same `connection_step_satisfied_from` special-case as above,
 *       asserted directly with no OAuth message involved.
 *
 *   NOT COVERED (documented, not fabricated):
 *     - BigQuery kyomi_oauth (GOOGLE_OAUTH_SUCCESS) — retired by KYO-704
 *       (`686c9a91`, PR #513): removed from `BIGQUERY_META.auth_modes`
 *       (`crates/kyomi-core/src/datasource_registry.rs`), so it is no
 *       longer a selectable Authentication Mode in the create-mode UI at
 *       all. This was previously the one arm with a genuine, directly
 *       assertable disabled -> enabled Next transition driven purely by a
 *       simulated postMessage (`datasources.rs`'s modal-level listener sets
 *       `test_result{success:true}` off `GoogleSuccess` with no server
 *       round-trip, and `service_account`/`enterprise_oauth` are the only
 *       modes left, per the "already enabled by design" and "needs a real
 *       test" entries above and below respectively) — with it gone, no
 *       create-mode auth mode can reach "Next enabled" from a simulated
 *       message any more. This is a finding, not a gap papered over: the
 *       modal-level listener still sets `test_result`/`modal_oauth_connected`
 *       off `GoogleSuccess`/`BigqueryEnterpriseSuccess` unconditionally,
 *       regardless of which auth mode is currently selected (`datasources.rs`
 *       ~L3707-3726) — which means dispatching either message while
 *       `service_account` (the current default) is selected would flip
 *       `test_succeeded` to `true` and enable Next for a mode with no
 *       actual validated credential. That cross-mode leakage was
 *       deliberately NOT used to manufacture a transition here — asserting
 *       against it would test an incidental side effect instead of a
 *       designed gate, exactly the kind of fake result this suite must not
 *       produce. Flagged out of scope in the KYO-473 report as a possible
 *       latent bug worth its own ticket.
 *     - Snowflake oauth (SNOWFLAKE_OAUTH_SUCCESS)
 *     - Databricks oauth (DATABRICKS_OAUTH_SUCCESS)
 *     - Synapse enterprise_oauth (MICROSOFT_ENTERPRISE_OAUTH_SUCCESS)
 *     These three arms only set `modal_oauth_connected`/`modal_oauth_email`
 *     off the postMessage and then call `do_test_and_discover()` — the real
 *     Test & Discover action, which needs a reachable account/warehouse and
 *     cannot be made to report success without real third-party credentials.
 *     Simulating success here would either hang on a real network call or
 *     require faking the server response, which is exactly the kind of fake
 *     result this suite must not produce. Worse, in *create* mode the OAuth
 *     status panel that would otherwise show "Connected" is itself hidden
 *     (`is_create_mode` gate in e.g. `SnowflakeAuthModeSection`, only a
 *     static "connect after saving" message renders), so there is no
 *     create-mode-visible signal at all to assert against for these three
 *     besides Next — which needs the real test. Left uncovered.
 *     - Microsoft OAuth (`MICROSOFT_OAUTH_SUCCESS`) — this message type has
 *     no BigQuery/Snowflake/Databricks/Synapse consumer in the datasource
 *     modal at all (`OAuthMessage::MicrosoftSuccess` only clears the
 *     "connecting" flag); it isn't wired to any create-mode gate to assert.
 *
 * Assertions use isVisible()/isEnabled()/isConnected (the last via a
 * pre-tagged elementHandle — see the KYO-473 hardening paragraph above),
 * never count().
 */
const { chromium } = require('playwright');

// Overrides (all optional — defaults target local dev):
//   E2E_BASE_URL        - app base URL          (default http://localhost:3000)
//   E2E_ADMIN_EMAIL     - admin login email     (default e2e-admin@kyomi.dev)
//   E2E_ADMIN_PASSWORD  - admin login password  (default E2eAdminPass123!)
const BASE = process.env.E2E_BASE_URL || 'http://localhost:3000';
const ADMIN_EMAIL = process.env.E2E_ADMIN_EMAIL || 'e2e-admin@kyomi.dev';
const ADMIN_PASSWORD = process.env.E2E_ADMIN_PASSWORD || 'E2eAdminPass123!';
const SHOT = '/tmp/ds-create-oauth';
const results = [];

// Glob for the list_datasources server-fn endpoint. Confirmed at runtime via
// the diagnostic log in armListDatasourcesDelay() below rather than assumed
// — leptos #[server] fns without an explicit `endpoint = ...` are named
// after the fn, so this is `/leptos-api/list_datasources`, but the trailing
// `*` tolerates any suffix Playwright's request actually carries.
const LIST_DATASOURCES_ROUTE = '**/leptos-api/list_datasources*';
// Safety-net cap on how long a held response can be delayed — comfortably
// exceeds Arm A's 2s observation poll (40 x 50ms) so the hold never expires
// mid-observation, but guarantees the script can never hang even if release
// is never explicitly called (e.g. an unexpected throw between arm/release).
const REFETCH_HOLD_MS = 3000;

function check(name, pass, detail) {
  results.push({ name, pass: !!pass, detail: detail || '' });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
}
const vis = async (loc) => loc.isVisible().catch(() => false);

/** Print an unmissable, greppable banner for a KYO-429 regression sighting. */
function regressionBanner(ticket, reason) {
  const line = '='.repeat(78);
  console.log(`\n${line}`);
  console.log(`${ticket} REGRESSION — this was fixed; something reintroduced it. See ticket.`);
  console.log(reason);
  console.log(`${line}\n`);
}

// Pre-fix, KYO-429 fired off the *response* of a `list_datasources` refetch
// that `datasources.rs`'s GoogleSuccess handler's cache invalidation
// triggers — not off the postMessage itself — and that response arriving
// remounted DatasourcesContent/DatasourceModal out from under the open
// modal. Holding that one response for the duration of Arm A's observation
// window made the remount structurally impossible to land mid-observation.
// With KYO-429's root cause fixed (DatasourcesPage now branches on a
// Memo<DatasourcesViewState>, so that refetch's arrival no longer changes
// the branch at all), this delay is no longer required for correctness —
// it is kept deliberately, as a regression guard: belt-and-braces so the
// assertion below stays deterministic even if the Memo branch is ever
// weakened, cheap insurance that costs nothing else in the script (the
// DOM-identity modal-survival / Name-field-retained assertions added
// alongside it are the primary guard — see the KYO-473 hardening
// paragraph in the header comment for why identity, not visibility, is
// what actually catches a remount). This is request delay, not response mocking: the
// real request still goes to the real server and gets the real response —
// only fulfilment to the page is deferred.
//
// Deliberately a boolean latch polled from inside the handler, not a
// per-request Promise resolved from outside: an earlier version resolved a
// stored Promise and then called page.unroute() immediately afterward,
// which raced Playwright's own internal teardown of the in-flight route
// against this handler's still-pending route.continue() call and crashed
// the process with "Route is already handled!" — an uncaught rejection
// inside a route handler kills the whole script, bypassing every check()
// and the exit-code accounting in the finally block below. The route is
// left registered for the rest of the script; once released, it degrades
// to a bounded (<=20ms) pass-through rather than needing to be torn down.
let delayActive = false;
function releaseListDatasourcesDelay() {
  delayActive = false;
}
async function armListDatasourcesDelay(page) {
  delayActive = true;
  await page.route(LIST_DATASOURCES_ROUTE, async (route) => {
    console.log(`[list_datasources delay armed] holding ${route.request().url()}`);
    const deadline = Date.now() + REFETCH_HOLD_MS;
    while (delayActive && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 20));
    }
    await route.continue().catch(() => {});
  });
}

async function pickAuthMode(page, label) {
  const trigger = page.locator('label:has-text("Authentication Mode")')
    .locator('xpath=following-sibling::*[1]')
    .locator('button[aria-haspopup="listbox"]');
  await trigger.click({ timeout: 10000 });
  await page.locator('[role="option"]', { hasText: label }).first().click({ timeout: 10000 });
  await page.waitForTimeout(800);
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const page = await ctx.newPage();
  // "Failed to load resource" lines are Chromium's own generic echo of any
  // non-2xx HTTP response — already captured with full detail in failedReqs
  // below. Keeping them here too would make this check redundant with (and
  // strictly noisier than) that array, and would flag expected pre-auth 401s
  // and the expected get_google_oauth_projects 500 (Arm A's fake email has
  // no real Google token to look up projects with — expected, not a bug) as
  // if they were real JS/WASM errors. Only genuine console.error text and
  // real JS exceptions belong here.
  const consoleErrors = [];
  page.on('console', m => {
    if (m.type() === 'error' && !/^Failed to load resource:/.test(m.text())) {
      consoleErrors.push(m.text());
    }
  });
  const failedReqs = [];
  page.on('response', r => {
    if (r.status() >= 400) {
      const url = r.url();
      // KYO-426 (known, out of scope): create mode fires a datasource-scoped
      // OAuth status fetch for a datasource that doesn't exist yet -> 500.
      if (url.includes('oauth') && url.includes('status')) return;
      failedReqs.push(`${r.status()} ${url}`);
    }
  });
  page.on('pageerror', e => consoleErrors.push('PAGEERROR: ' + e.message));

  try {
    // ── Login ──────────────────────────────────────────────────────────
    await page.goto(`${BASE}/login`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.fill('input[type="email"]', ADMIN_EMAIL, { timeout: 10000 });
    await page.fill('input[type="password"]', ADMIN_PASSWORD, { timeout: 10000 });
    await page.click('button[type="submit"]', { timeout: 10000 });
    await page.waitForURL(u => !u.toString().includes('/login'), { timeout: 20000 });
    check('login as admin', true);

    // ── Open create modal (default type is BigQuery — no Type switch needed) ──
    await page.goto(`${BASE}/settings/datasources`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.waitForTimeout(3000);
    await page.locator('button:has-text("Add Datasource")').first().click({ timeout: 10000 });
    await page.waitForTimeout(1500);
    check('create modal opens', await vis(page.locator('text=Connection Method')));

    const nameInput = page.locator('input[placeholder="Production Database"]').first();
    await nameInput.fill('E2E OAuth Contract', { timeout: 10000 });
    check('name field filled', (await nameInput.inputValue()) === 'E2E OAuth Contract');

    const nextBtn = () => page.locator('button:has-text("Next")').last();
    const modalVisible = () => page.locator('text=Connection Method').isVisible().catch(() => false);

    // ══ A — BigQuery enterprise_oauth: BIGQUERY_ENTERPRISE_OAUTH_SUCCESS ══
    //        triggers the same cache-invalidate + refetch KYO-429 rode in
    //        on; Next itself is already enabled by mode selection alone
    //        (KYO-404 — see Arm B) so this is a survival/continuity guard,
    //        not a disabled -> enabled transition. See the KYO-473 RETARGET
    //        and Coverage sections in the header comment for why
    //        `kyomi_oauth`/GOOGLE_OAUTH_SUCCESS could not stay Arm A's
    //        vehicle (retired by KYO-704) and why enterprise_oauth is the
    //        correct replacement rather than a weaker substitute.
    //
    // Wrapped so that an unexpected exception here does not abort Arm B
    // below, which is independent and unaffected.
    //
    // KYO-429 used to fire off the *response* of the list_datasources
    // refetch that the postMessage's cache invalidation triggers, not off
    // the postMessage itself — so holding that one response for the
    // observation window (armListDatasourcesDelay(), armed immediately
    // below, released in the finally block once the assertion is made)
    // made the remount structurally unable to land mid-observation. With
    // KYO-429 fixed, that remount cannot happen at all regardless of this
    // delay — see the ARM A DETERMINISM header comment for the full
    // mechanism and why the delay is kept anyway, as a regression guard.
    //
    // Verdict: this arm expects exactly one outcome — the Name-input DOM
    // node survives untouched (KYO-429), still showing its typed value,
    // and (only meaningful once that's confirmed) Next reads enabled per
    // KYO-404. The remounted branch is kept as defensive reporting,
    // distinctly labelled, in case KYO-429 or something with the same
    // signature ever regresses; it is not the expected path.
    try {
      await pickAuthMode(page, 'Enterprise');
      await page.screenshot({ path: `${SHOT}-A0-bigquery-enterprise-oauth-before.png`, fullPage: true });

      // Not a "before" state in the disabled -> enabled sense — enterprise_
      // oauth's create-mode gate is satisfied the instant the mode is
      // selected (KYO-404, `connection_step_satisfied_from`), before any
      // OAuth message. Asserted here anyway so a regression that somehow
      // ties this pair's create-mode gate back to needing the message
      // would show up as a baseline failure, not just an after-message one.
      let enabledBefore = await nextBtn().isEnabled().catch(() => null);
      check('A: Next is already ENABLED for bigquery/enterprise_oauth before any OAuth message (KYO-404 design)',
        enabledBefore === true, `enabled=${enabledBefore}`);

      // Tag the live Name <input> DOM node itself, right before the
      // postMessage, so the guard below can tell "this exact modal, still
      // alive" from "the text/controls of A modal, possibly a fresh
      // instance" — see the KYO-473 hardening paragraph in the header
      // comment for why a text/visibility read cannot make that
      // distinction, and why this elementHandle can.
      const nameInputHandle = await nameInput.elementHandle();
      const nameInputStillConnected = () =>
        nameInputHandle.evaluate((el) => el.isConnected).catch(() => false);

      let enabledAfter = null;
      let remounted = false;
      let textAlsoGoneAtPoll = null; // diagnostic only — see header comment
      await armListDatasourcesDelay(page);
      try {
        await page.evaluate(() => window.postMessage(
          { type: 'BIGQUERY_ENTERPRISE_OAUTH_SUCCESS', data: { email: 'e2e@kyomi.dev', provider_email: 'e2e@kyomi.dev' } },
          window.location.origin));

        // Tight poll for the full observation window rather than a single
        // fixed wait. Unlike the retired kyomi_oauth arm, there is no
        // disabled -> enabled transition to wait for here (Next is already
        // enabled) — this loop instead gives the deliberately-held
        // list_datasources refetch its full window to resolve and, if
        // KYO-429 has regressed, remount the modal out from under this
        // observation. The identity check is checked FIRST and is the only
        // thing that breaks the loop on its own — it is the authoritative
        // signal (see header comment). `enabledAfter` is still sampled
        // each tick for the KYO-404 check below, but is not trusted to
        // detect a remount by itself.
        for (let i = 0; i < 40; i++) {
          if (!(await nameInputStillConnected())) { remounted = true; break; }
          if (textAlsoGoneAtPoll === null && !(await modalVisible())) { textAlsoGoneAtPoll = i; }
          // Explicit short timeout: isEnabled() (unlike isVisible()) auto-waits
          // up to its default timeout if the element vanishes, so a remount
          // landing in this gap could otherwise stall this iteration up to 30s.
          enabledAfter = await nextBtn().isEnabled({ timeout: 200 }).catch(() => null);
          await page.waitForTimeout(50);
        }
      } finally {
        // Release immediately once the observation window has closed,
        // rather than sitting on the held response for the full
        // safety-net duration — nothing downstream (Arm B's modal reopen)
        // should pay for this arm's timing control. Deliberately does NOT
        // call page.unroute() here — see the comment on
        // armListDatasourcesDelay() for why that raced and crashed the
        // process in an earlier version.
        releaseListDatasourcesDelay();
      }

      // The delay is released above, but the deferred list_datasources
      // response hasn't necessarily reached the page and been processed
      // yet — give it a bounded extra window to land before taking the
      // final reading, so "survived" means "survived the actual refetch
      // landing," not "survived only until this script happened to stop
      // polling." Harmless on the fixed build too: the Memo branch means
      // this can only ever time out without finding a remount.
      if (!remounted) {
        for (let i = 0; i < 20; i++) {
          if (!(await nameInputStillConnected())) { remounted = true; break; }
          await page.waitForTimeout(50);
        }
      }
      await page.screenshot({ path: `${SHOT}-A1-bigquery-enterprise-oauth-after.png`, fullPage: true });

      // ── KYO-429 regression guard: Name-input DOM node survives intact ──
      // This is the direct assertion of the bug KYO-429 described — the
      // create modal (and its in-progress, unsaved Name field) must
      // survive a *_OAUTH_SUCCESS postMessage unchanged, proven by DOM
      // node identity rather than a text/visibility read that a stale,
      // about-to-be-torn-down instance can satisfy just as well as a
      // genuinely untouched one (see header comment).
      check('A: create modal\'s Name-input DOM node is NOT remounted after simulated BIGQUERY_ENTERPRISE_OAUTH_SUCCESS (KYO-429 regression guard)',
        !remounted,
        `remounted=${remounted}` + (textAlsoGoneAtPoll !== null
          ? `; note: "Connection Method" text also read not-visible at poll #${textAlsoGoneAtPoll} (diagnostic only, not authoritative)`
          : ''));

      const nameValueAfter = remounted ? null : await nameInput.inputValue().catch(() => null);
      check('A: Name field still holds its typed value after simulated BIGQUERY_ENTERPRISE_OAUTH_SUCCESS (KYO-429 regression guard)',
        !remounted && nameValueAfter === 'E2E OAuth Contract',
        remounted ? 'modal remounted — cannot read Name field' : `value=${JSON.stringify(nameValueAfter)}`);

      if (remounted) {
        // Unexpected with the list_datasources delay armed AND with the
        // KYO-429 root cause fixed — DatasourcesPage no longer remounts
        // DatasourcesContent on a Some(Ok(_)) -> Some(Ok(_)) refetch at
        // all, so nothing should be able to tear down the modal here
        // regardless of the delay. Still bannered rather than reported as
        // a generic failure, since a remount here is the KYO-429
        // signature regardless of what let it back in.
        regressionBanner('KYO-429',
          'a recognized *_OAUTH_SUCCESS postMessage on /settings/datasources ' +
          'caused the create modal\'s Name-input DOM node to be replaced ' +
          '(a remount), detected by DOM node identity (isConnected), not ' +
          'text visibility. KYO-429 fixed exactly this: DatasourcesPage ' +
          '(crates/kyomi-ui/src/pages/settings/datasources.rs) now ' +
          'branches its view on a Memo<DatasourcesViewState> so a ' +
          'Some(Ok(_)) -> Some(Ok(_)) list_datasources refetch no longer ' +
          'changes the branch and DatasourcesContent/DatasourceModal are ' +
          'never rebuilt. Seeing a remount here — with the list_datasources ' +
          'response delay still armed — means that fix has regressed; ' +
          'investigate DatasourcesPage before assuming this is a test issue.');
        // KYO-404 design check, NOT a second KYO-429 detector — see header
        // comment. Once the modal has been proven remounted, any reading
        // of Next taken during/after that window is unreliable (it may be
        // sampling the old, about-to-be-torn-down instance, or nothing at
        // all) and asserts nothing about KYO-404 either way. Reported
        // failed for visibility alongside the banner, not because Next
        // itself was shown to misbehave.
        check('A (KYO-404 design check, NOT a KYO-429 detector): Next-enabled reading for bigquery/enterprise_oauth after simulated BIGQUERY_ENTERPRISE_OAUTH_SUCCESS',
          false, 'KYO-429 REGRESSION — see banner above; last sampled enabled=' + enabledAfter + ' is not trustworthy once the modal has remounted');
      } else if (enabledAfter === true) {
        check('A (KYO-404 design check, NOT a KYO-429 detector): Next remains ENABLED for bigquery/enterprise_oauth after simulated BIGQUERY_ENTERPRISE_OAUTH_SUCCESS',
          true, `enabled=${enabledAfter}`);
      } else {
        // The Name-input node was never replaced, so this is a genuine,
        // standalone KYO-404 regression — independent of KYO-429, and not
        // bannered as one.
        check('A (KYO-404 design check, NOT a KYO-429 detector): Next remains ENABLED for bigquery/enterprise_oauth after simulated BIGQUERY_ENTERPRISE_OAUTH_SUCCESS',
          false, `enabled=${enabledAfter} while the modal's Name-input node was confirmed NOT remounted — investigate connection_step_satisfied_from / bq_auth_mode directly, this is not a KYO-429 signature`);
      }
    } catch (armAErr) {
      check('A (KYO-404 design check, NOT a KYO-429 detector): Next remains ENABLED for bigquery/enterprise_oauth after simulated BIGQUERY_ENTERPRISE_OAUTH_SUCCESS',
        false, `threw: ${armAErr.message.split('\n')[0]}`);
    }

    // ══ B — BigQuery enterprise_oauth: covered-by-design (KYO-404), no ═════
    //        postMessage needed — Next is enabled from mode-selection alone.
    // Reopen the modal fresh if it isn't already — defensive: with KYO-429
    // fixed, Arm A's postMessage should no longer be able to close it, but
    // Arm B doesn't depend on Arm A's outcome either way, so this keeps
    // Arm B independent even if Arm A hit its own unrelated failure above.
    if (!(await modalVisible())) {
      await page.locator('button:has-text("Add Datasource")').first().click({ timeout: 10000 });
      await page.waitForTimeout(1500);
      const reopened = await modalVisible();
      check('modal reopened for Arm B (independent of Arm A)', reopened);
      const nameInput2 = page.locator('input[placeholder="Production Database"]').first();
      await nameInput2.fill('E2E OAuth Contract B', { timeout: 10000 });
    }

    await pickAuthMode(page, 'Enterprise');
    await page.waitForTimeout(500);
    await page.screenshot({ path: `${SHOT}-B-enterprise-oauth.png`, fullPage: true });

    let enterpriseEnabled = await nextBtn().isEnabled().catch(() => null);
    check('B: Next is ENABLED for bigquery/enterprise_oauth by design (KYO-404 precreate exception, not a postMessage-driven transition)',
      enterpriseEnabled === true, `enabled=${enterpriseEnabled}`);

    check('no hydration panics / console errors', consoleErrors.length === 0,
      consoleErrors.slice(0, 3).join(' | '));

  } catch (e) {
    check('script completed without throwing', false, e.message.split('\n')[0]);
    await page.screenshot({ path: `${SHOT}-ERROR.png`, fullPage: true }).catch(() => {});
  } finally {
    console.log('\n--- HTTP >=400 ---');
    failedReqs.forEach(f => console.log('  ' + f));
    console.log('\n--- Uncovered arms (documented, not asserted) ---');
    console.log('  - bigquery/kyomi_oauth (GOOGLE_OAUTH_SUCCESS) — retired by KYO-704 (686c9a91, PR #513); no longer a selectable Authentication Mode');
    console.log('  - snowflake/oauth (SNOWFLAKE_OAUTH_SUCCESS) — needs real Snowflake account for do_test_and_discover()');
    console.log('  - databricks/oauth (DATABRICKS_OAUTH_SUCCESS) — needs real Databricks warehouse for do_test_and_discover()');
    console.log('  - synapse/enterprise_oauth (MICROSOFT_ENTERPRISE_OAUTH_SUCCESS) — needs real Synapse account for do_test_and_discover()');
    console.log('  - MICROSOFT_OAUTH_SUCCESS — not wired to any create-mode gate in the datasource modal');
    const failed = results.filter(r => !r.pass);
    console.log(`\n===== ${results.length - failed.length}/${results.length} passed =====`);
    if (failed.length) { console.log('FAILURES:'); failed.forEach(f => console.log(`  - ${f.name}  ${f.detail}`)); }
    await browser.close();
    process.exit(failed.length ? 1 : 0);
  }
})();
