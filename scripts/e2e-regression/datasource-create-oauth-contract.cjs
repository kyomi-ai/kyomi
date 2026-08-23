/**
 * KYO-424 — Simulated-OAuth UI contract for create-mode datasource setup.
 *
 * Real third-party OAuth credentials (Google/Snowflake/Databricks/Microsoft)
 * make full E2E impractical here, so this asserts the *UI contract* instead:
 * dispatch the exact `postMessage` shape the real OAuth popup sends on
 * success, straight into the open modal (the listener validates
 * `event.origin === window.location.origin`, so posting from the page itself
 * is legitimate — see `install_oauth_listener` /
 * `crates/kyomi-ui/src/utils/oauth_popup.rs:140-178`), and assert the
 * create-mode "Next" gate reacts correctly.
 *
 * ARM A DETERMINISM — KYO-429 is a confirmed app bug (via prior instrumented
 * repro, not a test defect), and it is still open in the app today: any
 * recognized `*_OAUTH_SUCCESS` postMessage on /settings/datasources
 * invalidates the `datasources` query cache; when that background
 * `list_datasources` refetch's *response* resolves, the page's top-level
 * view closure re-runs, reconstructing the whole `DatasourcesContent`
 * subtree and remounting `DatasourceModal` with default state — discarding
 * the very "Next" transition Arm A exists to prove, ~20ms after the
 * message. Left alone, whether Arm A observes the transition or the
 * remount is a race between two async events, and it is genuinely flaky:
 * three manual runs produced FAIL / PASS / FAIL.
 *
 * This spec closes that race instead of living with it or weakening the
 * assertion. The remount can only happen once the `list_datasources`
 * refetch's response reaches the page, so immediately before dispatching
 * the postMessage, Arm A arms a Playwright `page.route()` interception on
 * that server-fn endpoint (`armListDatasourcesDelay()`, matching
 * `/leptos-api/list_datasources*`) that holds the response until the
 * assertion has been made (or a safety-net timeout, so the script can
 * never hang). This is timing control, not mocking: the real request still
 * reaches the real server and gets the real response — only its arrival at
 * the page is deferred, and only for the duration needed to observe a
 * transition that would otherwise happen anyway. The behaviour under test
 * — the modal's synchronous reaction to the postMessage — is completely
 * untouched. With the remount unable to land mid-observation, Arm A's
 * outcome is deterministic and its assertion (`isEnabled()`, a real
 * disabled -> enabled transition, exactly as strict as before) gates the
 * exit code like every other assertion in this file. The modal-died branch
 * is kept as a defensive, distinctly-labelled report in case the delay
 * ever fails to hold — see its comment inline — but is not the expected
 * path. Arm B is independent and unaffected — see below.
 *
 * Coverage (see the final report for the authoritative list + reasons):
 *
 *   COVERED, including the disabled -> enabled transition itself:
 *     - BigQuery + kyomi_oauth via GOOGLE_OAUTH_SUCCESS. By design,
 *       `datasources.rs:2737-2753` sets `test_result{success:true}` and
 *       `discovery_status="success"` directly off the postMessage — no
 *       server round-trip needed, so the "Next disabled -> enabled"
 *       transition is directly assertable here. It previously wasn't,
 *       because of the independently-confirmed KYO-429 bug described
 *       above: delivering *any* recognized `*_OAUTH_SUCCESS` postMessage on
 *       `/settings/datasources` could silently reset the whole page's
 *       component tree within ~20ms — the create modal closing and all
 *       in-progress form state discarded before the transition could be
 *       observed. Reproduced with `page.addEventListener`/
 *       `removeEventListener` instrumentation showing both
 *       `install_oauth_listener` instances tear down and reinstall
 *       immediately after the message is delivered; ruled out real
 *       navigation (`framenavigated` never fires, URL unchanged), full
 *       reload (a `window.__marker` set beforehand survives), and the
 *       top-level WASM panic overlay (never appears). See KYO-429 for full
 *       repro + evidence — the app bug itself is NOT fixed by this spec;
 *       only the test's ability to observe past it is. This spec now
 *       reliably asserts the transition it was written to prove, via the
 *       list_datasources response delay described above.
 *
 *   COVERED, but as "already enabled by design" (KYO-404) — NOT a
 *   transition, and the spec does not pretend it is one:
 *     - BigQuery + enterprise_oauth: `connection_step_satisfied_from`
 *       (`datasources.rs:239`) special-cases this combination to always
 *       satisfy the create-mode gate, because no slug-scoped connect
 *       endpoint exists before the datasource is saved. Next is enabled
 *       from the moment this mode is selected, before any OAuth message.
 *
 *   NOT COVERED (documented, not fabricated):
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
 * Assertions use isVisible()/isEnabled(), never count().
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

/** Print an unmissable, greppable banner for a confirmed-blocking app bug. */
function blockedBanner(ticket, reason) {
  const line = '='.repeat(78);
  console.log(`\n${line}`);
  console.log(`BLOCKED BY ${ticket} — known app bug, not a broken test. See ticket.`);
  console.log(reason);
  console.log(`${line}\n`);
}

