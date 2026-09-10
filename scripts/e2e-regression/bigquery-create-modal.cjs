/**
 * BigQuery create-modal E2E — a customer's reported defect.
 * Covers KYO-404, 405, 408, 411, 413, 417.
 *
 * Assertions use isVisible(), never count(): count() matches hidden DOM and
 * would pass on a control the user cannot actually see — which is precisely
 * the defect being tested ("the control never appears").
 *
 * STALE ASSERTIONS (KYO-602) — this spec has NOT been run since the changes
 * below landed. The stale selectors were deliberately NOT rewritten here:
 * choosing correct replacement selectors requires driving the modal in a
 * real browser, which this pass did not do. Reconciling and running the spec
 * is tracked as KYO-604. Until then, treat a failure on any of these lines
 * as expected, not as a new regression:
 *
 *   - `text=Google account authorization required` (section A) — DEAD.
 *     KYO-705 removed the entire KYO-408/KYO-499 Google-OAuth-allowlist
 *     attestation notice and confirmation checkbox, once Kyomi's Google
 *     OAuth app left Testing publishing status in the Google Cloud
 *     Console (Google no longer refuses un-allowlisted accounts, so there
 *     was nothing left to attest to). The module that owned the shared
 *     copy/persistence for that notice is gone. The checks this spec ran
 *     against that notice were replaced with absence checks — see
 *     section A below.
 *   - `button:has-text("Request access")` (section D only — the KYO-417
 *     feedback-modal trigger, unrelated to the removed attestation
 *     notice) — STALE.
 *   - `text=Request BigQuery Access` (section D, KYO-417) — WILL BECOME
 *     STALE. Passes today, but KYO-504's PR #457 is open and removes the
 *     access-request feedback type entirely.
 *   - `text=Default Project` (section B) — DEAD. The field was removed
 *     outright by KYO-415 (`1f27f54c`, PR #410), enforced by
 *     `bigquery_default_project_field_is_gone_billing_project_survives`
 *     (`crates/kyomi-ui/src/pages/settings/datasources/tests/auth_mode_sections.rs:509`).
 *     Unlike the other four, this one needed no browser to establish — a
 *     merged unit test proves it.
 *
 *   - `check('KYO-404 Next is disabled for kyomi_oauth with no Google
 *     connection', ...)` (section A) — WILL BECOME STALE. KYO-704 retired
 *     BigQuery's `kyomi_oauth` auth mode and made `service_account` the
 *     create-mode default (`BIGQUERY_DEFAULT_AUTH_MODE`,
 *     `crates/kyomi-ui/src/pages/settings/datasources.rs`), so a freshly
 *     opened create modal no longer lands on the kyomi_oauth panel by
 *     default — this section needs to explicitly select an auth mode
 *     before relying on kyomi_oauth-specific UI, or be retargeted at
 *     `enterprise_oauth`/`service_account`. Not rewritten here — same
 *     "needs a real browser" reasoning as the rest of this list; tracked
 *     alongside KYO-604.
 *
 * Still present on `main` and not affected: `Connect BigQuery`,
 * `Validate & Discover Projects`, `Billing Project`.
 */
const { chromium } = require('playwright');

// Overrides (all optional — defaults target local dev):
//   E2E_BASE_URL        - app base URL          (default http://localhost:3000)
//   E2E_ADMIN_EMAIL     - admin login email     (default e2e-admin@kyomi.dev)
//   E2E_ADMIN_PASSWORD  - admin login password  (default E2eAdminPass123!)
const BASE = process.env.E2E_BASE_URL || 'http://localhost:3000';
const ADMIN_EMAIL = process.env.E2E_ADMIN_EMAIL || 'e2e-admin@kyomi.dev';
const ADMIN_PASSWORD = process.env.E2E_ADMIN_PASSWORD || 'E2eAdminPass123!';
const SHOT = '/tmp/bq-e2e';
const results = [];

function check(name, pass, detail) {
  results.push({ name, pass: !!pass, detail: detail || '' });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
}
const vis = async (page, sel) => page.locator(sel).first().isVisible().catch(() => false);

