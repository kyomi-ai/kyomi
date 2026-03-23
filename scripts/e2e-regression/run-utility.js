#!/usr/bin/env node
/**
 * E2E Regression Tests — Utility Group
 * Tests: welcome, unsubscribe, accept-ownership
 *
 * Run: node /tmp/e2e-utility-test.js
 */

const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const config = {
  servers: {
    react: 'http://localhost:8002',
    leptos: 'http://localhost:3000',
  },
  testUser: {
    email: 'e2e-test@kyomi.dev',
    password: 'E2eTestPass123!',
  },
  timeouts: {
    navigation: 15000,
    action: 8000,
  },
  screenshotsDir: '/tmp/e2e-regression/utility',
};

// ── Helpers ──────────────────────────────────────────────────────────────────

async function launchPage(server) {
  const baseUrl = config.servers[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  return { browser, context, page, baseUrl };
}

async function loginAndGetPage(server) {
  const { browser, context, page, baseUrl } = await launchPage(server);
  await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: config.timeouts.navigation });
  await page.fill('input[type="email"], input[name="email"]', config.testUser.email, { timeout: config.timeouts.action });
  await page.fill('input[type="password"], input[name="password"]', config.testUser.password, { timeout: config.timeouts.action });
  await page.click('button[type="submit"]', { timeout: config.timeouts.action });
  await page.waitForURL(url => !url.toString().includes('/login'), { timeout: config.timeouts.navigation });
  return { browser, context, page, baseUrl };
}

async function saveScreenshot(page, group, flow, server) {
  const dir = path.join(config.screenshotsDir, flow);
  fs.mkdirSync(dir, { recursive: true });
  const filepath = path.join(dir, `${server}.png`);
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
}

function result(pass, message) {
  return { pass, message };
}

// ── Flow Tests ────────────────────────────────────────────────────────────────

/**
 * utility-01: Welcome page (no temp_token → redirect to login)
 */
async function testWelcome(server) {
  const { browser, page, baseUrl } = await launchPage(server);
  const assertions = [];
  let screenshot = null;

  try {
    await page.goto(`${baseUrl}/welcome`, { waitUntil: 'networkidle', timeout: config.timeouts.navigation });

    // Should redirect to /login (no valid token)
    const url = page.url();
    assertions.push(url.includes('/login')
      ? result(true, `Redirected to login — got: ${url}`)
      : result(false, `Expected redirect to /login, got: ${url}`)
    );

    screenshot = await saveScreenshot(page, 'utility', 'utility-01-welcome', server);
  } catch (e) {
    assertions.push(result(false, `Error: ${e.message}`));
  } finally {
    await browser.close();
  }

  return { assertions, screenshot };
}

/**
 * utility-02: Unsubscribe page (no login required)
 */
async function testUnsubscribe(server) {
  const { browser, page, baseUrl } = await launchPage(server);
  const assertions = [];
  let screenshot = null;

  try {
    await page.goto(`${baseUrl}/unsubscribe`, { waitUntil: 'networkidle', timeout: config.timeouts.navigation });

    // Should load without auth
    assertions.push(!page.url().includes('/login')
      ? result(true, 'Page loads without requiring auth')
      : result(false, `Unexpected redirect to login: ${page.url()}`)
    );

    // Email input visible
    const emailInput = await page.$('input[type="email"], input[name="email"]');
    assertions.push(emailInput
      ? result(true, 'Email input visible')
      : result(false, 'Email input not found')
    );

    // Unsubscribe button visible
    const btn = await page.$('button[type="submit"], button:has-text("Unsubscribe")');
    assertions.push(btn
      ? result(true, 'Submit/Unsubscribe button visible')
      : result(false, 'Submit button not found')
    );

    screenshot = await saveScreenshot(page, 'utility', 'utility-02-unsubscribe', server);
  } catch (e) {
    assertions.push(result(false, `Error: ${e.message}`));
  } finally {
    await browser.close();
  }

  return { assertions, screenshot };
}

/**
 * utility-03: Unsubscribe with prefilled email (?email=...)
 */
async function testUnsubscribePrefilled(server) {
  const { browser, page, baseUrl } = await launchPage(server);
  const assertions = [];
  let screenshot = null;

  try {
    await page.goto(`${baseUrl}/unsubscribe?email=test@example.com`, { waitUntil: 'networkidle', timeout: config.timeouts.navigation });

    // Email input should have prefilled value
    const emailInput = await page.$('input[type="email"], input[name="email"]');
    if (emailInput) {
      const value = await emailInput.inputValue();
      assertions.push(value === 'test@example.com'
        ? result(true, `Email pre-filled: ${value}`)
        : result(false, `Email not pre-filled, got: "${value}"`)
      );
    } else {
      assertions.push(result(false, 'Email input not found'));
    }

    screenshot = await saveScreenshot(page, 'utility', 'utility-03-unsubscribe-prefilled', server);
  } catch (e) {
    assertions.push(result(false, `Error: ${e.message}`));
  } finally {
    await browser.close();
  }

  return { assertions, screenshot };
}

