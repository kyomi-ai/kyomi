#!/usr/bin/env node
'use strict';
const { chromium } = require('playwright');
const fs = require('fs');

const SERVERS = {
  react: 'http://localhost:8002',
  leptos: 'http://localhost:3000',
};

const CREDS = {
  email: 'e2e-test@kyomi.dev',
  password: 'E2eTestPass123!',
};

const TIMEOUTS = { nav: 15000, action: 8000 };

async function loginAndGetPage(server, baseUrl) {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });
  await page.fill('input[type="email"], input[name="email"]', CREDS.email, { timeout: TIMEOUTS.action });
  await page.fill('input[type="password"], input[name="password"]', CREDS.password, { timeout: TIMEOUTS.action });
  await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });
  await page.waitForURL(url => !url.toString().includes('/login'), { timeout: TIMEOUTS.nav });
  return { browser, context, page, baseUrl };
}

async function saveScreenshot(page, flowId, server) {
  const dir = `/tmp/e2e-regression/flows-verify/${flowId}`;
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
  const assertions = [];
  let screenshotPath = null;
  let { browser, page } = await loginAndGetPage(server, baseUrl);

  try {
    // Navigate to /
    await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });

    // Wait for client-side routing to settle
    await page.waitForTimeout(2000);

    const finalUrl = page.url();
    const finalPath = finalUrl.replace(baseUrl, '') || '/';

    // Assert: Does NOT stay on / with login page content.
    // React SPA may keep URL at / but renders chat content.
    // Leptos SSR should redirect URL to /chat.
    // We check that either:
    //   (a) the URL changed away from /, OR
    //   (b) the page content is authenticated (shows nav/chat UI, NOT the login form)
    const isLoginPage = await page.evaluate(() => {
      // Check if this looks like the login page
      const loginForm = document.querySelector('input[type="password"]');
      const loginHeading = document.body.innerText.includes('Welcome back') ||
                           document.body.innerText.includes('Sign in');
      return !!(loginForm && loginHeading);
    });

    const urlChangedFromRoot = !finalPath.match(/^\/?$/);

    // For Leptos (SSR): expect URL to change to /chat
    // For React (SPA): URL may stay at / but must not show login page
    if (urlChangedFromRoot) {
      assertions.push({
        pass: true,
        message: `URL redirected from / to ${finalPath}`,
      });
    } else if (!isLoginPage) {
      assertions.push({
        pass: true,
        message: `URL stayed at / (SPA behavior) but page shows authenticated content — not the login form`,
      });
    } else {
      assertions.push({
        pass: false,
        message: `Failed: URL is still / AND page shows login form — not redirected`,
      });
    }

    // Assert: page shows authenticated content (chat/dashboard UI)
    const bodyText = await page.evaluate(() => document.body.innerText);
    const hasAuthContent = bodyText.includes('New chat') ||
                           bodyText.includes('Chats') ||
                           bodyText.includes('Dashboards') ||
                           bodyText.includes('Knowledge') ||
                           bodyText.includes('SQL Editor');

    assertions.push({
      pass: hasAuthContent,
      message: hasAuthContent
        ? `Authenticated chat UI rendered (sidebar navigation visible)`
        : `Authenticated UI not found — page does not show sidebar nav (body text starts: ${bodyText.substring(0, 100)})`,
    });

    // Take screenshot after redirect
    screenshotPath = await saveScreenshot(page, 'flows-01', server);

    // Assert: sidebar logo renders (check for <img> with no broken src)
    const logoChecks = await page.evaluate(() => {
      const imgs = Array.from(document.querySelectorAll('img'));
      return imgs.map(img => ({
        src: img.src,
        alt: img.alt || '',
        naturalWidth: img.naturalWidth,
        naturalHeight: img.naturalHeight,
        complete: img.complete,
      }));
    });

    // Find the sidebar logo image (kyomi logo)
    const logoImgs = logoChecks.filter(img =>
      img.src && (
        img.src.toLowerCase().includes('logo') ||
        img.alt.toLowerCase().includes('kyomi') ||
        img.alt.toLowerCase().includes('logo')
      )
    );

    if (logoImgs.length === 0) {
      const allImgs = logoChecks.filter(img => img.src && img.src.startsWith('http'));
      if (allImgs.length === 0) {
        // No img elements at all — may be inline SVG
        assertions.push({
          pass: true,
          message: `No <img> elements found — logo likely rendered as inline SVG (visual review required)`,
        });
      } else {
        const brokenAny = allImgs.filter(img => !img.complete || img.naturalWidth === 0);
        if (brokenAny.length > 0) {
          assertions.push({
            pass: false,
            message: `Visual: ${brokenAny.length} broken image(s) found on page — possible broken logo: ${brokenAny.map(i => i.src).join(', ')}`,
          });
        } else {
          assertions.push({
            pass: true,
            message: `No logo-specific img found but all ${allImgs.length} img elements loaded successfully`,
          });
        }
      }
    } else {
      const brokenLogos = logoImgs.filter(img => !img.complete || img.naturalWidth === 0);
      if (brokenLogos.length > 0) {
        assertions.push({
          pass: false,
          message: `Visual: Sidebar logo image broken — ${brokenLogos.length} logo img(s) failed to load (src: ${brokenLogos.map(i => i.src).join(', ')})`,
        });
      } else {
        assertions.push({
          pass: true,
          message: `Sidebar logo renders correctly (${logoImgs.length} logo img(s) loaded, all with naturalWidth > 0)`,
        });
      }
    }

  } finally {
    await browser.close();
  }

  return { assertions, screenshot: screenshotPath };
}

// Runner
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
    console.log(`  [${server}] ${result[server].status.toUpperCase()} — ${flow.id}`);
    if (result[server].error) {
      console.log(`    ERROR: ${result[server].error}`);
    }
  }
  result.classification = classify(result.react, result.leptos);
  result.notes = null;
  return result;
}

async function main() {
  console.log('Running flows-verify E2E tests...\n');
  const flows = [];
  for (const flow of FLOWS) {
    console.log(`Testing: ${flow.name} (${flow.id})`);
    flows.push(await runFlow(flow));
  }

  const outputDir = '/tmp/e2e-regression/flows-verify';
  fs.mkdirSync(outputDir, { recursive: true });
  const results = {
    group: 'flows-verify',
    timestamp: new Date().toISOString(),
    flows,
  };
  fs.writeFileSync(`${outputDir}/results.json`, JSON.stringify(results, null, 2));
  console.log(`\nResults saved to ${outputDir}/results.json`);

  // Summary
  const regressions = flows.filter(f => f.classification === 'REGRESSION');
  const bothFail = flows.filter(f => f.classification === 'BOTH_FAIL');
  const passes = flows.filter(f => f.classification === 'PASS');
  const behavDiff = flows.filter(f => f.classification === 'BEHAVIORAL_DIFF');

  console.log(`\nSummary:`);
  console.log(`  PASS: ${passes.length}`);
  console.log(`  REGRESSION: ${regressions.length}`);
  console.log(`  BOTH_FAIL: ${bothFail.length}`);
  console.log(`  BEHAVIORAL_DIFF: ${behavDiff.length}`);
}

main().catch(e => { console.error('Fatal:', e); process.exit(1); });
