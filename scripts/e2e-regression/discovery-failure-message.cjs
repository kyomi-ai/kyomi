/**
 * KYO-470 — verify KYO-469's fix actually renders on screen.
 * Covers KYO-469 (PR #388): `ConnectionTestResultBadge`
 * (crates/kyomi-ui/src/pages/settings/datasources.rs:5988-6016) must render
 * the server's actual failure reason (`TestConnectionResult::message`)
 * beneath the "Failed" row, at BOTH call sites that share the component —
 * not the hardcoded, useless word "Failed" both sites rendered before the
 * fix, no matter which of several distinct causes produced the failure.
 *
 * Call sites covered:
 *   A. Generic "Test & Discover" (datasources.rs:4995, success_label="Connected")
 *      — exercised here via a real Postgres container with a wrong password.
 *   B. BigQuery "Validate & Discover Projects" (datasources.rs:6755,
 *      success_label="Valid") — exercised here via a syntactically-valid but
 *      fake service-account JSON that cannot possibly validate.
 *
 * Assertions use isVisible()/isEnabled(), never count(), with ONE deliberate
 * exception: the "renders exactly once" check for the failure-reason <p> is a
 * cardinality question, not a visibility one, so it uses count() — paired
 * with an isVisible() assertion on the very same locator, per
 * docs/standards/testing/assert-visibility-not-dom-presence.md.
 *
 * COLLISION WARNING (see ground truth in the ticket): a document-wide
 * `text=Failed` also matches the unrelated datasources LIST page error state
 * (`<p class="text-error-foreground">Failed to load datasources: {e}</p>` at
 * datasources.rs:623). Every "Failed" assertion here is scoped to the
 * specific badge row (`div.flex.items-center.gap-3` containing the row's own
 * action button), identified via Playwright's `has:` locator filter rather
 * than fragile parent-class-chain selectors, so it cannot match that banner.
 *
 * There are no `data-testid`/`id` attributes anywhere in this component or
 * its call sites — selectors are class/structure/text based throughout, as
 * is every other spec in this directory.
 *
 * STATUS: written but NOT YET RUN. The server binary and WASM bundle were
 * still being rebuilt in this worktree at write time (a build was running
 * concurrently and would have contended for the cargo lock), so this spec
 * has not been executed even once. Treat it as unverified until it is run.
 */
const { chromium } = require('playwright');

// Overrides (all optional — defaults target local dev and the
// docker-compose kyomi-postgres-test container):
//   E2E_BASE_URL        - app base URL            (default http://localhost:3570)
//   E2E_ADMIN_EMAIL     - admin login email       (default e2e-admin@kyomi.dev)
//   E2E_ADMIN_PASSWORD  - admin login password    (default E2eAdminPass123!)
//   E2E_PG_HOST         - Postgres host           (default 127.0.0.1)
//   E2E_PG_PORT         - Postgres port           (default 5434)
//   E2E_PG_DATABASE     - Postgres database name  (default test_db)
//   E2E_PG_USER         - Postgres username       (default test_user)
//   E2E_PG_PASSWORD     - Postgres CORRECT password (default test_password)
const BASE = process.env.E2E_BASE_URL || 'http://localhost:3570';
const ADMIN_EMAIL = process.env.E2E_ADMIN_EMAIL || 'e2e-admin@kyomi.dev';
const ADMIN_PASSWORD = process.env.E2E_ADMIN_PASSWORD || 'E2eAdminPass123!';
const PG_HOST = process.env.E2E_PG_HOST || '127.0.0.1';
const PG_PORT = process.env.E2E_PG_PORT || '5434';
const PG_DATABASE = process.env.E2E_PG_DATABASE || 'test_db';
const PG_USER = process.env.E2E_PG_USER || 'test_user';
const PG_PASSWORD = process.env.E2E_PG_PASSWORD || 'test_password';
// Deliberately wrong — a real credential rejection from the same reachable
// container, not an unroutable host (which costs a 30s timeout and yields a
// timeout message rather than a credentials message; see ticket ground truth).
const PG_WRONG_PASSWORD = 'kyo470-deliberately-wrong-password';
const SHOT = '/tmp/kyo470';
const results = [];
// Captured reason strings, printed clearly labelled at the end so they can
// be quoted directly in the verification report.
const reasons = {};

function check(name, pass, detail) {
  results.push({ name, pass: !!pass, detail: detail || '' });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
}
const vis = async (loc) => loc.isVisible().catch(() => false);

