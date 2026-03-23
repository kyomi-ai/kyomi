#!/usr/bin/env node
/**
 * E2E Regression Tests — Utility Group (verify2)
 * Flows: utility-02 (unsubscribe), utility-03 (unsubscribe prefilled)
 *
 * Run: NODE_PATH=/home/jason/repos/kyomi/node_modules node /home/jason/repos/kyomi/scripts/e2e-regression/test-utility-verify2.cjs
 */

'use strict';

const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

// ── Config ────────────────────────────────────────────────────────────────────

const config = {
  servers: {
    react: 'http://localhost:8002',
    leptos: 'http://localhost:3000',
  },
  timeouts: {
    navigation: 15000,
    action: 8000,
    wasm_settle: 3000,
  },
  screenshotsDir: '/tmp/e2e-regression/verify2/utility',
};

// ── Helpers ───────────────────────────────────────────────────────────────────

function mkdirp(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

async function launchPage(server) {
  const baseUrl = config.servers[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  return { browser, context, page, baseUrl };
}

async function saveScreenshot(page, flowId, server) {
  const dir = path.join(config.screenshotsDir, flowId);
  mkdirp(dir);
  const filepath = path.join(dir, `${server}.png`);
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
}

function pass(message) {
  return { pass: true, message };
}

function fail(message) {
  return { pass: false, message };
}

// ── Flow: utility-02 — Unsubscribe page ──────────────────────────────────────

async function testUnsubscribe(server) {
  const { browser, page, baseUrl } = await launchPage(server);
  const assertions = [];
  let screenshot = null;

  try {
    await page.goto(`${baseUrl}/unsubscribe`, {
      waitUntil: 'networkidle',
      timeout: config.timeouts.navigation,
    });

    // Allow WASM settle for Leptos
    await page.waitForTimeout(config.timeouts.wasm_settle);
    await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});

    const finalUrl = page.url();

    // Assert 1: Unsubscribe form visible (must NOT redirect to /login)
    const form = await page.$('form');
    assertions.push(
      form && !finalUrl.includes('/login')
        ? pass(`Unsubscribe form visible — final URL: ${finalUrl}`)
        : fail(`Unsubscribe form not visible or redirected — final URL: ${finalUrl}`)
    );

    // Assert 2: Email input visible
    const emailInput = await page.$('input[type="email"], input[name="email"]');
    const emailVisible = emailInput ? await emailInput.isVisible().catch(() => false) : false;
    assertions.push(
      emailVisible
        ? pass('Email input is present and visible')
        : fail('Email input not found on unsubscribe page')
    );

    // Assert 3: "Unsubscribe" button visible
    const unsubscribeBtn = await page.locator('button:has-text("Unsubscribe")').first();
    const unsubscribeBtnVisible = await unsubscribeBtn.isVisible().catch(() => false);
    const submitBtn = await page.$('button[type="submit"]');
    assertions.push(
      unsubscribeBtnVisible || submitBtn
        ? pass('Unsubscribe/submit button is present')
        : fail('No Unsubscribe or submit button found')
    );

    // Assert 4: Logo renders correctly (naturalWidth > 0)
    const logos = await page.$$('img[alt="Kyomi"]');
    let logoOk = false;
    let anyLogo = logos.length > 0;
    for (const logo of logos) {
      const naturalWidth = await logo.evaluate((el) => el.naturalWidth).catch(() => 0);
      if (naturalWidth > 0) {
        logoOk = true;
      }
    }
    if (!anyLogo) {
      assertions.push(fail('No logo img[alt="Kyomi"] found on unsubscribe page'));
    } else {
      assertions.push(
        logoOk
          ? pass(`Logo renders correctly (naturalWidth > 0; ${logos.length} logos found)`)
          : fail(`Logo appears broken — all ${logos.length} logo img(s) have naturalWidth=0`)
      );
    }

    screenshot = await saveScreenshot(page, 'utility-02', server);
  } catch (e) {
    assertions.push(fail(`Exception: ${e.message}`));
  } finally {
    await browser.close();
  }

  return { assertions, screenshot };
}

// ── Flow: utility-03 — Unsubscribe with prefilled email ──────────────────────

