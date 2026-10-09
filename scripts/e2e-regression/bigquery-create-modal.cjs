/**
 * BigQuery create-modal E2E — a customer's reported defect.
 * Covers the surviving KYO-404, KYO-405 and KYO-413 create-mode controls.
 *
 * Assertions use isVisible(), never count(): count() matches hidden DOM and
 * would pass on a control the user cannot actually see — which is precisely
 * the defect being tested ("the control never appears").
 *
 * Current create-mode contract (KYO-604):
 * - KYO-704 retired kyomi_oauth; Service Account is the default. Its former
 *   Connect BigQuery / Google-connection Next checks no longer apply here.
 * - KYO-705 removed the allowlist notice and attestation checkbox.
 * - KYO-415 removed Default Project; Billing Project remains supported.
 * - KYO-504 removed the Request BigQuery Access feedback type, so the former
 *   section D request-access flow is removed.
 *
 * This spec uses a deliberately invalid key only to exercise local JSON parsing
 * and control visibility. It never validates against Google or saves a datasource.
 * Remove checks credential teardown and the unvalidated Next gate; it does not
 * prove teardown clears a previously successful validation result.
 *
 * Run against a built local dev server with seeded test users. Syntax checks do
 * not establish a passing browser run; screenshots are written to /tmp/bq-e2e-*.
 * The named built-app run and screenshot evidence are tracked in KYO-895.
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

const INVALID_SA = JSON.stringify({
  type: 'service_account',
  project_id: 'kyomi-e2e-project',
  private_key_id: 'e2e0000000000000000000000000000000000000',
  private_key: '-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBgkqhkiG9w0BAQEFAASCAUAwggE8AgEAAkEAtESTkeyF0rE2eTest\n-----END PRIVATE KEY-----\n',
  client_email: 'kyomi-bq@kyomi-e2e-project.iam.gserviceaccount.com',
  client_id: '100000000000000000000',
  token_uri: 'https://oauth2.googleapis.com/token',
});

function authModeTrigger(page) {
  return page.locator('label:has-text("Authentication Mode")')
    .locator('xpath=following-sibling::*[1]')
    .locator('button[aria-haspopup="listbox"]');
}

async function pickAuthMode(page, label) {
  await authModeTrigger(page).click({ timeout: 10000 });
  await page.getByRole('option', { name: label, exact: true }).click({ timeout: 10000 });
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

    // ══ A — Default mode and retired controls ═══════════════════════════════
    const authTrigger = authModeTrigger(page);
    check('KYO-704 Service Account is the default create-mode authentication',
      await authTrigger.isVisible() &&
      (await authTrigger.innerText()).trim() === 'Service Account (Recommended)');
    await authTrigger.click({ timeout: 10000 });
    const serviceAccountOption = page.getByRole('option', {
      name: 'Service Account (Recommended)', exact: true,
    });
    await serviceAccountOption.waitFor({ state: 'visible', timeout: 10000 });
    // Check retirement while the options are open: checking a closed listbox
    // would pass even if the retired option was still offered.
    check('KYO-704 Kyomi OAuth is not offered in create mode',
      !(await vis(page, '[role="option"]:has-text("Google OAuth (Kyomi)")')));
    await serviceAccountOption.click({ timeout: 10000 });
    await page.waitForTimeout(800);
    check('KYO-705 allowlist notice is absent',
      !(await vis(page, 'text=Google account authorization required')));
    check('KYO-705 beta-access attestation is absent',
      !(await vis(page, 'text=I have beta access')));
    check('no attestation checkbox renders in the service-account Connection tab',
      !(await vis(page, '[role="checkbox"]')));

    const nextBtn = () => page.getByRole('button', { name: 'Next', exact: true });
    let d = await nextBtn().isDisabled();
    check('Next is visible and disabled before service-account validation',
      await nextBtn().isVisible() && d === true, `disabled=${d}`);

    // ══ B — Service Account: the customer's unblocking path ════════════════════
    await page.screenshot({ path: `${SHOT}-B1-sa-empty.png`, fullPage: true });

    check('service-account JSON field is visible', await vis(page, 'textarea'));
    check('"Validate & Discover Projects" correctly hidden before JSON supplied',
      !(await vis(page, 'button:has-text("Validate & Discover Projects")')));

    await page.locator('textarea').first().fill(INVALID_SA, { timeout: 10000 });
    await page.waitForTimeout(1500);
    await page.screenshot({ path: `${SHOT}-B2-sa-filled.png`, fullPage: true });

    check('service-account email is shown',
      await vis(page, 'text=kyomi-bq@kyomi-e2e-project.iam.gserviceaccount.com'));

    // ★ THE reported defect — this control never appeared for the customer.
    check('KYO-405 ★ "Validate & Discover Projects" IS VISIBLE',
      await vis(page, 'button:has-text("Validate & Discover Projects")'));
    check('KYO-405 ★ "Billing Project" field is visible',
      await vis(page, 'text=Billing Project'));
    check('KYO-415 Default Project field is absent in service-account mode',
      !(await vis(page, 'text=Default Project')));
    d = await nextBtn().isDisabled();
    check('parsed service-account JSON alone does not enable Next',
      await nextBtn().isVisible() && d === true, `disabled=${d}`);

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

    // ══ KYO-413 — credential teardown hides validation controls ════════════
    const removeBtn = page.locator('button:has-text("Remove")').first();
    if (await removeBtn.isVisible().catch(() => false)) {
      await removeBtn.click({ timeout: 10000 });
      await page.waitForTimeout(1200);
      await page.screenshot({ path: `${SHOT}-B3-after-remove.png`, fullPage: true });
      check('KYO-413 ★ Remove hides "Validate & Discover Projects" again',
        !(await vis(page, 'button:has-text("Validate & Discover Projects")')));
      check('KYO-413 Remove restores the service-account JSON field',
        await vis(page, 'textarea'));
      d = await nextBtn().isDisabled();
      check('KYO-413 Next remains disabled after removing unvalidated credentials',
        await nextBtn().isVisible() && d === true, `disabled=${d}`);
    } else {
      check('KYO-413 Remove control visible', false, 'Remove button not visible');
    }

    // ══ C — Enterprise OAuth: KYO-404 create-mode exception ═════════════════
    await pickAuthMode(page, 'Google OAuth (Enterprise)');
    await page.screenshot({ path: `${SHOT}-C-enterprise.png`, fullPage: true });
    check('enterprise OAuth configuration fields are visible',
      await vis(page, 'input[placeholder="From Google Cloud Console"]') &&
      await vis(page, 'input[placeholder="OAuth client secret"]'));
    check('enterprise OAuth explains connection happens after saving',
      await vis(page, 'text=After saving, connect your BigQuery account from this settings panel.'));
    check('enterprise OAuth connect button is hidden until the datasource is saved',
      !(await vis(page, 'button:has-text("Connect BigQuery")')));
    check('no attestation checkbox renders in the enterprise Connection tab',
      !(await vis(page, '[role="checkbox"]')));
    check('KYO-415 Default Project field is absent in enterprise mode',
      !(await vis(page, 'text=Default Project')));
    d = await nextBtn().isDisabled();
    check('KYO-404 ★ Next is visible and ENABLED for enterprise_oauth in create mode',
      await nextBtn().isVisible() && d === false, `disabled=${d}`);
    await nameInput.fill('');
    d = await nextBtn().isDisabled();
    check('enterprise OAuth still requires a datasource name',
      await nextBtn().isVisible() && d === true, `disabled=${d}`);
    await nameInput.fill('E2E BigQuery');

    // Switching away from the enterprise precreate exception must close Next.
    await pickAuthMode(page, 'Service Account (Recommended)');
    d = await nextBtn().isDisabled();
    check('switching back to an empty service account disables Next',
      await nextBtn().isVisible() && d === true, `disabled=${d}`);
    await page.screenshot({ path: `${SHOT}-C2-back-to-sa.png`, fullPage: true });

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
