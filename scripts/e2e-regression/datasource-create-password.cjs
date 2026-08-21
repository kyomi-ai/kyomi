/**
 * KYO-424 — Postgres create-mode E2E, full end-to-end against a real container.
 *
 * Covers the password-provider create flow end to end:
 *   1. Log in, open Settings > Datasources > Add Datasource.
 *   2. Pick Postgres, fill in real connection config for the
 *      `kyomi-postgres-test` container (127.0.0.1:5434/test_db).
 *   3. Run Test & Discover and assert it succeeds.
 *   4. Assert the Next button transitions disabled -> enabled (not just that
 *      it ends up enabled — the transition is the thing being proven).
 *   5. Complete creation.
 *   6. Assert the created datasource is actually queryable: run a real SQL
 *      query against it from the SQL Editor and confirm a server-round-trip
 *      value comes back. "Queryable" here concretely means: the SQL Editor's
 *      datasource selector can select it, and `SELECT <unique-marker> AS
 *      e2e_marker` executed against it (Ctrl+Enter, same pipeline commit
 *      history's own test-arrow-pipeline.cjs uses) returns that exact marker
 *      in the results — i.e. the full stack (credential resolution, pooled
 *      connection, real query execution, Arrow decode, results render) is
 *      exercised, not just "a row exists in the datasource list".
 *   7. Delete the datasource so the spec is re-runnable without polluting
 *      the dev database. Cleanup failure is reported loudly (non-zero exit)
 *      rather than left as silent garbage.
 *
 * Assertions use isVisible()/isEnabled(), never count(): count() matches
 * hidden DOM and would pass on a control the user cannot actually see.
 *
 * KNOWN BLOCKED STATE — KYO-428 (confirmed app bug, not a test defect):
 * create-mode Test & Discover silently drops non-default ports for
 * postgres/mysql/redshift/clickhouse (a serde_qs string/number coercion
 * bug). This spec targets the test container on port 5434 — nothing listens
 * on the default 5432, so the bug cannot be sidestepped. Every run currently
 * ends at step 3 with an unmissable "BLOCKED BY KYO-428" banner and a
 * non-zero exit; steps 4-7 (create + queryability + cleanup) are skipped
 * deterministically rather than attempted against a connection that never
 * succeeded. This spec goes green only once KYO-428 is actually fixed.
 */
const { chromium } = require('playwright');