async function testUnsubscribePrefilled(server) {
  const { browser, page, baseUrl } = await launchPage(server);
  const assertions = [];
  let screenshot = null;

  try {
    await page.goto(`${baseUrl}/unsubscribe?email=test@example.com`, {
      waitUntil: 'networkidle',
      timeout: config.timeouts.navigation,
    });

    // Allow WASM settle
    await page.waitForTimeout(config.timeouts.wasm_settle);
    await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});

    const finalUrl = page.url();

    // Assert 1: Email input pre-filled with "test@example.com"
    const emailInput = page.locator('input[type="email"], input[name="email"]').first();
    const inputExists = await emailInput.isVisible().catch(() => false);
    if (!inputExists) {
      assertions.push(fail('Email input not found — cannot check prefill'));
    } else {
      const value = await emailInput.inputValue().catch(() => '');
      assertions.push(
        value === 'test@example.com'
          ? pass(`Email input pre-filled correctly: "${value}"`)
          : fail(`Email input not pre-filled — expected "test@example.com", got: "${value}"`)
      );
    }

    // Assert 2: Logo renders correctly (naturalWidth > 0)
    const logos = await page.$$('img[alt="Kyomi"]');
    let logoOk = false;
    let anyLogo = logos.length > 0;
    for (const logo of logos) {
      const naturalWidth = await logo.evaluate((el) => el.naturalWidth).catch(() => 0);
      if (naturalWidth > 0) logoOk = true;
    }
    if (!anyLogo) {
      assertions.push(fail('No logo img[alt="Kyomi"] found on unsubscribe page'));
    } else {
      assertions.push(
        logoOk
          ? pass(`Logo renders correctly (naturalWidth > 0; ${logos.length} logos found)`)
          : fail(`Logo appears broken — all ${logos.length} logo img(s) have naturalWidth=0`)
      );
    }

    screenshot = await saveScreenshot(page, 'utility-03', server);
  } catch (e) {
    assertions.push(fail(`Exception: ${e.message}`));
  } finally {
    await browser.close();
  }

  return { assertions, screenshot };
}

// ── Flow runner ───────────────────────────────────────────────────────────────

const FLOWS = [
  {
    id: 'utility-02',
    name: 'Unsubscribe page loads without auth',
    fn: testUnsubscribe,
  },
  {
    id: 'utility-03',
    name: 'Unsubscribe with prefilled email',
    fn: testUnsubscribePrefilled,
  },
];

function classify(reactResult, leptosResult) {
  const rPass = reactResult.status === 'pass';
  const lPass = leptosResult.status === 'pass';
  if (rPass && lPass) return 'PASS';
  if (rPass && !lPass) return 'REGRESSION';
  if (!rPass && lPass) return 'BEHAVIORAL_DIFF';
  return 'BOTH_FAIL';
}

async function runFlow(flow) {
  console.log(`\nRunning: ${flow.id} — ${flow.name}`);
  const flowResult = { id: flow.id, name: flow.name };

  for (const server of ['react', 'leptos']) {
    console.log(`  [${server}] starting...`);
    try {
      const { assertions, screenshot } = await flow.fn(server);
      const allPass = assertions.every((a) => a.pass);
      flowResult[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter((a) => !a.pass).map((a) => a.message).join('; '),
      };
      if (allPass) {
        console.log(`  [${server}] PASS`);
      } else {
        console.log(`  [${server}] FAIL`);
        assertions.filter((a) => !a.pass).forEach((a) => console.log(`    x ${a.message}`));
      }
    } catch (e) {
      flowResult[server] = {
        status: 'fail',
        assertions: [{ pass: false, message: `Unhandled exception: ${e.message}` }],
        screenshot: null,
        error: e.message,
      };
      console.log(`  [${server}] EXCEPTION: ${e.message}`);
    }
  }

  flowResult.classification = classify(flowResult.react, flowResult.leptos);
  flowResult.notes = '';
  console.log(`  -> ${flowResult.classification}`);
  return flowResult;
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log('E2E Regression — Utility Group (verify2)');
  console.log('=========================================');

  mkdirp(config.screenshotsDir);

  const flows = [];
  for (const flow of FLOWS) {
    const result = await runFlow(flow);
    flows.push(result);
  }

  // Save results
  const resultsPath = path.join(config.screenshotsDir, 'results.json');
  const output = {
    group: 'utility',
    timestamp: new Date().toISOString(),
    flows,
  };
  fs.writeFileSync(resultsPath, JSON.stringify(output, null, 2));

  // Print summary
  console.log('\n=== SUMMARY ===');
  const counts = { PASS: 0, REGRESSION: 0, BEHAVIORAL_DIFF: 0, BOTH_FAIL: 0 };
  flows.forEach((f) => {
    const icon = f.classification === 'PASS' ? 'PASS' : 'FAIL';
    console.log(`[${icon}] [${f.classification}] ${f.name}`);
    counts[f.classification] = (counts[f.classification] || 0) + 1;
  });
  console.log('\nCounts:', JSON.stringify(counts));
  console.log(`\nResults saved to: ${resultsPath}`);
}

main().catch((e) => {
  console.error('Fatal error:', e);
  process.exit(1);
});
