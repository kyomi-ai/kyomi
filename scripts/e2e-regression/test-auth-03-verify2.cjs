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

async function saveScreenshot(page, group, flowId, server) {
  const dir = `/tmp/e2e-regression/${group}/${flowId}`;
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

async function loginAndGetPage(baseUrl) {
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
  return { browser, page };
}

// auth-03: Logout
// React: user menu button has aria-label="E2E Test User", menu shows inline with "Logout" button
// Leptos: user button has text "E2E Test User", menu popup has <a href="/login">Logout</a>
async function testAuth03Logout(server) {
  const baseUrl = SERVERS[server];
  const assertions = [];
  let screenshot = null;
  let browser = null;

  try {
    const session = await loginAndGetPage(baseUrl);
    browser = session.browser;
    const page = session.page;

    // Assert we are authenticated (not on login page)
    const initialUrl = page.url();
    const loggedIn = !initialUrl.includes('/login');
    assertions.push({
      pass: loggedIn,
      message: loggedIn
        ? `Logged in successfully — URL: ${initialUrl}`
        : `Login failed — still on login URL: ${initialUrl}`,
    });

    if (!loggedIn) {
      screenshot = await saveScreenshot(page, 'verify2/auth', 'auth-03', server);
      await browser.close();
      return { assertions, screenshot };
    }

    // Click user menu button — works for both React (aria-label) and Leptos (text)
    // Both render a button with "E2E Test User" text
    let menuButtonClicked = false;
    const menuButtonSelectors = [
      'button[aria-label="E2E Test User"]',          // React
      'button:has-text("E2E Test User")',              // Leptos + React fallback
    ];

    for (const sel of menuButtonSelectors) {
      try {
        // Use waitForSelector with a generous timeout to handle slow page loads
        await page.waitForSelector(sel, { state: 'visible', timeout: TIMEOUTS.action });
        await page.locator(sel).first().click({ timeout: TIMEOUTS.action });
        menuButtonClicked = true;
        break;
      } catch {
        // try next selector
      }
    }

    assertions.push({
      pass: menuButtonClicked,
      message: menuButtonClicked
        ? 'User avatar/menu button clicked successfully'
        : 'Could not find user avatar/menu button in sidebar',
    });

    if (!menuButtonClicked) {
      screenshot = await saveScreenshot(page, 'verify2/auth', 'auth-03', server);
      await browser.close();
      return { assertions, screenshot };
    }

    // Wait for menu to appear
    await page.waitForTimeout(600);

    // Click Sign Out / Logout — both React ("Logout" button) and Leptos ("Logout" link)
    let logoutClicked = false;
    const logoutSelectors = [
      'text="Logout"',
      'text="Sign Out"',
      'text="Sign out"',
      'text="Log out"',
      'text="Log Out"',
      'button:has-text("Logout")',
      'button:has-text("Sign Out")',
      'a:has-text("Logout")',
      'a:has-text("Sign out")',
      '[role="menuitem"]:has-text("Logout")',
      '[role="menuitem"]:has-text("Sign out")',
    ];

    for (const sel of logoutSelectors) {
      const el = page.locator(sel).first();
      if (await el.isVisible({ timeout: 2000 }).catch(() => false)) {
        await el.click({ timeout: TIMEOUTS.action });
        logoutClicked = true;
        break;
      }
    }

    assertions.push({
      pass: logoutClicked,
      message: logoutClicked
        ? 'Sign Out / Logout button clicked'
        : 'Could not find Sign Out / Logout option in user menu',
    });

    if (!logoutClicked) {
      screenshot = await saveScreenshot(page, 'verify2/auth', 'auth-03', server);
      await browser.close();
      return { assertions, screenshot };
    }

    // Wait for redirect to /login
    try {
      await page.waitForURL(url => url.toString().includes('/login'), { timeout: TIMEOUTS.nav });
    } catch {
      // will check manually below
    }

    await page.waitForTimeout(500);
    const finalUrl = page.url();

    // Assert: redirected to /login
    const onLoginPage = finalUrl.includes('/login');
    assertions.push({
      pass: onLoginPage,
      message: onLoginPage
        ? `Redirected to /login after logout — URL: ${finalUrl}`
        : `NOT redirected to /login after logout — URL: ${finalUrl}`,
    });

    // Assert: login form visible (no authenticated layout)
    const loginFormVisible = await page.locator('input[type="email"], input[name="email"]').isVisible({ timeout: 5000 }).catch(() => false);
    assertions.push({
      pass: loginFormVisible,
      message: loginFormVisible
        ? 'Login form visible — no authenticated layout after logout'
        : 'Login form NOT visible — authenticated layout may still be showing',
    });

    // Assert: sidebar navigation not visible (no authenticated sidebar)
    // Check for nav items that only exist when authenticated
    const sidebarNavVisible = await page.locator('text="New chat"').isVisible({ timeout: 2000 }).catch(() => false);
    assertions.push({
      pass: !sidebarNavVisible,
      message: !sidebarNavVisible
        ? 'Authenticated sidebar not visible after logout'
        : 'Authenticated sidebar still visible after logout — session not cleared',
    });

    screenshot = await saveScreenshot(page, 'verify2/auth', 'auth-03', server);
    await browser.close();
    return { assertions, screenshot };
  } catch (e) {
    if (browser) await browser.close().catch(() => {});
    throw e;
  }
}

const FLOWS = [
  { id: 'auth-03', name: 'Logout', fn: testAuth03Logout },
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
    console.log(`[${server.toUpperCase()}] auth-03: ${result[server].status.toUpperCase()} — ${result[server].error || 'all assertions passed'}`);
  }
  result.classification = classify(result.react, result.leptos);
  result.notes = '';
  return result;
}

async function main() {
  const flows = [];
  for (const flow of FLOWS) {
    flows.push(await runFlow(flow));
  }

  const outputDir = '/tmp/e2e-regression/verify2/auth';
  fs.mkdirSync(outputDir, { recursive: true });
  const resultsPath = `${outputDir}/results.json`;
  fs.writeFileSync(resultsPath, JSON.stringify({
    group: 'auth',
    timestamp: new Date().toISOString(),
    flows,
  }, null, 2));

  console.log(`\nResults saved to: ${resultsPath}`);
  for (const flow of flows) {
    console.log(`\n[${flow.classification}] ${flow.id}: ${flow.name}`);
    for (const server of ['react', 'leptos']) {
      const r = flow[server];
      console.log(`  ${server}: ${r.status}${r.error ? ` — ${r.error}` : ''}`);
      for (const a of r.assertions || []) {
        console.log(`    ${a.pass ? '✓' : '✗'} ${a.message}`);
      }
    }
  }
}

main().catch(e => { console.error('Fatal:', e); process.exit(1); });