const BASE = 'http://localhost:3000';
const SHOT = '/tmp/ds-create-pw';
const results = [];
let cleanupFailed = false;

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

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const page = await ctx.newPage();
  // "Failed to load resource" lines are Chromium's own generic echo of any
  // non-2xx HTTP response — already captured with full detail in failedReqs
  // below. Keeping them here too would make this check redundant with (and
  // strictly noisier than) that array, and would flag expected pre-auth 401s
  // (before the login cookie is set) as if they were real JS/WASM errors.
  // Only genuine console.error text and real JS exceptions belong here.
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
      // KYO-426 (known, out of scope): create-mode fires a datasource-scoped
      // OAuth status fetch for a datasource that doesn't exist yet -> 500.
      // Not applicable to this postgres-only spec, but excluded defensively
      // in case some other code path triggers it incidentally.
      if (url.includes('/oauth/') && url.includes('status')) return;
      failedReqs.push(`${r.status()} ${url}`);
    }
  });
  page.on('pageerror', e => consoleErrors.push('PAGEERROR: ' + e.message));

  const dsName = `E2E Postgres ${Date.now()}`;
  const marker = String(Date.now()).slice(-9) + '42'; // run-unique, not a "1" collision risk
  let createdSuccessfully = false;

  try {
    // ── Login ──────────────────────────────────────────────────────────
    await page.goto(`${BASE}/login`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.fill('input[type="email"]', 'e2e-admin@kyomi.dev', { timeout: 10000 });
    await page.fill('input[type="password"]', 'E2eAdminPass123!', { timeout: 10000 });
    await page.click('button[type="submit"]', { timeout: 10000 });
    await page.waitForURL(u => !u.toString().includes('/login'), { timeout: 20000 });
    check('login as admin', true);

    // ── Open create modal ─────────────────────────────────────────────
    await page.goto(`${BASE}/settings/datasources`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.waitForTimeout(3000);

    await page.locator('button:has-text("Add Datasource")').first().click({ timeout: 10000 });
    await page.waitForTimeout(1500);
    check('create modal opens', await vis(page.locator('text=Connection Method')));

    // ── Pick Postgres ──────────────────────────────────────────────────
    const pickedType = await selectByLabel(page, 'Type', 'PostgreSQL');
    check('Type selector switches to PostgreSQL', pickedType);
    await page.waitForTimeout(500);

    // ── Name ───────────────────────────────────────────────────────────
    const nameInput = page.locator('input[placeholder="Production Database"]').first();
    await nameInput.fill(dsName, { timeout: 10000 });
    check('name field filled', (await nameInput.inputValue()) === dsName);

    // ── SSL mode: the test container has no TLS listener, "require" (the
    //    field's default) would fail to connect ──────────────────────────
    const sslSet = await selectByLabel(page, 'SSL Mode', 'Disable');
    check('SSL Mode set to Disable', sslSet);

    // ── Connection config ─────────────────────────────────────────────
    const hostInput = page.locator('input[placeholder="db.example.com"]').first();
    await hostInput.fill('127.0.0.1', { timeout: 10000 });
    const portInput = page.locator('input[type="number"][placeholder="5432"]').first();
    await portInput.fill('5434', { timeout: 10000 });
    const dbInput = page.locator('input[placeholder="mydb"]').first();
    await dbInput.fill('test_db', { timeout: 10000 });
    check('host/port/database filled',
      (await hostInput.inputValue()) === '127.0.0.1'
      && (await portInput.inputValue()) === '5434'
      && (await dbInput.inputValue()) === 'test_db');

    // ── Credentials ────────────────────────────────────────────────────
    const userInput = page.locator('input[placeholder="Database username"]').first();
    await userInput.fill('test_user', { timeout: 10000 });
    const passInput = page.locator('input[type="password"]').first();
    await passInput.fill('test_password', { timeout: 10000 });
    check('username/password filled',
      (await userInput.inputValue()) === 'test_user'
      && (await passInput.inputValue()) === 'test_password');

    await page.screenshot({ path: `${SHOT}-1-filled.png`, fullPage: true });

    // ── Next must be disabled before a successful Test & Discover ───────
    const nextBtn = () => page.locator('button:has-text("Next")').last();
    let nextEnabledBefore = await nextBtn().isEnabled().catch(() => null);
    check('Next is DISABLED before Test & Discover succeeds',
      nextEnabledBefore === false, `enabled=${nextEnabledBefore}`);

    // ── Test & Discover against the real container ───────────────────────
    const testBtn = page.locator('button:has-text("Test & Discover")').first();
    check('Test & Discover button is visible', await vis(testBtn));
    await testBtn.click({ timeout: 10000 });

    const connectedLabel = page.locator('text=Connected').first();
    const failedLabel = page.locator('text=Failed').first();
    // Poll rather than a fixed sleep — a real network round-trip to the
    // container has variable latency.
    let testSucceeded = false;
    let testFailed = false;
    // DATASOURCE_TIMEOUT_CONNECT is 30s per attempt, and this call makes up
    // to two such attempts (create_provider, then test_connection) — allow
    // up to 70s before giving up.
    for (let i = 0; i < 70; i++) {
      testSucceeded = await vis(connectedLabel);
      testFailed = await vis(failedLabel);
      if (testSucceeded || testFailed) break;
      await page.waitForTimeout(1000);
    }
    await page.screenshot({ path: `${SHOT}-2-tested.png`, fullPage: true });
    check('Test & Discover succeeds against real Postgres container',
      testSucceeded, testSucceeded ? '' : `Failed label visible=${testFailed}`);

    // ── Next transitions to ENABLED after the successful test ───────────
    let nextEnabledAfter = await nextBtn().isEnabled().catch(() => null);
    check('Next transitions to ENABLED after Test & Discover succeeds',
      nextEnabledAfter === true, `enabled=${nextEnabledAfter}`);

    const canProceedToCreate = testSucceeded && nextEnabledAfter === true;

    if (!canProceedToCreate) {
      // Deterministic, honest stop: do not attempt Create against a
      // connection that never tested successfully (that would risk a
      // false-positive datasource row), and do not throw a generic error
      // that would bury the real cause in a stack-trace-shaped FAIL line.
      blockedBanner('KYO-428',
        'create-mode Test & Discover silently drops the non-default port (5434) for ' +
        'postgres/mysql/redshift/clickhouse — a serde_qs string/number coercion bug. ' +
        'Nothing listens on the default 5432 in this environment, so it cannot be ' +
        'sidestepped. Aborting before Create; steps 4-7 (create, queryability, ' +
        'cleanup) are skipped, not attempted.');
      check('create + queryability steps reached', false,
        'skipped — BLOCKED BY KYO-428, Test & Discover must succeed first');
    } else {
      // ── Next -> Catalog tab -> Create (leave scope default: index everything) ──
      await nextBtn().click({ timeout: 10000 });
      await page.waitForTimeout(800);
      check('Catalog tab reached', await vis(page.locator('text=Catalog Scope')));

      const createBtn = page.locator('button:has-text("Create")').last();
      check('Create button visible on Catalog tab', await vis(createBtn));
      await createBtn.click({ timeout: 10000 });
      await page.waitForTimeout(3000);
      await page.screenshot({ path: `${SHOT}-3-created.png`, fullPage: true });

      const modalClosed = !(await vis(page.locator('text=Connection Method')));
      check('modal closes after Create', modalClosed);

      const rowVisible = await vis(page.locator(`text=${dsName}`).first());
      check('created datasource appears in the list', rowVisible);
      createdSuccessfully = modalClosed && rowVisible;

      if (createdSuccessfully) {
        // ── Queryable: run a real query against it from the SQL Editor ──────
        await page.goto(`${BASE}/sql-editor`, { waitUntil: 'networkidle', timeout: 30000 });
        await page.waitForTimeout(5000);

        // Datasource selector — the <Select> in the header, scoped by its
        // known fixed-width wrapper (no visible <label> next to it).
        const dsSelectorWrapper = page.locator('div[class*="w-[140px]"]').first();
        const dsSelectorTrigger = dsSelectorWrapper.locator('button[aria-haspopup="listbox"]');
        check('SQL Editor datasource selector visible', await vis(dsSelectorTrigger));
        await dsSelectorTrigger.click({ timeout: 10000 });
        await page.waitForTimeout(300);
        const dsOption = page.locator('[role="option"]', { hasText: dsName }).first();
        const dsOptionVisible = await vis(dsOption);
        check('created datasource is selectable in SQL Editor', dsOptionVisible);
        if (dsOptionVisible) {
          await dsOption.click({ timeout: 10000 });
          await page.waitForTimeout(1000);
        } else {
          await page.keyboard.press('Escape');
        }

        const kodeEditor = page.locator('.kode-editor');
        check('SQL code editor visible', await vis(kodeEditor));
        await kodeEditor.click({ timeout: 10000 });
        await page.waitForTimeout(200);
        await page.keyboard.press('Control+a');
        await page.keyboard.press('Backspace');
        await page.waitForTimeout(200);
        await page.keyboard.type(`SELECT ${marker} AS e2e_marker`, { delay: 8 });
        await page.waitForTimeout(300);
        await page.keyboard.press('Control+Enter');
        await page.waitForTimeout(6000);
        await page.screenshot({ path: `${SHOT}-4-queried.png`, fullPage: true });

        const bodyText = await page.textContent('body');
        const markerFound = bodyText.includes(marker);
        const hasServerError = /error running server function/i.test(bodyText);
        check('query against the created datasource returns the expected marker value',
          markerFound && !hasServerError,
          `markerFound=${markerFound} hasServerError=${hasServerError}`);
      } else {
        check('queryability check reached', false,
          'skipped — datasource creation did not complete');
      }
    }

    check('no hydration panics / console errors', consoleErrors.length === 0,
      consoleErrors.slice(0, 3).join(' | '));

  } catch (e) {
    check('script completed without throwing', false, e.message.split('\n')[0]);
    await page.screenshot({ path: `${SHOT}-ERROR.png`, fullPage: true }).catch(() => {});
  } finally {
    // ── Cleanup: delete the datasource we created, so re-runs don't pollute ──
    if (createdSuccessfully) {
      try {
        await page.goto(`${BASE}/settings/datasources`, { waitUntil: 'networkidle', timeout: 30000 });
        await page.waitForTimeout(3000);

        const nameSpan = page.locator('span.font-medium.truncate', { hasText: dsName }).first();
        const row = nameSpan.locator('xpath=ancestor::div[contains(@class,"hover:bg-muted")][1]');
        const deleteBtn = row.locator('button[class*="hover:bg-error"]').first();

        if (await vis(deleteBtn)) {
          await deleteBtn.click({ timeout: 10000 });
          await page.waitForTimeout(800);
          const confirmBtn = page.locator('button:has-text("Delete")').last();
          if (await vis(confirmBtn)) {
            await confirmBtn.click({ timeout: 10000 });
            await page.waitForTimeout(2000);
            const stillThere = await vis(page.locator(`text=${dsName}`).first());
            if (stillThere) {
              cleanupFailed = true;
              console.log(`\n*** CLEANUP FAILED *** "${dsName}" still visible in the list after confirming delete.`);
            } else {
              check('cleanup: created datasource deleted', true);
            }
          } else {
            cleanupFailed = true;
            console.log(`\n*** CLEANUP FAILED *** delete confirmation dialog did not appear for "${dsName}".`);
          }
        } else {
          cleanupFailed = true;
          console.log(`\n*** CLEANUP FAILED *** could not locate delete button for "${dsName}" — MANUAL CLEANUP REQUIRED.`);
        }
      } catch (cleanupErr) {
        cleanupFailed = true;
        console.log(`\n*** CLEANUP FAILED *** ${cleanupErr.message} — MANUAL CLEANUP REQUIRED for "${dsName}".`);
        await page.screenshot({ path: `${SHOT}-CLEANUP-ERROR.png`, fullPage: true }).catch(() => {});
      }
    }

    console.log('\n--- HTTP >=400 ---');
    failedReqs.forEach(f => console.log('  ' + f));
    const failed = results.filter(r => !r.pass);
    console.log(`\n===== ${results.length - failed.length}/${results.length} passed =====`);
    if (failed.length) { console.log('FAILURES:'); failed.forEach(f => console.log(`  - ${f.name}  ${f.detail}`)); }
    if (cleanupFailed) { console.log(`*** CLEANUP FAILED for datasource "${dsName}" — remove it manually. ***`); }
    await browser.close();
    process.exit((failed.length || cleanupFailed) ? 1 : 0);
  }
})();