const FAKE_SA = JSON.stringify({
  type: 'service_account',
  project_id: 'kyomi-e2e-project',
  private_key_id: 'e2e0000000000000000000000000000000000000',
  private_key: '-----BEGIN PRIVATE KEY-----\nMIIBVgIBADANBgkqhkiG9w0BAQEFAASCAUAwggE8AgEAAkEAtESTkeyF0rE2eTest\n-----END PRIVATE KEY-----\n',
  client_email: 'kyomi-bq@kyomi-e2e-project.iam.gserviceaccount.com',
  client_id: '100000000000000000000',
  token_uri: 'https://oauth2.googleapis.com/token',
});

/** Open a <Select> by its exact visible <label> text, choose an option by visible text. */
async function selectByLabel(page, labelText, optionText) {
  const labels = page.locator(`label:text-is("${labelText}")`);
  const n = await labels.count();
  for (let i = 0; i < n; i++) {
    const label = labels.nth(i);
    if (!(await vis(label))) continue;
    const trigger = label.locator('xpath=following-sibling::*[1]').locator('button[aria-haspopup="listbox"]');
    if (await vis(trigger)) {
      await trigger.click({ timeout: 10000 });
      await page.waitForTimeout(200);
      await page.locator('[role="option"]', { hasText: optionText }).first().click({ timeout: 10000 });
      await page.waitForTimeout(500);
      return true;
    }
  }
  return false;
}

/** Open the "Authentication Mode" <Select> and choose an option by visible text. */
async function pickAuthMode(page, label) {
  const trigger = page.locator('label:has-text("Authentication Mode")')
    .locator('xpath=following-sibling::*[1]')
    .locator('button[aria-haspopup="listbox"]');
  await trigger.click({ timeout: 10000 });
  await page.locator('[role="option"]', { hasText: label }).first().click({ timeout: 10000 });
  await page.waitForTimeout(800);
}

// The single globally-unique failure-reason paragraph selector
// (crates/kyomi-ui/src/pages/settings/datasources.rs:6009 — one hit in
// crates/kyomi-ui/src/). Only one of the two ConnectionTestResultBadge call
// sites is ever mounted at a time (each is gated behind its own <Show>, which
// in Leptos does not render the false branch's content at all), so a
// document-wide selector for this exact class combination cannot cross-match
// the other call site.
const reasonSelector = 'p.text-xs.text-error-foreground.mt-2';

/**
 * Poll (up to 70 x 1s) a badge row for either its success text or "Failed" —
 * a real network round-trip has variable latency, so this never uses a fixed
 * sleep. DATASOURCE_TIMEOUT_CONNECT is 30s per attempt and Test & Discover /
 * Validate can make up to two such attempts, hence the 70s ceiling (mirrors
 * scripts/e2e-regression/datasource-create-password.cjs:230).
 */
