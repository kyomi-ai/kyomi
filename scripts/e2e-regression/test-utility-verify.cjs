#!/usr/bin/env node
// E2E regression tests: utility page group — utility-02 and utility-03
// Tests the /unsubscribe page for form visibility and email pre-fill.

const { chromium } = require('playwright');
const fs = require('fs');

const SERVERS = {
  react: 'http://localhost:8002',
  leptos: 'http://localhost:3000',
};

const TIMEOUTS = { nav: 15000, action: 8000 };
const OUTPUT_DIR = '/tmp/e2e-regression/utility-verify';

// --- Helpers ---

function classify(react, leptos) {
  if (react.status === 'pass' && leptos.status === 'pass') return 'PASS';
  if (react.status === 'pass' && leptos.status === 'fail') return 'REGRESSION';
  if (react.status === 'fail' && leptos.status === 'fail') return 'BOTH_FAIL';
  return 'BEHAVIORAL_DIFF';
}

async function saveScreenshot(page, flowId, server) {
  const dir = `${OUTPUT_DIR}/${flowId}`;
  fs.mkdirSync(dir, { recursive: true });
  const filepath = `${dir}/${server}.png`;
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
}

// --- Flow: utility-02 — Unsubscribe page (basic form) ---

async function testUtility02(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();

  const assertions = [];

  try {
    await page.goto(`${baseUrl}/unsubscribe`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });

    // Assert: form visible
    const formVisible = await page.isVisible('form', { timeout: TIMEOUTS.action }).catch(() => false);
    assertions.push({ pass: formVisible, message: 'Unsubscribe form visible' });

    // Assert: email input visible
    const emailVisible = await page.isVisible('input[type="email"], input[name="email"]', { timeout: TIMEOUTS.action }).catch(() => false);
    assertions.push({ pass: emailVisible, message: 'Email input visible' });

    // Assert: Unsubscribe button visible with correct text
    const buttonVisible = await page.isVisible('button[type="submit"]', { timeout: TIMEOUTS.action }).catch(() => false);
    assertions.push({ pass: buttonVisible, message: 'Submit button visible' });

    let buttonTextOk = false;
    if (buttonVisible) {
      const buttonText = await page.textContent('button[type="submit"]');
      buttonTextOk = buttonText && buttonText.trim().includes('Unsubscribe');
    }
    assertions.push({ pass: buttonTextOk, message: 'Button text contains "Unsubscribe"' });

    // Assert: h1 heading visible
    const headingVisible = await page.isVisible('h1', { timeout: TIMEOUTS.action }).catch(() => false);
    assertions.push({ pass: headingVisible, message: 'H1 heading visible' });

    // Assert: no login redirect (still on /unsubscribe)
    const currentUrl = page.url();
    const notRedirected = !currentUrl.includes('/login') && !currentUrl.includes('/signin');
    assertions.push({ pass: notRedirected, message: `No login redirect — URL is ${currentUrl}` });

    const screenshot = await saveScreenshot(page, 'utility-02', server);

    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

// --- Flow: utility-03 — Unsubscribe page with prefilled email ---

async function testUtility03(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();

  const assertions = [];
  const TEST_EMAIL = 'test@example.com';

  try {
    await page.goto(`${baseUrl}/unsubscribe?email=${TEST_EMAIL}`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });

    // Assert: email input is pre-filled
    const emailInput = page.locator('input[type="email"], input[name="email"]');
    const emailVisible = await emailInput.isVisible({ timeout: TIMEOUTS.action }).catch(() => false);
    assertions.push({ pass: emailVisible, message: 'Email input visible' });

    let prefillOk = false;
    if (emailVisible) {
      const value = await emailInput.inputValue();
      prefillOk = value === TEST_EMAIL;
      assertions.push({ pass: prefillOk, message: `Email pre-filled with "${TEST_EMAIL}" — actual: "${value}"` });
    } else {
      assertions.push({ pass: false, message: `Email pre-filled with "${TEST_EMAIL}" — input not found` });
    }

    // Assert: no login redirect
    const currentUrl = page.url();
    const notRedirected = !currentUrl.includes('/login') && !currentUrl.includes('/signin');
    assertions.push({ pass: notRedirected, message: `No login redirect — URL is ${currentUrl}` });

    const screenshot = await saveScreenshot(page, 'utility-03', server);

    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

// --- Runner ---

const FLOWS = [
  { id: 'utility-02', name: 'Unsubscribe page — form view', fn: testUtility02 },
  { id: 'utility-03', name: 'Unsubscribe page — prefilled email', fn: testUtility03 },
];

async function runFlow(flow) {
  const result = { id: flow.id, name: flow.name };
  for (const server of ['react', 'leptos']) {
    console.log(`  [${server}] Running ${flow.id}...`);
    try {
      const { assertions, screenshot } = await flow.fn(server);
      const allPass = assertions.every(a => a.pass);
      result[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter(a => !a.pass).map(a => a.message).join('; '),
      };
      const icon = allPass ? '✓' : '✗';
      console.log(`  [${server}] ${icon} ${flow.id}: ${allPass ? 'PASS' : 'FAIL'}`);
      if (!allPass) {
        assertions.filter(a => !a.pass).forEach(a => console.log(`    FAIL: ${a.message}`));
      }
    } catch (e) {
      console.log(`  [${server}] ✗ ${flow.id}: ERROR — ${e.message}`);
      result[server] = { status: 'fail', error: e.message, assertions: [], screenshot: null };
    }
  }
  result.classification = classify(result.react, result.leptos);
  console.log(`  => classification: ${result.classification}`);
  return result;
}

async function main() {
  fs.mkdirSync(OUTPUT_DIR, { recursive: true });
  console.log('=== E2E Regression: utility page group ===\n');

  const flows = [];
  for (const flow of FLOWS) {
    console.log(`\nFlow: ${flow.id} — ${flow.name}`);
    flows.push(await runFlow(flow));
  }

  const results = {
    group: 'utility-verify',
    timestamp: new Date().toISOString(),
    flows,
  };

  const resultsPath = `${OUTPUT_DIR}/results.json`;
  fs.writeFileSync(resultsPath, JSON.stringify(results, null, 2));
  console.log(`\nResults saved to ${resultsPath}`);

  // Summary
  const passes = flows.filter(f => f.classification === 'PASS').length;
  const regressions = flows.filter(f => f.classification === 'REGRESSION').length;
  const bothFail = flows.filter(f => f.classification === 'BOTH_FAIL').length;
  const behDiff = flows.filter(f => f.classification === 'BEHAVIORAL_DIFF').length;
  console.log(`\nSummary: ${passes} PASS, ${regressions} REGRESSION, ${bothFail} BOTH_FAIL, ${behDiff} BEHAVIORAL_DIFF`);
}

main().catch(e => { console.error('Fatal:', e); process.exit(1); });
