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

const GROUP = 'auth-verify';
const FLOW_ID = 'auth-03';

function classify(react, leptos) {
  if (react.status === 'pass' && leptos.status === 'pass') return 'PASS';
  if (react.status === 'pass' && leptos.status === 'fail') return 'REGRESSION';
  if (react.status === 'fail' && leptos.status === 'fail') return 'BOTH_FAIL';
  return 'BEHAVIORAL_DIFF';
}

async function saveScreenshot(page, flowId, server) {
  const dir = `/tmp/e2e-regression/${GROUP}/${flowId}`;
  fs.mkdirSync(dir, { recursive: true });
  const filepath = `${dir}/${server}.png`;
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
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
  return { browser, context, page };
}

async function testAuth03Logout(server) {
  const baseUrl = SERVERS[server];
  const assertions = [];
  let screenshot = null;
  let browser = null;

  try {
    // Step 1: Login using auth-01 flow
    const session = await loginAndGetPage(baseUrl);
    browser = session.browser;
    const page = session.page;

    // Confirm we are authenticated (not on /login)
    const postLoginUrl = page.url();
    assertions.push({
      pass: !postLoginUrl.includes('/login'),
      message: `Login succeeded — landed at: ${postLoginUrl}`,
    });

    // Step 2: Click user avatar/menu in sidebar
    // Try data-testid first, then aria, then text-based selectors
    const avatarSelectors = [
      '[data-testid="user-avatar"]',
      '[data-testid="user-menu"]',
      '[data-testid="sidebar-user"]',
      'button[aria-label*="user" i]',
      'button[aria-label*="account" i]',
      'button[aria-label*="profile" i]',
      '[role="button"][aria-label*="user" i]',
    ];

    let avatarClicked = false;
    for (const sel of avatarSelectors) {
      try {
        const el = page.locator(sel).first();
        const visible = await el.isVisible({ timeout: 2000 });
        if (visible) {
          await el.click({ timeout: TIMEOUTS.action });
          avatarClicked = true;
          console.log(`[${server}] Clicked avatar via: ${sel}`);
          break;
        }
      } catch (_) {}
    }

    // If no specific avatar found, look for any sidebar button that might be user-related
    if (!avatarClicked) {
      // Try clicking on sidebar bottom area where user info usually lives
      const sidebarBottomSelectors = [
        'nav button:last-child',
        'aside button:last-child',
        '[data-testid="sidebar"] button:last-child',
        'button:has(img[alt*="avatar" i])',
        'button:has(img[alt*="user" i])',
      ];
      for (const sel of sidebarBottomSelectors) {
        try {
          const el = page.locator(sel).first();
          const visible = await el.isVisible({ timeout: 2000 });
          if (visible) {
            await el.click({ timeout: TIMEOUTS.action });
            avatarClicked = true;
            console.log(`[${server}] Clicked sidebar bottom via: ${sel}`);
            break;
          }
        } catch (_) {}
      }
    }

    assertions.push({
      pass: avatarClicked,
      message: avatarClicked
        ? 'User avatar/menu button found and clicked'
        : 'Could not find user avatar/menu button in sidebar',
    });

    if (!avatarClicked) {
      // Take screenshot to show current state
      screenshot = await saveScreenshot(page, FLOW_ID, server);
      await browser.close();
      return { assertions, screenshot };
    }

    // Wait briefly for menu to appear
    await page.waitForTimeout(500);

    // Step 3: Click "Sign Out" or "Logout"
    const logoutSelectors = [
      'button:has-text("Sign Out")',
      'button:has-text("Sign out")',
      'button:has-text("Logout")',
      'button:has-text("Log out")',
      'button:has-text("Log Out")',
      '[role="menuitem"]:has-text("Sign Out")',
      '[role="menuitem"]:has-text("Sign out")',
      '[role="menuitem"]:has-text("Logout")',
      '[role="menuitem"]:has-text("Log out")',
      '[role="menuitem"]:has-text("Log Out")',
      'a:has-text("Sign Out")',
      'a:has-text("Logout")',
      'a:has-text("Log out")',
      '[data-testid="logout"]',
      '[data-testid="sign-out"]',
    ];

    let logoutClicked = false;
    for (const sel of logoutSelectors) {
      try {
        const el = page.locator(sel).first();
        const visible = await el.isVisible({ timeout: 2000 });
        if (visible) {
          await el.click({ timeout: TIMEOUTS.action });
          logoutClicked = true;
          console.log(`[${server}] Clicked logout via: ${sel}`);
          break;
        }
      } catch (_) {}
    }

    assertions.push({
      pass: logoutClicked,
      message: logoutClicked
        ? 'Sign Out / Logout button found and clicked'
        : 'Could not find Sign Out or Logout button in user menu',
    });

    if (!logoutClicked) {
      screenshot = await saveScreenshot(page, FLOW_ID, server);
      await browser.close();
      return { assertions, screenshot };
    }

    // Wait for redirect to /login
    try {
      await page.waitForURL(url => url.toString().includes('/login'), { timeout: TIMEOUTS.nav });
    } catch (_) {
      // URL may not have changed — check below
    }

    // Take screenshot after logout
    screenshot = await saveScreenshot(page, FLOW_ID, server);

    // Assert: redirected to /login
    const finalUrl = page.url();
    const onLoginPage = finalUrl.includes('/login');
    assertions.push({
      pass: onLoginPage,
      message: onLoginPage
        ? `Redirected to /login after logout (url: ${finalUrl})`
        : `NOT redirected to /login — still at: ${finalUrl}`,
    });

    // Assert: no authenticated layout visible
    // Authenticated layout typically has sidebar nav or dashboard elements
    const authLayoutSelectors = [
      'nav[data-testid="sidebar"]',
      '[data-testid="app-sidebar"]',
      '[data-testid="main-nav"]',
      'aside[role="navigation"]',
    ];

    let authLayoutVisible = false;
    for (const sel of authLayoutSelectors) {
      try {
        const el = page.locator(sel).first();
        const visible = await el.isVisible({ timeout: 1000 });
        if (visible) {
          authLayoutVisible = true;
          break;
        }
      } catch (_) {}
    }

    assertions.push({
      pass: !authLayoutVisible,
      message: !authLayoutVisible
        ? 'Authenticated layout not visible after logout'
        : 'Authenticated layout still visible after logout — session not cleared',
    });

    // Assert: login form is visible on the page
    let loginFormVisible = false;
    try {
      const emailInput = page.locator('input[type="email"], input[name="email"]').first();
      loginFormVisible = await emailInput.isVisible({ timeout: 3000 });
    } catch (_) {}

    assertions.push({
      pass: loginFormVisible,
      message: loginFormVisible
        ? 'Login form visible after logout'
        : 'Login form NOT visible after logout',
    });

    await browser.close();
    return { assertions, screenshot };

  } catch (err) {
    if (browser) {
      try {
        // Attempt screenshot even on error
        const pages = browser.contexts()[0]?.pages() || [];
        if (pages.length > 0) {
          screenshot = await saveScreenshot(pages[0], FLOW_ID, server);
        }
      } catch (_) {}
      await browser.close();
    }
    throw err;
  }
}