async function pollBadge(page, row, successText) {
  let succeeded = false;
  let failed = false;
  for (let i = 0; i < 70; i++) {
    succeeded = await vis(row.locator(`text=${successText}`).first());
    failed = await vis(row.locator('text=Failed').first());
    if (succeeded || failed) break;
    await page.waitForTimeout(1000);
  }
  return { succeeded, failed };
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const page = await ctx.newPage();
  // "Failed to load resource" lines are Chromium's own generic echo of any
  // non-2xx HTTP response — already captured with full detail in failedReqs
  // below. Only genuine console.error text and real JS exceptions belong here.
  const consoleErrors = [];
  page.on('console', m => {
    if (m.type() === 'error' && !/^Failed to load resource:/.test(m.text())) {
      consoleErrors.push(m.text());
    }
  });
  const failedReqs = [];
  page.on('response', r => { if (r.status() >= 400) failedReqs.push(`${r.status()} ${r.url()} @ ${page.url()}`); });
  page.on('pageerror', e => consoleErrors.push('PAGEERROR: ' + e.message));

  try {
    // ── Login as ADMIN — both call sites are is_admin-gated ─────────────
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
    check('create modal opens', await vis(page.locator('text=Connection Method')));

    // Name is required by can_next — fill it so the modal is in a normal state.
    const nameInput = page.locator('input[placeholder="Production Database"]').first();
    await nameInput.fill('E2E KYO-470', { timeout: 10000 });
    check('name field filled', (await nameInput.inputValue()) === 'E2E KYO-470');
    await page.screenshot({ path: `${SHOT}-1-modal-open.png`, fullPage: true });

    // Row locators identify the specific badge row by the button it contains
    // — not by parent Tailwind class chains, which are not unique identifiers
    // — and match the button in either its idle or pending label so the
    // locator stays valid across the async Test/Validate call.
    const bqRow = page.locator('div.flex.items-center.gap-3', {
      has: page.locator('button', { hasText: /Validate & Discover Projects|Validating/ }),
    }).first();
    const genericRow = page.locator('div.flex.items-center.gap-3', {
      has: page.locator('button', { hasText: /^Test & Discover$|Discovering/ }),
    }).first();

    // ══ A — BigQuery service_account: fake key that cannot validate ═══════
    // "Set Type FIRST, then auth mode, then fields" — each of the first two
    // resets test_result, so doing it in this order (rather than filling
    // fields first) avoids clobbering an assertion we haven't made yet.
    const pickedBigQuery = await selectByLabel(page, 'Type', 'BigQuery');
    check('Type selector switches to BigQuery', pickedBigQuery);
    await page.waitForTimeout(500);

    await pickAuthMode(page, 'Service Account');
    check('service-account JSON field is visible', await vis(page.locator('textarea').first()));

    await page.locator('textarea').first().fill(FAKE_SA, { timeout: 10000 });
    await page.waitForTimeout(1500);
    check('service-account email is parsed and shown',
      await vis(page.locator('text=kyomi-bq@kyomi-e2e-project.iam.gserviceaccount.com')));
    await page.screenshot({ path: `${SHOT}-2-bq-sa-filled.png`, fullPage: true });

    const validateBtn = bqRow.locator('button', { hasText: /^Validate & Discover Projects$/ });
    check('"Validate & Discover Projects" is visible', await vis(validateBtn));
    await validateBtn.click({ timeout: 10000 });

    const bqOutcome = await pollBadge(page, bqRow, 'Valid');
    await page.screenshot({ path: `${SHOT}-3-bq-failed.png`, fullPage: true });
    check('KYO-469 ★ BigQuery service_account: badge shows "Failed"',
      bqOutcome.failed, `succeeded=${bqOutcome.succeeded} failed=${bqOutcome.failed}`);

    const bqReasonLoc = page.locator(reasonSelector).first();
    const bqReasonVisible = await vis(bqReasonLoc);
    const bqReasonText = bqReasonVisible ? (await bqReasonLoc.textContent() || '').trim() : '';
    reasons.bigqueryServiceAccount = bqReasonText;
    console.log(`\nKYO-469 BigQuery service_account failure reason: "${bqReasonText}"`);
    check('KYO-469 ★ BigQuery failure reason <p> is visible', bqReasonVisible);
    check('KYO-469 ★ BigQuery failure reason is non-empty and is not the literal word "Failed"',
      bqReasonText.length > 0 && bqReasonText !== 'Failed', bqReasonText);
    check('KYO-470 BigQuery failure reason carries no "internal:" tag',
      !bqReasonText.includes('internal:'), bqReasonText);

    // ══ B — Generic Postgres: real container, wrong password ═════════════
    const pickedPostgres = await selectByLabel(page, 'Type', 'PostgreSQL');
    check('Type selector switches to PostgreSQL', pickedPostgres);
    await page.waitForTimeout(500);

    const sslSet = await selectByLabel(page, 'SSL Mode', 'Disable');
    check('SSL Mode set to Disable', sslSet);

    const hostInput = page.locator('input[placeholder="db.example.com"]').first();
    await hostInput.fill(PG_HOST, { timeout: 10000 });
    const portInput = page.locator('input[type="number"][placeholder="5432"]').first();
    await portInput.fill(PG_PORT, { timeout: 10000 });
    const dbInput = page.locator('input[placeholder="mydb"]').first();
    await dbInput.fill(PG_DATABASE, { timeout: 10000 });
    const userInput = page.locator('input[placeholder="Database username"]').first();
    await userInput.fill(PG_USER, { timeout: 10000 });
    const passInput = page.locator('input[type="password"]').first();
    await passInput.fill(PG_WRONG_PASSWORD, { timeout: 10000 });
    check('host/port/database/username/wrong-password filled',
      (await hostInput.inputValue()) === PG_HOST
      && (await portInput.inputValue()) === PG_PORT
      && (await dbInput.inputValue()) === PG_DATABASE
      && (await userInput.inputValue()) === PG_USER
      && (await passInput.inputValue()) === PG_WRONG_PASSWORD);
    await page.screenshot({ path: `${SHOT}-4-pg-wrong-password-filled.png`, fullPage: true });

    const testBtn = genericRow.locator('button', { hasText: /^Test & Discover$/ });
    check('"Test & Discover" button is visible', await vis(testBtn));
    await testBtn.click({ timeout: 10000 });

    const pgFailOutcome = await pollBadge(page, genericRow, 'Connected');
    await page.screenshot({ path: `${SHOT}-5-pg-failed.png`, fullPage: true });
    check('KYO-469 ★ Postgres wrong-password: badge shows "Failed"',
      pgFailOutcome.failed, `succeeded=${pgFailOutcome.succeeded} failed=${pgFailOutcome.failed}`);

    const pgFailReasonLoc = page.locator(reasonSelector).first();
    const pgFailReasonVisible = await vis(pgFailReasonLoc);
    const pgFailReasonText = pgFailReasonVisible ? (await pgFailReasonLoc.textContent() || '').trim() : '';
    reasons.postgresWrongPassword = pgFailReasonText;
    console.log(`\nKYO-469 Postgres wrong-password failure reason: "${pgFailReasonText}"`);
    check('KYO-469 ★ Postgres failure reason <p> is visible', pgFailReasonVisible);
    check('KYO-469 ★ Postgres failure reason is non-empty and is not the literal word "Failed"',
      pgFailReasonText.length > 0 && pgFailReasonText !== 'Failed', pgFailReasonText);
    check('KYO-470 Postgres failure reason matches an expected server message (contains, not equality)',
      /check your credentials|Failed to connect:/.test(pgFailReasonText), pgFailReasonText);
    check('KYO-470 Postgres failure reason carries no "internal:" tag',
      !pgFailReasonText.includes('internal:'), pgFailReasonText);

    // Deliberate count() use — the acceptance criterion here is cardinality
    // ("exactly once"), not visibility, per
    // docs/standards/testing/assert-visibility-not-dom-presence.md. Paired
    // with the isVisible() assertion above on the same locator so a
    // hidden-but-duplicated node cannot slip past either check.
    const pgFailReasonCount = await page.locator(reasonSelector).count();
    check('KYO-470 ★ Postgres failure reason renders EXACTLY ONCE',
      pgFailReasonVisible && pgFailReasonCount === 1,
      `visible=${pgFailReasonVisible} count=${pgFailReasonCount}`);

    // ══ C — Generic Postgres: same container, correct password ═══════════
    await passInput.fill(PG_PASSWORD, { timeout: 10000 });
    check('password corrected', (await passInput.inputValue()) === PG_PASSWORD);
    await testBtn.click({ timeout: 10000 });

    const pgOkOutcome = await pollBadge(page, genericRow, 'Connected');
    await page.screenshot({ path: `${SHOT}-6-pg-success.png`, fullPage: true });
    check('KYO-470 Postgres correct password: badge shows "Connected"',
      pgOkOutcome.succeeded, `succeeded=${pgOkOutcome.succeeded} failed=${pgOkOutcome.failed}`);
    check('KYO-470 Postgres success: no failure-reason <p> is present',
      !(await vis(page.locator(reasonSelector).first())));

    check('no hydration panics / console errors', consoleErrors.length === 0,
      consoleErrors.slice(0, 3).join(' | '));

  } catch (e) {
    check('script completed without throwing', false, e.message.split('\n')[0]);
    await page.screenshot({ path: `${SHOT}-ERROR.png`, fullPage: true }).catch(() => {});
  } finally {
    console.log('\n===== KYO-470 captured failure reasons =====');
    console.log(`BigQuery service_account: "${reasons.bigqueryServiceAccount ?? '(not captured)'}"`);
    console.log(`Postgres wrong password:  "${reasons.postgresWrongPassword ?? '(not captured)'}"`);
    console.log('==============================================\n');

    console.log('--- HTTP >=400 ---');
    failedReqs.forEach(f => console.log('  ' + f));
    const failed = results.filter(r => !r.pass);
    console.log(`\n===== ${results.length - failed.length}/${results.length} passed =====`);
    if (failed.length) { console.log('FAILURES:'); failed.forEach(f => console.log(`  - ${f.name}  ${f.detail}`)); }
    await browser.close();
    process.exit(failed.length ? 1 : 0);
  }
})();
