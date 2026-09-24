// KYO-806 billing paywall regression.
//
// A lapsed SaaS workspace owner must see ONLY a full-screen paywall — no
// sidebar, no settings, no read-only app access — regardless of which URL
// they're on. A non-owner member of the same lapsed workspace sees the same
// paywall with an "ask the owner" message and no pay button. An active
// workspace must never show the paywall.
//
// This script flips the SHARED e2e test workspace
// (`e2e-test-workspace-0001`, seeded by `seed-test-user.py`) between
// `active` and `past_due` via a direct Postgres UPDATE — there is no UI
// path to make a workspace's subscription lapse, and no live Stripe
// webhooks fire against a dev server. The paywall only ever shows in SaaS
// mode (self-hosted/personal deployments have no billing at all), so this
// script must run against a SaaS-mode server: dev.kyomi.ai itself, or a
// local dev server started per .claude/CLAUDE.md's "Local Dev Server"
// section (SaaS mode on port 3000, never `SELF_HOSTED=true`). The
// workspace's original subscription row is captured before the first
// mutation and restored in a `finally` block so this script leaves the
// shared fixture exactly as it found it, whether it passes, fails, or
// throws.
//
// Because `e2e-admin@kyomi.dev` (owner) and `e2e-test@kyomi.dev`
// (non-owner) share that one workspace, lapsing it once covers both the
// owner and non-owner scenarios without needing a second seeded workspace.
//
// Scenario coverage:
//   1. Owner, lapsed workspace: paywall shows at `/`, `/dashboards`,
//      `/chat`, and `/settings/billing` — no sidebar, address bar
//      unchanged, pay CTA present.
//   2. Non-owner, lapsed workspace: paywall shows with the owner's
//      contact, no pay button.
//   3. Active workspace: no paywall, normal sidebar + page content.
//   4. 402-while-open: an already-open, already-authenticated tab (loaded
//      while active) locks into the paywall after the workspace is lapsed
//      out from under it and Create Dashboard fires a gated server-fn call
//      — without a full page reload.
//
// The workspace is lapsed via `subscription_status = 'past_due'` with no
// `stripe_subscription_id` on the row (the seeded workspace never had a
// live Stripe subscription). Per `checkout_path`
// (`crates/kyomi-auth/src/subscription_service.rs`), that routes the
// paywall's CTA to `Subscribe`, never `RecoverPayment` — this script
// therefore asserts the generic "Subscribe now" pay CTA, not the
// `RecoverPayment` flow (which needs a real Stripe customer/subscription
// this local environment doesn't have). Driving an actual embedded
// checkout to completion is out of scope here regardless — Stripe's
// hosted iframe isn't something this script can drive reliably — so
// clicking the CTA is intentionally not exercised past its click.
//
// Run (default PORT is 3000, the standard local dev-server port):
//     NODE_PATH=/home/jason/repos/kyomi/node_modules \
//         node scripts/e2e-regression/billing-paywall.cjs
//
// Overrides (all optional):
//   E2E_BASE_URL    - app base URL      (default http://localhost:3000, or
//                                         http://localhost:$PORT if PORT is set)
//   DATABASE_URL    - Postgres connection string for the direct workspace
//                      mutation (default matches seed-test-user.py's default:
//                      postgresql://kyomi:password@localhost:5433/kyomi)
//
// Exits 0 when every scenario passes, 1 otherwise. Screenshots (full page,
// 1920x1080) saved to /tmp/kyo-806-*.png for each scenario regardless of
// pass/fail, for visual review.

const { chromium } = require('playwright');
const { execFileSync } = require('child_process');

const BASE_URL = process.env.E2E_BASE_URL || `http://localhost:${process.env.PORT || '3000'}`;
const DATABASE_URL =
  process.env.DATABASE_URL || 'postgresql://kyomi:password@localhost:5433/kyomi';

const WORKSPACE_ID = 'e2e-test-workspace-0001';
const OWNER_EMAIL = 'e2e-admin@kyomi.dev';
const OWNER_PASSWORD = 'E2eAdminPass123!';
const NON_OWNER_EMAIL = 'e2e-test@kyomi.dev';
const NON_OWNER_PASSWORD = 'E2eTestPass123!';

let failed = false;
const fail = (msg) => {
  failed = true;
  console.log(`FAIL: ${msg}`);
};
const ok = (msg) => console.log(`PASS: ${msg}`);

// ── Direct DB mutation (no UI path exists to lapse a workspace locally) ────

function psql(sql) {
  return execFileSync('psql', [DATABASE_URL, '-t', '-A', '-c', sql], {
    encoding: 'utf8',
  }).trim();
}

