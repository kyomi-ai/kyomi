#!/usr/bin/env node
// E2E regression tests for auth page group
// auth-01: Login with email/password
// auth-02: Login with wrong password  (runs LAST — triggers rate limit)
// auth-03: Logout
// auth-04: Password recovery start
//
// NOTE: auth-02 runs last because wrong-password attempts trigger a 15-minute
// rate limit on the shared IP. If rate-limited, clear with:
//   podman exec kyomi-redis-dev redis-cli del "ratelimit:ip:unknown:login"

const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const SERVERS = {
  react: 'http://localhost:8002',
  leptos: 'http://localhost:3000',
};

const CREDS = {
  email: 'e2e-test@kyomi.dev',
  password: 'E2eTestPass123!',
};

const TIMEOUTS = { nav: 15000, action: 8000 };
const GROUP = 'auth';

// ─── Helpers ──────────────────────────────────────────────────────────────────

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

async function doLogin(page, baseUrl) {
  await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });
  // React needs extra time to hydrate the SPA before inputs appear
  await page.waitForTimeout(1500);
  await page.fill('input[type="email"], input[name="email"]', CREDS.email, { timeout: TIMEOUTS.action });
  await page.fill('input[type="password"], input[name="password"]', CREDS.password, { timeout: TIMEOUTS.action });
  await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });
  // Extended nav timeout — allows for slower server responses under load
  await page.waitForURL(url => !url.toString().includes('/login'), { timeout: 25000 });
  // Wait for the app to fully render after redirect
  await page.waitForLoadState('networkidle').catch(() => {});
  await page.waitForTimeout(2000);
}

// ─── auth-01: Login with email/password ───────────────────────────────────────

async function testAuth01(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  const assertions = [];

  try {
    await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });
    await page.waitForTimeout(1500);
    await page.fill('input[type="email"], input[name="email"]', CREDS.email, { timeout: TIMEOUTS.action });
    await page.fill('input[type="password"], input[name="password"]', CREDS.password, { timeout: TIMEOUTS.action });
    await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });
    await page.waitForURL(url => !url.toString().includes('/login'), { timeout: TIMEOUTS.nav });
    await page.waitForLoadState('networkidle').catch(() => {});
    await page.waitForTimeout(2000);

    const currentUrl = page.url();
    assertions.push({
      pass: !currentUrl.includes('/login'),
      message: `Redirected away from /login — current URL: ${currentUrl}`,
    });

    // No error alert visible
    const errorAlert = await page.locator('[role="alert"]').filter({ hasText: /invalid|error|wrong|fail/i }).count();
    assertions.push({
      pass: errorAlert === 0,
      message: errorAlert === 0
        ? 'No error alert visible after login'
        : `Error alert visible after successful login (count: ${errorAlert})`,
    });

    // Authenticated layout visible — check for a known nav item text present on both React and Leptos
    const navItemVisible = await page.locator('button:has-text("New chat"), a:has-text("New chat"), button:has-text("Chats"), a:has-text("Chats")').first().isVisible({ timeout: 5000 }).catch(() => false);
    assertions.push({
      pass: navItemVisible,
      message: navItemVisible
        ? 'Authenticated sidebar nav items visible'
        : 'Authenticated sidebar nav items NOT visible after login',
    });

    const screenshot = await saveScreenshot(page, 'auth-01', server);
    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

// ─── auth-03: Logout ──────────────────────────────────────────────────────────