const FAKE_SA = JSON.stringify({
  type: 'service_account',
  project_id: 'kyomi-e2e-project',
  private_key_id: 'e2e0000000000000000000000000000000000000',
  private_key: '-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBgkqhkiG9w0BAQEFAASCAUAwggE8AgEAAkEAtESTkeyF0rE2eTest\n-----END PRIVATE KEY-----\n',
  client_email: 'kyomi-bq@kyomi-e2e-project.iam.gserviceaccount.com',
  client_id: '100000000000000000000',
  token_uri: 'https://oauth2.googleapis.com/token',
});

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
  const consoleErrors = [];
  page.on('console', m => { if (m.type() === 'error') consoleErrors.push(m.text()); });
  const failedReqs = [];
  page.on('response', r => { if (r.status() >= 400) failedReqs.push(`${r.status()} ${r.url()} @ ${page.url()}`); });
  page.on('pageerror', e => consoleErrors.push('PAGEERROR: ' + e.message));

  try {
    await page.goto(`${BASE}/login`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.fill('input[type="email"]', ADMIN_EMAIL, { timeout: 10000 });
    await page.fill('input[type="password"]', ADMIN_PASSWORD, { timeout: 10000 });
    await page.click('button[type="submit"]', { timeout: 10000 });
    await page.waitForURL(u => !u.toString().includes('/login'), { timeout: 20000 });
    check('login as workspace admin', true);

    await page.goto(`${BASE}/settings/datasources`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.waitForTimeout(3000);

    await page.locator('button:has-text("Add Datasource")').first().click({ timeout: 10000 });
    await page.waitForTimeout(1500);
    check('create modal opens', await vis(page, 'text=Connection Method'));

    // Name is required by can_next — fill it or every Next assertion is vacuous.
    const nameInput = page.locator('input[placeholder="Production Database"]').first();
    await nameInput.fill('E2E BigQuery', { timeout: 10000 });
    await page.waitForTimeout(500);
    check('name field filled', (await nameInput.inputValue()) === 'E2E BigQuery');

    // ══ A — Kyomi OAuth: the customer's first reported symptom ════════════════════════
    await pickAuthMode(page, 'Kyomi');
    await page.screenshot({ path: `${SHOT}-A-kyomi-oauth.png`, fullPage: true });

    check('KYO-404 ★ "Connect BigQuery" button is VISIBLE in create mode',
      await vis(page, 'button:has-text("Connect BigQuery")'));
    // KYO-705 removed the KYO-408/KYO-499 Google-OAuth-allowlist
    // attestation notice and confirmation checkbox entirely — Kyomi's
    // Google OAuth app left Testing publishing status, so there is
    // nothing left to attest to, and no checkbox renders in this section
    // any more. No selector-based check replaces the three removed here:
    // this whole spec hasn't been run since KYO-602 (see the file header),
    // so a hand-picked selector for "the notice is gone" would be no more
    // trustworthy than the ones it replaces. Absence is left for the
    // KYO-604 reconciliation pass / the next batch QA sweep to confirm in
    // a real browser.
    check('no checkbox renders in the kyomi_oauth Connection tab',
      !(await vis(page, '[role="checkbox"]')));

    // Correct create-mode gate: Next needs a proven OAuth connection.
    const nextBtn = () => page.locator('button:has-text("Next")').last();
    let d = await nextBtn().isDisabled().catch(() => null);
    check('KYO-404 Next is disabled for kyomi_oauth with no Google connection',
      d === true, `disabled=${d}`);

    // ══ B — Service Account: the customer's unblocking path ════════════════════
    await pickAuthMode(page, 'Service Account');
    await page.screenshot({ path: `${SHOT}-B1-sa-empty.png`, fullPage: true });

    check('service-account JSON field is visible', await vis(page, 'textarea'));
    check('"Validate & Discover Projects" correctly hidden before JSON supplied',
      !(await vis(page, 'button:has-text("Validate & Discover Projects")')));

    await page.locator('textarea').first().fill(FAKE_SA, { timeout: 10000 });
    await page.waitForTimeout(1500);
    await page.screenshot({ path: `${SHOT}-B2-sa-filled.png`, fullPage: true });

    check('service-account email is shown',
      await vis(page, 'text=kyomi-bq@kyomi-e2e-project.iam.gserviceaccount.com'));

    // ★ THE reported defect — this control never appeared for the customer.
    check('KYO-405 ★ "Validate & Discover Projects" IS VISIBLE',
      await vis(page, 'button:has-text("Validate & Discover Projects")'));
    check('KYO-405 ★ "Billing Project" field is visible',
      await vis(page, 'text=Billing Project'));
    // KYO-415 removed the "Default Project" field; assertion deleted (KYO-602).

    // Free-text fallback: the customer must be able to type a project id by hand,
    // because their IAM cannot list projects.
    const billing = page.locator('input[placeholder="my-gcp-project"]').first();
    if (await billing.isVisible().catch(() => false)) {
      await billing.fill('kyomi-e2e-project', { timeout: 8000 });
      check('KYO-405 ★ Billing Project accepts a manually typed project id',
        (await billing.inputValue()) === 'kyomi-e2e-project');
    } else {
      check('KYO-405 ★ Billing Project accepts a manually typed project id', false,
        'free-text input not visible');
    }

    // ══ KYO-413 — tearing down credentials must re-close the gate ═══════════
    const removeBtn = page.locator('button:has-text("Remove")').first();
    if (await removeBtn.isVisible().catch(() => false)) {
      await removeBtn.click({ timeout: 10000 });
      await page.waitForTimeout(1200);
      await page.screenshot({ path: `${SHOT}-B3-after-remove.png`, fullPage: true });
      check('KYO-413 ★ Remove hides "Validate & Discover Projects" again',
        !(await vis(page, 'button:has-text("Validate & Discover Projects")')));
      d = await nextBtn().isDisabled().catch(() => null);
      check('KYO-413 ★ Next is re-disabled after credential teardown',
        d === true, `disabled=${d}`);
    } else {
      check('KYO-413 Remove control visible', false, 'Remove button not visible');
    }

    // ══ C — Enterprise OAuth: KYO-404 create-mode exception ═════════════════
    await pickAuthMode(page, 'Enterprise');
    await page.screenshot({ path: `${SHOT}-C-enterprise.png`, fullPage: true });
    d = await nextBtn().isDisabled().catch(() => null);
    check('KYO-404 ★ Next is ENABLED for enterprise_oauth in create mode',
      d === false, `disabled=${d}`);

    // ══ D — KYO-417 feedback context gating (last: modals stack) ════════════
    await pickAuthMode(page, 'Kyomi');
    await page.locator('button:has-text("Request access")').first().click({ timeout: 10000 });
    await page.waitForTimeout(1800);
    await page.screenshot({ path: `${SHOT}-D1-request-access.png`, fullPage: true });
    check('KYO-417 ★ "Request access" opens the feedback modal',
      await vis(page, 'text=Send Feedback'));
    check('KYO-417 ★ "Request BigQuery Access" type is revealed in this context',
      await vis(page, 'text=Request BigQuery Access'));

    check('no hydration panics / console errors', consoleErrors.length === 0,
      consoleErrors.slice(0, 3).join(' | '));

  } catch (e) {
    check('script completed without throwing', false, e.message.split('\n')[0]);
    await page.screenshot({ path: `${SHOT}-ERROR.png`, fullPage: true }).catch(() => {});
  } finally {
    console.log('\n--- HTTP >=400 ---');
    failedReqs.forEach(f => console.log('  ' + f));
    const failed = results.filter(r => !r.pass);
    console.log(`\n===== ${results.length - failed.length}/${results.length} passed =====`);
    if (failed.length) { console.log('FAILURES:'); failed.forEach(f => console.log(`  - ${f.name}  ${f.detail}`)); }
    await browser.close();
    process.exit(failed.length ? 1 : 0);
  }
})();