function captureWorkspaceSubscriptionState() {
  const row = psql(
    `SELECT subscription_status, COALESCE(stripe_subscription_id, ''), ` +
      `COALESCE(stripe_customer_id, ''), COALESCE(subscription_period_end::text, ''), ` +
      `COALESCE(trial_ends_at::text, '') ` +
      `FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
  );
  const [status, subId, custId, periodEnd, trialEnd] = row.split('|');
  return { status, subId, custId, periodEnd, trialEnd };
}

function restoreWorkspaceSubscriptionState(state) {
  psql(
    `UPDATE workspaces SET ` +
      `subscription_status = '${state.status}', ` +
      `stripe_subscription_id = ${state.subId ? `'${state.subId}'` : 'NULL'}, ` +
      `stripe_customer_id = ${state.custId ? `'${state.custId}'` : 'NULL'}, ` +
      `subscription_period_end = ${state.periodEnd ? `'${state.periodEnd}'` : 'NULL'}, ` +
      `trial_ends_at = ${state.trialEnd ? `'${state.trialEnd}'` : 'NULL'} ` +
      `WHERE workspace_id = '${WORKSPACE_ID}'`
  );
}

function lapseWorkspace() {
  // past_due with no stripe_subscription_id — see the module doc for why
  // this routes the paywall to the `Subscribe` CTA rather than
  // `RecoverPayment`.
  psql(
    `UPDATE workspaces SET subscription_status = 'past_due', stripe_subscription_id = NULL ` +
      `WHERE workspace_id = '${WORKSPACE_ID}'`
  );
}

function activateWorkspace(state) {
  psql(
    `UPDATE workspaces SET subscription_status = 'active' WHERE workspace_id = '${WORKSPACE_ID}'`
  );
  void state; // captured state is restored at the very end, not here
}

// ── Playwright helpers ──────────────────────────────────────────────────────

async function login(page, email, password) {
  await page.goto(`${BASE_URL}/login`, { waitUntil: 'networkidle', timeout: 30000 });
  await page.fill('input[type="email"]', email, { timeout: 8000 });
  await page.fill('input[type="password"]', password, { timeout: 8000 });
  await page.click('button[type="submit"]', { timeout: 8000 });
  await page.waitForURL((u) => !u.toString().includes('/login'), { timeout: 20000 });
}

/** Any nav item that only ever renders inside the authenticated app shell. */
async function sidebarIsPresent(page) {
  return (
    (await page.locator('a:has-text("Dashboards")').count()) > 0 ||
    (await page.locator('text=New chat').count()) > 0
  );
}

async function paywallIsPresent(page) {
  return (
    (await page.locator('text=Your payment failed').count()) > 0 ||
    (await page.locator('text=Your trial has ended').count()) > 0 ||
    (await page.locator('text=Your subscription has ended').count()) > 0
  );
}

async function assertPaywallAtPath(page, path, label) {
  await page.goto(`${BASE_URL}${path}`, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(1500);

  const currentPath = new URL(page.url()).pathname;
  if (currentPath !== path) {
    fail(`${label}: address bar changed from ${path} to ${currentPath} — the paywall must never rewrite the URL`);
  } else {
    ok(`${label}: address bar stayed at ${path}`);
  }

  if (await sidebarIsPresent(page)) {
    fail(`${label}: sidebar/nav is present — a lapsed workspace must see ONLY the paywall`);
  } else {
    ok(`${label}: no sidebar/nav rendered`);
  }

  if (!(await paywallIsPresent(page))) {
    fail(`${label}: paywall copy not found on the page`);
  } else {
    ok(`${label}: paywall copy is present`);
  }
}

// ── Scenarios ────────────────────────────────────────────────────────────────

async function scenarioActiveUser(browser) {
  const page = await (
    await browser.newContext({ viewport: { width: 1920, height: 1080 } })
  ).newPage();
  try {
    await login(page, OWNER_EMAIL, OWNER_PASSWORD);
    await page.goto(`${BASE_URL}/dashboards`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.waitForTimeout(1500);

    if (await paywallIsPresent(page)) {
      fail('active workspace: paywall rendered for an active workspace');
    } else {
      ok('active workspace: no paywall');
    }
    if (!(await sidebarIsPresent(page))) {
      fail('active workspace: sidebar/nav is missing — normal app shell must render');
    } else {
      ok('active workspace: sidebar/nav renders normally');
    }

    await page.screenshot({ path: '/tmp/kyo-806-active-user.png', fullPage: true });
  } finally {
    await page.close();
  }
}

async function scenarioOwnerLapsed(browser) {
  const page = await (
    await browser.newContext({ viewport: { width: 1920, height: 1080 } })
  ).newPage();
  try {
    await login(page, OWNER_EMAIL, OWNER_PASSWORD);

    for (const path of ['/', '/dashboards', '/chat', '/settings/billing']) {
      await assertPaywallAtPath(page, path, `owner-lapsed @ ${path}`);
    }

    // Pay CTA present for the owner — see the module doc for why this is
    // "Subscribe now" rather than "Update payment method" in this seeded
    // environment.
    if ((await page.locator('button:has-text("Subscribe now")').count()) === 0) {
      fail('owner-lapsed: pay CTA ("Subscribe now") not found');
    } else {
      ok('owner-lapsed: pay CTA is present');
    }

    // Log out must still work from the paywall.
    if ((await page.locator('button:has-text("Log out")').count()) === 0) {
      fail('owner-lapsed: Log out control not found on the paywall');
    } else {
      ok('owner-lapsed: Log out control is present');
    }

    await page.screenshot({ path: '/tmp/kyo-806-owner-lapsed.png', fullPage: true });
  } finally {
    await page.close();
  }
}

async function scenarioNonOwnerLapsed(browser) {
  const page = await (
    await browser.newContext({ viewport: { width: 1920, height: 1080 } })
  ).newPage();
  try {
    await login(page, NON_OWNER_EMAIL, NON_OWNER_PASSWORD);
    await assertPaywallAtPath(page, '/', 'non-owner-lapsed @ /');

    if ((await page.locator('text=/Ask .* to update billing/').count()) === 0) {
      fail('non-owner-lapsed: "Ask {owner} to update billing" message not found');
    } else {
      ok('non-owner-lapsed: owner-contact message is present');
    }

    if (
      (await page.locator('button:has-text("Subscribe now")').count()) > 0 ||
      (await page.locator('button:has-text("Update payment method")').count()) > 0
    ) {
      fail('non-owner-lapsed: a pay button is visible — billing is owner-only');
    } else {
      ok('non-owner-lapsed: no pay button visible');
    }

    await page.screenshot({ path: '/tmp/kyo-806-non-owner-lapsed.png', fullPage: true });
  } finally {
    await page.close();
  }
}

/**
 * 402-while-open: load the app while the workspace is active, THEN lapse
 * the workspace out from under the open tab via the direct DB mutation, then
 * click Create Dashboard so its `create_dashboard` server-fn call is the
 * one that should observe the 402 and flip `PaywallAwareClient`'s
 * `report_payment_required`.
 * Asserts the paywall appears WITHOUT a full page reload — i.e. this is not
 * just "the paywall shows on the next full navigation", which
 * `assertPaywallAtPath`'s `page.goto` calls would not distinguish from a
 * plain server-side re-render.
 */
async function scenario402WhileOpen(browser, activeState) {
  const page = await (
    await browser.newContext({ viewport: { width: 1920, height: 1080 } })
  ).newPage();
  try {
    // Workspace must be active for this scenario's setup.
    activateWorkspace(activeState);

    await login(page, OWNER_EMAIL, OWNER_PASSWORD);
    await page.goto(`${BASE_URL}/dashboards`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.waitForTimeout(1500);

    if (await paywallIsPresent(page)) {
      fail('402-while-open: paywall already showing before the workspace was lapsed');
      return;
    }

    const urlBefore = page.url();
    const timeOriginBefore = await page.evaluate(() => performance.timeOrigin);

    lapseWorkspace();

    // Register the response waiter before clicking so a fast server-fn
    // response cannot be missed. This action cannot be served from the
    // client cache as the old Chats navigation could be.
    const responsePromise = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname.endsWith('/create_dashboard') &&
        response.request().method() === 'POST',
      { timeout: 15000 }
    );
    await page.getByRole('button', { name: 'Create Dashboard' }).click();
    const response = await responsePromise;
    if (response.status() !== 402) {
      fail(`402-while-open: create_dashboard returned HTTP ${response.status()}, expected 402`);
    } else {
      ok('402-while-open: create_dashboard returned HTTP 402');
    }

    await page.waitForTimeout(1500);
    const timeOriginAfter = await page.evaluate(() => performance.timeOrigin);
    if (timeOriginAfter !== timeOriginBefore) {
      fail('402-while-open: performance.timeOrigin changed — the document reloaded');
    } else {
      ok('402-while-open: performance.timeOrigin unchanged — no document reload');
    }

    if (page.url() !== urlBefore) {
      fail(`402-while-open: URL changed from ${urlBefore} to ${page.url()}`);
    } else {
      ok('402-while-open: URL stayed at /dashboards');
    }

    if (!(await paywallIsPresent(page))) {
      fail('402-while-open: paywall did not appear after the workspace lapsed mid-session');
    } else {
      ok('402-while-open: paywall appeared without a page reload');
    }

    await page.screenshot({ path: '/tmp/kyo-806-402-while-open.png', fullPage: true });
  } finally {
    await page.close();
  }
}

// ── Main ─────────────────────────────────────────────────────────────────────

(async () => {
  const originalState = captureWorkspaceSubscriptionState();
  let browser;

  try {
    browser = await chromium.launch({ headless: true });
    await scenarioActiveUser(browser);

    lapseWorkspace();
    await scenarioOwnerLapsed(browser);
    await scenarioNonOwnerLapsed(browser);

    await scenario402WhileOpen(browser, originalState);
  } catch (e) {
    fail(`unexpected error: ${e.message.split('\n')[0]}`);
  } finally {
    try {
      if (browser) await browser.close();
    } finally {
      restoreWorkspaceSubscriptionState(originalState);
      console.log(`Restored workspace ${WORKSPACE_ID} to its original subscription state.`);
    }
  }

  if (failed) {
    console.log('\nKYO-806 REGRESSED: the billing paywall is not behaving correctly.');
    process.exit(1);
  }
  console.log('\nKYO-806 OK: the billing paywall behaves correctly for owner, non-owner, active, and mid-session lapse.');
  process.exit(0);
})();