async function runFlow(flowFn, flowId, flowName) {
  const result = { id: flowId, name: flowName };

  for (const server of ['react', 'leptos']) {
    console.log(`\n--- Running ${flowId} on ${server} ---`);
    try {
      const { assertions, screenshot } = await flowFn(server);
      const allPass = assertions.every(a => a.pass);
      result[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter(a => !a.pass).map(a => a.message).join('; '),
      };
      console.log(`[${server}] Status: ${result[server].status}`);
      assertions.forEach(a => console.log(`  [${a.pass ? 'PASS' : 'FAIL'}] ${a.message}`));
    } catch (err) {
      console.error(`[${server}] Fatal error:`, err.message);
      result[server] = {
        status: 'fail',
        assertions: [],
        screenshot: null,
        error: err.message,
      };
    }
  }

  result.classification = classify(result.react, result.leptos);
  result.notes = '';

  return result;
}

async function main() {
  console.log('=== auth-03: Logout ===\n');

  const flowResult = await runFlow(testAuth03Logout, FLOW_ID, 'Logout');
  flowResult.notes = buildNotes(flowResult);

  const output = {
    group: GROUP,
    timestamp: new Date().toISOString(),
    flows: [flowResult],
  };

  const outputDir = `/tmp/e2e-regression/${GROUP}`;
  fs.mkdirSync(outputDir, { recursive: true });
  const outputPath = `${outputDir}/results.json`;
  fs.writeFileSync(outputPath, JSON.stringify(output, null, 2));

  console.log(`\n=== RESULTS ===`);
  console.log(`Classification: ${flowResult.classification}`);
  console.log(`React: ${flowResult.react.status}`);
  console.log(`Leptos: ${flowResult.leptos.status}`);
  console.log(`Results saved to: ${outputPath}`);

  if (flowResult.react.screenshot) console.log(`React screenshot: ${flowResult.react.screenshot}`);
  if (flowResult.leptos.screenshot) console.log(`Leptos screenshot: ${flowResult.leptos.screenshot}`);
}

function buildNotes(result) {
  const notes = [];
  if (result.react.error) notes.push(`React: ${result.react.error}`);
  if (result.leptos.error) notes.push(`Leptos: ${result.leptos.error}`);
  return notes.join(' | ');
}

main().catch(e => {
  console.error('Fatal:', e);
  process.exit(1);
});