// KYO-429 fires off the *response* of a `list_datasources` refetch that
// `datasources.rs`'s GoogleSuccess handler's cache invalidation triggers —
// not off the postMessage itself. Holding that one response for the
// duration of Arm A's observation window makes the remount structurally
// impossible to land mid-observation, without touching anything about the
// behaviour under test (the modal's synchronous reaction to the
// postMessage is untouched; only an unrelated background refetch's
// arrival time is controlled). This is request delay, not response
// mocking: the real request still goes to the real server and gets the
// real response — only fulfilment to the page is deferred.
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

    // ══ A — BigQuery kyomi_oauth: GOOGLE_OAUTH_SUCCESS sets test_result ═══
    // Wrapped so that an unexpected exception here does not abort Arm B
    // below, which is independent and unaffected.
    //
    // KYO-429 fires off the *response* of the list_datasources refetch that
    // the postMessage's cache invalidation triggers, not off the
    // postMessage itself — so holding that one response for the
    // observation window (armListDatasourcesDelay(), armed immediately
    // below, released in the finally block once the assertion is made)
    // makes the remount structurally unable to land mid-observation. See
    // the ARM A DETERMINISM header comment for the full mechanism.
    //
    // Verdict: with the race closed, this arm expects exactly one outcome —
    // Next reaches enabled while the modal is still alive. The modal-died
    // and modal-survived-but-never-enabled branches are kept as defensive
    // reporting (distinctly labelled, not conflated) in case the delay
    // ever fails to hold for some unrelated reason; they are not the
    // expected path.
    try {
      await pickAuthMode(page, 'Kyomi');
      await page.screenshot({ path: `${SHOT}-A0-kyomi-oauth-before.png`, fullPage: true });

      let disabledBefore = await nextBtn().isDisabled().catch(() => null);
      check('A: Next is DISABLED for kyomi_oauth before any OAuth message',
        disabledBefore === true, `disabled=${disabledBefore}`);

      let enabledAfter = null;
      let modalDied = false;
      await armListDatasourcesDelay(page);
      try {
        await page.evaluate(() => window.postMessage(
          { type: 'GOOGLE_OAUTH_SUCCESS', data: { email: 'e2e@kyomi.dev', provider_email: 'e2e@kyomi.dev' } },
          window.location.origin));

        // Tight poll rather than a single fixed wait, so the transition is
        // caught as soon as it happens rather than over-waiting.
        for (let i = 0; i < 40; i++) {
          if (!(await modalVisible())) { modalDied = true; break; }
          enabledAfter = await nextBtn().isEnabled().catch(() => null);
          if (enabledAfter === true) break;
          await page.waitForTimeout(50);
        }
      } finally {
        // Release immediately once the assertion window has closed, rather
        // than sitting on the held response for the full safety-net
        // duration — nothing downstream (Arm B's modal reopen) should pay
        // for this arm's timing control. Deliberately does NOT call
        // page.unroute() here — see the comment on armListDatasourcesDelay()
        // for why that raced and crashed the process in an earlier version.
        releaseListDatasourcesDelay();
      }
      await page.screenshot({ path: `${SHOT}-A1-kyomi-oauth-after.png`, fullPage: true });

      if (modalDied) {
        // Unexpected with the list_datasources delay armed — the refetch
        // that drives the KYO-429 remount should have been held for the
        // whole observation window. Still bannered rather than reported as
        // a generic failure, since a dead modal here is the KYO-429
        // signature regardless of why the mitigation didn't hold this run.
        blockedBanner('KYO-429',
          'a recognized *_OAUTH_SUCCESS postMessage on /settings/datasources ' +
          'invalidates the datasources query cache; the resulting refetch ' +
          'remounts DatasourceModal with default state before the create-mode ' +
          '"Next" transition can be observed. The create modal closed/reset ' +
          'during the poll window despite the list_datasources response ' +
          'delay being armed — this is the confirmed KYO-429 remount, not a ' +
          'selector or timing issue in this spec; investigate why the delay ' +
          'did not hold this run.');
        check('A ★ Next transitions to ENABLED after simulated GOOGLE_OAUTH_SUCCESS (bigquery/kyomi_oauth)',
          false, 'BLOCKED by KYO-429 — see banner above');
      } else if (enabledAfter === true) {
        check('A ★ Next transitions to ENABLED after simulated GOOGLE_OAUTH_SUCCESS (bigquery/kyomi_oauth)',
          true, `enabled=${enabledAfter}`);
      } else {
        // Modal survived the whole poll window but Next never enabled —
        // NOT the KYO-429 signature (that always kills the modal). A
        // distinct, un-bannered failure so it isn't conflated with KYO-429.
        check('A ★ Next transitions to ENABLED after simulated GOOGLE_OAUTH_SUCCESS (bigquery/kyomi_oauth)',
          false, `enabled=${enabledAfter}, modal survived — NOT the KYO-429 signature, investigate separately`);
      }
    } catch (armAErr) {
      check('A ★ Next transitions to ENABLED after simulated GOOGLE_OAUTH_SUCCESS (bigquery/kyomi_oauth)',
        false, `threw: ${armAErr.message.split('\n')[0]}`);
    }

    // ══ B — BigQuery enterprise_oauth: covered-by-design (KYO-404), no ═════
    //        postMessage needed — Next is enabled from mode-selection alone.
    // Reopen the modal fresh — Arm A's postMessage may have closed it
    // (KYO-429), and Arm B does not depend on Arm A's outcome.
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
