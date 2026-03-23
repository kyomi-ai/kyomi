#!/usr/bin/env node
/**
 * E2E Regression Tests — Utility Group, Phase 1 Only
 * Flows: utility-02 (unsubscribe), utility-03 (unsubscribe prefilled)
 *
 * Run: NODE_PATH=/home/jason/repos/kyomi/node_modules node /home/jason/repos/kyomi/scripts/e2e-regression/test-utility-phase1.cjs
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
    // Leptos WASM needs time to boot + run async effects
    wasm_settle: 3000,
  },
  screenshotsDir: '/tmp/e2e-regression/utility',
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

    // Assert 1: Must NOT redirect to /login (public page)
    assertions.push(
      !finalUrl.includes('/login')
        ? pass(`Page loads without auth redirect — final URL: ${finalUrl}`)
        : fail(`Redirected to /login — unsubscribe should be a public page, got: ${finalUrl}`)
    );

    // Assert 2: Email input visible
    const emailInput = await page.$('input[type="email"], input[name="email"]');
    assertions.push(
      emailInput
        ? pass('Email input is present and visible')
        : fail('Email input not found on unsubscribe page')
    );

    // Assert 3: Submit/Unsubscribe button visible
    const submitBtn = await page.$('button[type="submit"]');
    const unsubscribeBtn = page.locator('button:has-text("Unsubscribe")').first();
    const unsubscribeBtnVisible = await unsubscribeBtn.isVisible().catch(() => false);
    assertions.push(
      submitBtn || unsubscribeBtnVisible
        ? pass('Submit/Unsubscribe button is present')
        : fail('No submit or Unsubscribe button found')
    );

    // Assert 4: Logo renders correctly (not broken image)
    // Check both light and dark logos — at least one should be a visible <img>
    const logos = await page.$$('img[alt="Kyomi"]');
    let logoOk = false;
    let logoBroken = false;
    for (const logo of logos) {
      const naturalWidth = await logo.evaluate((el) => el.naturalWidth).catch(() => 0);
      if (naturalWidth > 0) {
        logoOk = true;
      } else {
        logoBroken = true;
      }
    }
    if (logos.length === 0) {
      assertions.push(fail('No logo <img alt="Kyomi"> found on unsubscribe page'));
    } else if (logoOk) {
      assertions.push(pass(`Logo renders correctly (naturalWidth > 0 on at least one logo; ${logos.length} logos found)`));
    } else if (logoBroken && !logoOk) {
      assertions.push(fail(`Logo appears broken — all ${logos.length} logo img(s) have naturalWidth=0 (broken image placeholder)`));
    }

    screenshot = await saveScreenshot(page, 'utility-02-unsubscribe', server);
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

    // Assert 1: Must NOT redirect to /login
    assertions.push(
      !finalUrl.includes('/login')
        ? pass(`Page loads without auth redirect — final URL: ${finalUrl}`)
        : fail(`Redirected to /login — page should be public, got: ${finalUrl}`)
    );

    // Assert 2: Email input has prefilled value
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

    // Assert 3: Logo renders correctly
    const logos = await page.$$('img[alt="Kyomi"]');
    let logoOk = false;
    for (const logo of logos) {
      const naturalWidth = await logo.evaluate((el) => el.naturalWidth).catch(() => 0);
      if (naturalWidth > 0) logoOk = true;
    }
    if (logos.length === 0) {
      assertions.push(fail('No logo <img alt="Kyomi"> found on unsubscribe page'));
    } else {
      assertions.push(
        logoOk
          ? pass(`Logo renders correctly (naturalWidth > 0; ${logos.length} logos found)`)
          : fail(`Logo appears broken — all ${logos.length} logo img(s) have naturalWidth=0`)
      );
    }

    screenshot = await saveScreenshot(page, 'utility-03-unsubscribe-prefilled', server);
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
        assertions.filter((a) => !a.pass).forEach((a) => console.log(`    ✗ ${a.message}`));
      }
    } catch (e) {
      flowResult[server] = {
        status: 'fail',
        assertions: [fail(`Unhandled exception: ${e.message}`)],
        screenshot: null,
        error: e.message,
      };
      console.log(`  [${server}] EXCEPTION: ${e.message}`);
    }
  }

  flowResult.classification = classify(flowResult.react, flowResult.leptos);
  flowResult.notes = '';
  console.log(`  → ${flowResult.classification}`);
  return flowResult;
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log('E2E Regression — Utility Group (Phase 1: utility-02, utility-03)');
  console.log('==================================================================');

  mkdirp(config.screenshotsDir);

  const flows = [];
  for (const flow of FLOWS) {
    const result = await runFlow(flow);
    flows.push(result);
  }

  // Save results
  const resultsPath = path.join(config.screenshotsDir, 'results-phase1.json');
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
    const icon = f.classification === 'PASS' ? '✓' : '✗';
    console.log(`${icon} [${f.classification}] ${f.name}`);
    counts[f.classification] = (counts[f.classification] || 0) + 1;
  });
  console.log('\nCounts:', JSON.stringify(counts));
  console.log(`\nResults saved to: ${resultsPath}`);
}

main().catch((e) => {
  console.error('Fatal error:', e);
  process.exit(1);
});