/**
 * utility-04: Accept ownership with invalid ID
 */
async function testAcceptOwnershipInvalid(server) {
  let ctx;
  const assertions = [];
  let screenshot = null;

  try {
    ctx = await loginAndGetPage(server);
    const { browser, page, baseUrl } = ctx;

    await page.goto(`${baseUrl}/accept-ownership/invalid-id-12345`, { waitUntil: 'networkidle', timeout: config.timeouts.navigation });

    // Should show error state (not 404 crash)
    const body = await page.textContent('body');
    const hasError = body.toLowerCase().includes('not found') ||
                     body.toLowerCase().includes('invalid') ||
                     body.toLowerCase().includes('transfer') ||
                     body.toLowerCase().includes('error');
    assertions.push(hasError
      ? result(true, 'Error state visible for invalid transfer ID')
      : result(false, `No error state found. Body excerpt: ${body.slice(0, 200)}`)
    );

    // Should have a link/button to navigate away
    const navLink = await page.$('a[href="/"], a[href*="dashboard"], a[href*="chat"], button');
    assertions.push(navLink
      ? result(true, 'Navigation link/button present')
      : result(false, 'No navigation link found — user may be stuck')
    );

    screenshot = await saveScreenshot(page, 'utility', 'utility-04-accept-ownership-invalid', server);
    await browser.close();
  } catch (e) {
    assertions.push(result(false, `Error: ${e.message}`));
    if (ctx) await ctx.browser.close().catch(() => {});
  }

  return { assertions, screenshot };
}

// ── Run All Flows ──────────────────────────────────────────────────────────────

const FLOWS = [
  { id: 'utility-01', name: 'Welcome page (no token → redirect to login)', fn: testWelcome, requiresAuth: false },
  { id: 'utility-02', name: 'Unsubscribe page loads without auth', fn: testUnsubscribe, requiresAuth: false },
  { id: 'utility-03', name: 'Unsubscribe with prefilled email', fn: testUnsubscribePrefilled, requiresAuth: false },
  { id: 'utility-04', name: 'Accept ownership with invalid ID', fn: testAcceptOwnershipInvalid, requiresAuth: true },
];

function classify(react, leptos) {
  if (react.status === 'pass' && leptos.status === 'pass') return 'PASS';
  if (react.status === 'pass' && leptos.status === 'fail') return 'REGRESSION';
  if (react.status === 'fail' && leptos.status === 'pass') return 'IMPROVEMENT';
  return 'BOTH_FAIL';
}

async function runFlow(flow) {
  console.log(`\nRunning: ${flow.id} — ${flow.name}`);
  const flowResult = { name: flow.name, id: flow.id };

  for (const server of ['react', 'leptos']) {
    try {
      console.log(`  [${server}]...`);
      const { assertions, screenshot } = await flow.fn(server);
      const allPass = assertions.every(a => a.pass);
      flowResult[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter(a => !a.pass).map(a => a.message).join('; '),
      };
      console.log(`  [${server}] ${allPass ? '✓ PASS' : '✗ FAIL'}`);
      if (!allPass) {
        assertions.filter(a => !a.pass).forEach(a => console.log(`    ✗ ${a.message}`));
      }
    } catch (e) {
      flowResult[server] = { status: 'fail', error: e.message, assertions: [], screenshot: null };
      console.log(`  [${server}] ✗ EXCEPTION: ${e.message}`);
    }
  }

  flowResult.classification = classify(flowResult.react, flowResult.leptos);
  console.log(`  Classification: ${flowResult.classification}`);
  return flowResult;
}

async function main() {
  console.log('E2E Regression — Utility Group');
  console.log('==============================');

  // Run flows sequentially (to avoid too many browser instances)
  const flows = [];
  for (const flow of FLOWS) {
    const r = await runFlow(flow);
    flows.push(r);
  }

  // Save results
  fs.mkdirSync('/tmp/e2e-regression/utility', { recursive: true });
  const resultsPath = '/tmp/e2e-regression/utility/results.json';
  fs.writeFileSync(resultsPath, JSON.stringify({ group: 'utility', flows, timestamp: new Date().toISOString() }, null, 2));

  // Print summary
  console.log('\n=== SUMMARY ===');
  const counts = { PASS: 0, REGRESSION: 0, IMPROVEMENT: 0, BOTH_FAIL: 0 };
  flows.forEach(f => {
    counts[f.classification] = (counts[f.classification] || 0) + 1;
    const icon = f.classification === 'PASS' ? '✓' : '✗';
    console.log(`${icon} [${f.classification}] ${f.name}`);
  });
  console.log('\nCounts:', counts);
  console.log(`\nResults saved to: ${resultsPath}`);
}

main().catch(e => {
  console.error('Fatal error:', e);
  process.exit(1);
});
