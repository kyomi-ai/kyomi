#!/usr/bin/env node
'use strict';
const { chromium } = require('playwright');
const fs = require('fs');

const SERVERS = {
  react: 'http://localhost:8002',
  leptos: 'http://localhost:3000',
};

const EMAIL = 'e2e-test@kyomi.dev';
const PASSWORD = 'E2eTestPass123!';
const TIMEOUTS = { nav: 15000, action: 8000 };

async function loginAndGetPage(server, baseUrl) {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });
  await page.fill('input[type="email"], input[name="email"]', EMAIL, { timeout: TIMEOUTS.action });
  await page.fill('input[type="password"], input[name="password"]', PASSWORD, { timeout: TIMEOUTS.action });
  await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });
  await page.waitForURL(url => !url.toString().includes('/login'), { timeout: TIMEOUTS.nav });
  return { browser, context, page, baseUrl };
}

async function saveScreenshot(page, flowId, server) {
  const dir = `/tmp/e2e-regression/verify2/flows/${flowId}`;
  fs.mkdirSync(dir, { recursive: true });
  const filepath = `${dir}/${server}.png`;
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
}

function classify(react, leptos) {
  if (react.status === 'pass' && leptos.status === 'pass') return 'PASS';
  if (react.status === 'pass' && leptos.status === 'fail') return 'REGRESSION';
  if (react.status === 'fail' && leptos.status === 'fail') return 'BOTH_FAIL';
  return 'BEHAVIORAL_DIFF';
}

// flows-01: Home redirect
async function testFlows01(server) {
  const baseUrl = SERVERS[server];
  const { browser, page } = await loginAndGetPage(server, baseUrl);
  const assertions = [];

  try {
    // Navigate to /
    await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });

    // Assert: redirected away from /
    const currentUrl = page.url();
    const redirectedAway = !currentUrl.replace(baseUrl, '').match(/^\/?$/);
    assertions.push({
      pass: redirectedAway,
      message: redirectedAway
        ? `Redirected away from / — landed on: ${currentUrl}`
        : `Did NOT redirect away from / — still at: ${currentUrl}`,
    });

    // Assert: on /chat or chat content rendered
    const onChat = currentUrl.includes('/chat') || await page.locator('[data-testid="chat"], [aria-label*="chat" i], main').count() > 0;
    assertions.push({
      pass: onChat,
      message: onChat
        ? 'Chat route or chat content detected'
        : 'Neither /chat URL nor chat content found after redirect',
    });

    // Assert: sidebar logo img[alt="Kyomi"] has naturalWidth > 0
    const logoCount = await page.locator('img[alt="Kyomi"]').count();
    assertions.push({
      pass: logoCount > 0,
      message: logoCount > 0
        ? `Found ${logoCount} img[alt="Kyomi"] element(s)`
        : 'No img[alt="Kyomi"] found in sidebar',
    });

    if (logoCount > 0) {
      const naturalWidth = await page.locator('img[alt="Kyomi"]').first().evaluate(img => img.naturalWidth);
      assertions.push({
        pass: naturalWidth > 0,
        message: naturalWidth > 0
          ? `Logo naturalWidth=${naturalWidth} (loaded correctly)`
          : `Logo naturalWidth=0 — image failed to load (broken)`,
      });
    }

    const screenshot = await saveScreenshot(page, 'flows-01', server);
    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

const FLOWS = [
  { id: 'flows-01', name: 'Home redirect', fn: testFlows01 },
];

async function runFlow(flow) {
  const result = { id: flow.id, name: flow.name };
  for (const server of ['react', 'leptos']) {
    try {
      const { assertions, screenshot } = await flow.fn(server);
      const allPass = assertions.every(a => a.pass);
      result[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter(a => !a.pass).map(a => a.message).join('; '),
      };
    } catch (e) {
      result[server] = { status: 'fail', error: e.message, assertions: [], screenshot: null };
    }
  }
  result.classification = classify(result.react, result.leptos);
  return result;
}

async function main() {
  const flows = [];
  for (const flow of FLOWS) {
    console.log(`Running flow: ${flow.id} — ${flow.name}`);
    flows.push(await runFlow(flow));
  }
  const outputDir = '/tmp/e2e-regression/verify2/flows';
  fs.mkdirSync(outputDir, { recursive: true });
  const resultsPath = `${outputDir}/results.json`;
  fs.writeFileSync(resultsPath, JSON.stringify({ group: 'flows-01', timestamp: new Date().toISOString(), flows }, null, 2));
  console.log(`Results saved to: ${resultsPath}`);
}

main().catch(e => { console.error('Fatal:', e); process.exit(1); });