async function testAuth03(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  const assertions = [];

  try {
    // Step 1: Login
    await doLogin(page, baseUrl);

    // Step 2: Find user menu button
    // React: button with aria-label containing user's name, OR button text contains "E2E Test"
    // Leptos: button without aria-label but text contains "E2E Test"
    // Both render: initial letter + user name + workspace text as the button content
    const userMenuSelectors = [
      'button[aria-label="E2E Test User"]',                        // React aria-label
      'button:has-text("E2E Test User")',                          // Both (text content)
      'button:has-text("E2E Test Workspace")',                     // Both (workspace text in button)
    ];

    let clicked = false;
    for (const sel of userMenuSelectors) {
      const count = await page.locator(sel).count();
      if (count > 0) {
        await page.locator(sel).first().click({ timeout: TIMEOUTS.action });
        clicked = true;
        break;
      }
    }

    assertions.push({
      pass: clicked,
      message: clicked ? 'User menu button found and clicked' : 'User menu button NOT found',
    });

    if (!clicked) {
      const screenshot = await saveScreenshot(page, 'auth-03', server);
      return { assertions, screenshot };
    }

    // Step 3: Wait for dropdown and click Logout
    // React: button with text "Logout" (calls API then navigates)
    // Leptos: <a href="/login"> with text "Logout" (direct link, no API call — known regression)
    await page.waitForSelector('a:has-text("Logout"), button:has-text("Logout")', { timeout: TIMEOUTS.action });
    await page.click('a:has-text("Logout"), button:has-text("Logout")', { timeout: TIMEOUTS.action });

    // Wait for navigation to /login — extended timeout for slow server responses
    await page.waitForURL(url => url.toString().includes('/login'), { timeout: 20000 });

    const currentUrl = page.url();
    assertions.push({
      pass: currentUrl.includes('/login'),
      message: `Redirected to /login after logout — current URL: ${currentUrl}`,
    });

    // Login form should be visible again — not still the authenticated layout
    await page.waitForLoadState('networkidle').catch(() => {});
    await page.waitForTimeout(1500);
    const loginFormVisible = await page.locator('input[type="email"], input[name="email"]').isVisible({ timeout: 5000 }).catch(() => false);
    assertions.push({
      pass: loginFormVisible,
      message: loginFormVisible
        ? 'Login form visible after logout (unauthenticated state)'
        : 'Login form NOT visible after logout',
    });

    // Nav items should NOT be present (not authenticated)
    const navStillVisible = await page.locator('button:has-text("New chat"), a:has-text("New chat")').first().isVisible({ timeout: 2000 }).catch(() => false);
    assertions.push({
      pass: !navStillVisible,
      message: !navStillVisible
        ? 'Authenticated nav items not visible after logout'
        : 'Authenticated nav items still visible after logout — session may still be active',
    });

    // Session invalidation check: navigate to a protected route — should redirect back to /login
    // React calls the logout API and clears cookies, so this should redirect to /login
    // Leptos only navigates to /login without clearing cookies, so this may stay authenticated
    await page.goto(`${baseUrl}/`, { waitUntil: 'networkidle', timeout: 15000 });
    await page.waitForTimeout(2000);
    const urlAfterProtectedNav = page.url();
    const sessionInvalidated = urlAfterProtectedNav.includes('/login');
    assertions.push({
      pass: sessionInvalidated,
      message: sessionInvalidated
        ? 'Session properly invalidated — protected route redirects to /login'
        : `Session NOT invalidated after logout — protected route landed on: ${urlAfterProtectedNav} (auth cookies still valid)`,
    });

    const screenshot = await saveScreenshot(page, 'auth-03', server);
    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

// ─── auth-04: Password recovery start ────────────────────────────────────────

async function testAuth04(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  const assertions = [];

  try {
    await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });
    // Wait for SPA hydration (React needs time after networkidle to render links)
    await page.waitForTimeout(2000);

    // Find the recovery link: Leptos and React both use "Can't sign in?" pointing to /account/recover
    const recoveryLink = page.locator('a[href*="recover"], a:has-text("Can\'t sign in"), a:has-text("Forgot password")');
    const recoveryLinkVisible = await recoveryLink.first().isVisible({ timeout: TIMEOUTS.action });
    assertions.push({
      pass: recoveryLinkVisible,
      message: recoveryLinkVisible
        ? 'Recovery link visible on login page'
        : 'Recovery link NOT visible on login page',
    });

    if (!recoveryLinkVisible) {
      const screenshot = await saveScreenshot(page, 'auth-04', server);
      return { assertions, screenshot };
    }

    await recoveryLink.first().click({ timeout: TIMEOUTS.action });
    await page.waitForURL(url => url.toString().includes('/account/recover'), { timeout: TIMEOUTS.nav });
    await page.waitForLoadState('networkidle').catch(() => {});
    await page.waitForTimeout(1000);

    // Fill in the email and submit
    await page.fill('input[type="email"], input[id="recovery-email"]', CREDS.email, { timeout: TIMEOUTS.action });
    await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });

    // Wait for success transition — Leptos transitions to "Check Your Email" card
    await page.waitForTimeout(3000);

    const pageContent = await page.content();
    const hasSuccessContent =
      pageContent.toLowerCase().includes('check your email') ||
      pageContent.toLowerCase().includes('if a verified account') ||
      pageContent.toLowerCase().includes('recovery link') ||
      pageContent.toLowerCase().includes('we have sent');

    assertions.push({
      pass: hasSuccessContent,
      message: hasSuccessContent
        ? 'Success message shown after recovery email submission'
        : 'No success message found after recovery submission',
    });

    // No error visible
    const errorAlert = await page.locator('[role="alert"]').filter({ hasText: /error|failed|invalid/i }).count();
    assertions.push({
      pass: errorAlert === 0,
      message: errorAlert === 0
        ? 'No error alert after recovery submission'
        : `Error alert visible after recovery submission (count: ${errorAlert})`,
    });

    const screenshot = await saveScreenshot(page, 'auth-04', server);
    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

// ─── auth-02: Login with wrong password (RUNS LAST — triggers rate limit) ─────

async function testAuth02(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  const assertions = [];

  try {
    await page.goto(`${baseUrl}/login`, { waitUntil: 'networkidle', timeout: TIMEOUTS.nav });
    await page.waitForTimeout(1500);
    await page.fill('input[type="email"], input[name="email"]', CREDS.email, { timeout: TIMEOUTS.action });
    await page.fill('input[type="password"], input[name="password"]', 'wrongpassword123', { timeout: TIMEOUTS.action });
    await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });

    // Wait for either error to appear or a short timeout
    await Promise.race([
      page.waitForSelector('[role="alert"]', { timeout: 8000 }),
      page.waitForTimeout(5000),
    ]).catch(() => {});

    const currentUrl = page.url();
    assertions.push({
      pass: currentUrl.includes('/login'),
      message: `Still on /login after wrong password — current URL: ${currentUrl}`,
    });

    // Error message visible — accept any rejection: invalid credentials OR rate limited
    const errorAlert = await page.locator('[role="alert"]').count();
    const errorText = errorAlert > 0 ? await page.locator('[role="alert"]').first().textContent() : '';
    const hasErrorText =
      errorText.toLowerCase().includes('invalid') ||
      errorText.toLowerCase().includes('incorrect') ||
      errorText.toLowerCase().includes('wrong') ||
      errorText.toLowerCase().includes('failed') ||
      errorText.toLowerCase().includes('error') ||
      errorText.toLowerCase().includes('password') ||
      errorText.toLowerCase().includes('credentials') ||
      errorText.toLowerCase().includes('rate limit') ||
      errorText.toLowerCase().includes('too many');
    assertions.push({
      pass: errorAlert > 0 && hasErrorText,
      message: errorAlert > 0
        ? hasErrorText
          ? `Login rejected as expected: "${errorText.trim()}"`
          : `Alert visible but content unexpected: "${errorText.trim()}"`
        : 'No error alert visible after wrong password',
    });

    const screenshot = await saveScreenshot(page, 'auth-02', server);
    return { assertions, screenshot };
  } finally {
    await browser.close();
  }
}

// ─── Runner ───────────────────────────────────────────────────────────────────

// auth-02 runs LAST to avoid rate-limiting subsequent tests
const FLOWS = [
  { id: 'auth-01', name: 'Login with email/password', fn: testAuth01 },
  { id: 'auth-03', name: 'Logout', fn: testAuth03 },
  { id: 'auth-04', name: 'Password recovery start', fn: testAuth04 },
  { id: 'auth-02', name: 'Login with wrong password', fn: testAuth02 },
];

async function runFlow(flow) {
  console.log(`\nRunning ${flow.id}: ${flow.name}`);
  const result = { id: flow.id, name: flow.name };

  for (const server of ['react', 'leptos']) {
    console.log(`  [${server}]...`);
    try {
      const { assertions, screenshot } = await flow.fn(server);
      const allPass = assertions.every(a => a.pass);
      result[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter(a => !a.pass).map(a => a.message).join('; '),
      };
      if (allPass) {
        console.log(`    PASS`);
      } else {
        console.log(`    FAIL: ${result[server].error}`);
      }
    } catch (e) {
      console.log(`    ERROR: ${e.message}`);
      result[server] = { status: 'fail', error: e.message, assertions: [], screenshot: null };
    }
  }

  result.classification = classify(result.react, result.leptos);
  console.log(`  Classification: ${result.classification}`);
  return result;
}

async function main() {
  const outputDir = `/tmp/e2e-regression/${GROUP}`;
  fs.mkdirSync(outputDir, { recursive: true });

  const flows = [];
  for (const flow of FLOWS) {
    flows.push(await runFlow(flow));
  }

  // Sort results back to logical order for the report
  const orderedIds = ['auth-01', 'auth-02', 'auth-03', 'auth-04'];
  flows.sort((a, b) => orderedIds.indexOf(a.id) - orderedIds.indexOf(b.id));

  const results = {
    group: GROUP,
    timestamp: new Date().toISOString(),
    flows,
  };

  const resultsPath = `${outputDir}/results.json`;
  fs.writeFileSync(resultsPath, JSON.stringify(results, null, 2));
  console.log(`\nResults saved to ${resultsPath}`);

  // Summary
  console.log('\n=== SUMMARY ===');
  for (const flow of flows) {
    const reactStatus = flow.react?.status || 'error';
    const leptosStatus = flow.leptos?.status || 'error';
    console.log(`${flow.id} (${flow.name}): react=${reactStatus}, leptos=${leptosStatus} → ${flow.classification}`);
  }
}

main().catch(e => {
  console.error('Fatal:', e);
  process.exit(1);
});
