// KYO-807 billing-status-over-WebSocket regression.
//
// When a server-side write changes a workspace's billing state, every open
// tab for that workspace must lock/unlock the paywall WITHOUT a reload —
// `kyomi_auth::websocket::helpers::broadcast_billing_status_changed` pushes
// a `billing_status_changed` event to the workspace over the already-open
// WebSocket, and `crates/kyomi-ui/src/cache/sync_engine.rs`'s subscription
// calls `refetch_billing_state()` on receipt. This script drives that path
// end to end with two open tabs and REAL, signed Stripe webhook POSTs (not
// a direct DB mutation, unlike `billing-paywall.cjs` KYO-806) — the write
// site under test IS the webhook handler
// (`apps/server/src/routes/billing.rs` -> `kyomi_auth::billing_webhook`),
// so a direct DB UPDATE would bypass the exact code path this ticket added
// the broadcast to.
//
// ## Why signed webhooks, not a UI flow
//
// There is no UI path to fail/recover a payment against a real Stripe
// subscription in a local dev environment, and driving Stripe's own hosted
// checkout/portal iframe is out of scope for a script (see
// `billing-paywall.cjs`'s header for the same reasoning). Instead this
// script POSTs correctly HMAC-signed `invoice.payment_failed` (lapse) and
// `customer.subscription.updated` (unlock) events directly to
// `/api/v1/billing/webhook`, exactly as Stripe itself would. The signature
// scheme is `stripe_webhook`'s own (`async-stripe-webhook` crate,
// `Webhook::construct_event`):
//
//     signed_payload = "<unix_timestamp>.<raw_json_payload_bytes>"
//     v1 = hex(HMAC-SHA256(STRIPE_WEBHOOK_SECRET, signed_payload))
//     header: "Stripe-Signature: t=<unix_timestamp>,v1=<v1>"
//
// (see `crates/kyomi-auth/src/stripe_service.rs`'s `construct_webhook_event`
// and, upstream, `async-stripe-webhook-*/src/webhook.rs`'s
// `do_construct_event`). The signature is rejected if the timestamp is more
// than 5 minutes old, so it's generated fresh immediately before each POST.
//
// ## Fixtures
//
// `stripe-fixtures/invoice-payment-failed.json` and
// `stripe-fixtures/subscription-updated.json` are the `data.object` payloads
// for the two events — hand-kept **byte-for-byte identical in shape** to the
// fixtures `crates/kyomi-auth/src/billing_webhook.rs`'s own tests parse
// (`tests::invoice_fixture` / `tests::subscription_fixture`), so they are
// proven to deserialize as real `stripe_shared::Invoice`/`Subscription`
// values by those Rust tests, not just "look plausible" JSON. `__TOKEN__`
// placeholders are substituted at request time (see `loadFixture` below); if
// you touch either Rust fixture, mirror the change here (and vice versa) —
// nothing enforces the two staying in sync automatically.
//
// `invoice-payment-failed.json`'s "subscription" field is set to a fake
// `stripe_subscription_id` this script writes onto the workspace row via a
// direct DB UPDATE first (there is no other way to give the seeded
// e2e workspace a `stripe_subscription_id` to match against — it never had
// a real Stripe subscription). This one direct-DB step is unavoidable setup,
// not the mutation under test — the mutation under test is entirely the two
// signed webhook POSTs.
//
// ## Scenario
//
// 1. Two tabs (two browser contexts, same owner login) open on the active
//    e2e workspace, each showing the normal app shell (no paywall).
// 2. POST a signed `invoice.payment_failed` event for the fake subscription
//    id. Assert BOTH tabs show the paywall, with NO navigation (checked via
//    `performance.timeOrigin` staying identical before/after — a full
//    reload resets it, an in-page WebSocket-driven update does not).
// 3. POST a signed `customer.subscription.updated` event (status `active`,
//    `metadata.app = "kyomi"`, `metadata.workspace_id` = the e2e workspace)
//    for the same fake subscription id. Assert BOTH tabs return to the
//    normal app shell, again with no navigation.
// 4. Restore the workspace's original subscription state in a `finally`
//    block regardless of pass/fail/throw — same pattern as
//    `billing-paywall.cjs`.
//
// ## Prerequisites
//
// - A SaaS-mode dev server (never `SELF_HOSTED=true` — see
//   `.claude/CLAUDE.md`'s "Local Dev Server" section) with `STRIPE_SECRET_KEY`
//   and `STRIPE_WEBHOOK_SECRET` configured in its environment — the webhook
//   handler 400s every request if `state.stripe` is `None`
//   (`apps/server/src/routes/billing.rs`'s `require_stripe`), and signature
//   verification obviously needs the real secret the server was started
//   with. `STRIPE_WEBHOOK_SECRET` must also be exported to THIS script's
//   environment (same value) so it can sign requests the server will accept.
// - `seed-test-user.py` already run (seeds `e2e-test-workspace-0001` and its
//   owner/member).
// - This script has NOT been run — no built server was available when it was
//   written (KYO-807). `node --check` is the only verification performed on
//   it; treat first execution as a real test of both the script and the
//   feature.
//
// Run (default PORT is 3000, the standard local dev-server port):
//     NODE_PATH=/home/jason/repos/kyomi/node_modules \
//         STRIPE_WEBHOOK_SECRET=whsec_... \
//         node scripts/e2e-regression/billing-live-status.cjs
//
// Overrides (all optional):
//   E2E_BASE_URL          - app base URL (default http://localhost:3000, or
//                            http://localhost:$PORT if PORT is set)
//   DATABASE_URL           - Postgres connection string (default matches
//                            seed-test-user.py's default:
//                            postgresql://kyomi:password@localhost:5433/kyomi)
//   STRIPE_WEBHOOK_SECRET  - REQUIRED. Must match the running server's own
//                            STRIPE_WEBHOOK_SECRET.
//
// Exits 0 when every scenario passes, 1 otherwise. Screenshots (full page,
// 1920x1080) saved to /tmp/kyo-807-*.png for each scenario regardless of
// pass/fail, for visual review.

const { chromium } = require('playwright');
const { execFileSync } = require('child_process');
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const BASE_URL = process.env.E2E_BASE_URL || `http://localhost:${process.env.PORT || '3000'}`;
const DATABASE_URL =
  process.env.DATABASE_URL || 'postgresql://kyomi:password@localhost:5433/kyomi';
const STRIPE_WEBHOOK_SECRET = process.env.STRIPE_WEBHOOK_SECRET;

const WORKSPACE_ID = 'e2e-test-workspace-0001';
const OWNER_EMAIL = 'e2e-admin@kyomi.dev';
const OWNER_PASSWORD = 'E2eAdminPass123!';

// A subscription id that has never existed at Stripe — fine, since these
// webhook POSTs never cause the server to call Stripe's API. Only the
// `stripe_subscription_id` column needs to match what the invoice event
// references, so `apply_invoice_payment_failed_webhook_event`'s workspace
// lookup finds this workspace.
const FAKE_SUBSCRIPTION_ID = 'sub_kyo807_e2e_fake';

let failed = false;
const fail = (msg) => {
  failed = true;
  console.log(`FAIL: ${msg}`);
};
const ok = (msg) => console.log(`PASS: ${msg}`);

// ── Fixture loading + Stripe webhook signing ────────────────────────────────

function loadFixture(filename, tokens) {
  const raw = fs.readFileSync(
    path.join(__dirname, 'stripe-fixtures', filename),
    'utf8'
  );
  let substituted = raw;
  for (const [token, value] of Object.entries(tokens)) {
    // Split/join avoids needing a regex-escaped global replace for tokens
    // that are plain `__TOKEN__` strings with no regex metacharacters.
    substituted = substituted.split(token).join(value);
  }
  // Fail loudly (not silently) if a placeholder from the fixture file was
  // never substituted — a stale/renamed token here must not send Stripe's
  // literal `__WORKSPACE_ID__` string to the server as if it were real.
  const leftover = substituted.match(/__[A-Z_]+__/);
  if (leftover) {
    throw new Error(`${filename}: unsubstituted placeholder ${leftover[0]}`);
  }
  // Round-trip through JSON.parse/stringify: proves the substituted fixture
  // is still valid JSON (a token collision could have broken it) and gives
  // us the exact byte string this script signs and sends — the same
  // "sign literally what you send" requirement the Rust side's
  // `Webhook::do_construct_event` enforces (it HMACs the raw request body).
  return JSON.stringify(JSON.parse(substituted));
}

function buildWebhookEnvelope(eventType, dataObjectJson) {
  const dataObject = JSON.parse(dataObjectJson);
  const envelope = {
    id: `evt_kyo807_${crypto.randomBytes(8).toString('hex')}`,
    object: 'event',
    api_version: null,
    created: Math.floor(Date.now() / 1000),
    livemode: false,
    pending_webhooks: 1,
    request: null,
    data: { object: dataObject },
    type: eventType,
  };
  return JSON.stringify(envelope);
}

/** `Webhook::construct_event`'s exact signature scheme — see header comment. */
function signStripePayload(payload, secret) {
  const timestamp = Math.floor(Date.now() / 1000);
  const signedPayload = `${timestamp}.${payload}`;
  const v1 = crypto.createHmac('sha256', secret).update(signedPayload).digest('hex');
  return `t=${timestamp},v1=${v1}`;
}

async function postSignedWebhook(eventType, dataObjectJson) {
  if (!STRIPE_WEBHOOK_SECRET) {
    throw new Error(
      'STRIPE_WEBHOOK_SECRET is not set — required to sign webhook requests the server will accept'
    );
  }
  const payload = buildWebhookEnvelope(eventType, dataObjectJson);
  const signature = signStripePayload(payload, STRIPE_WEBHOOK_SECRET);

  const res = await fetch(`${BASE_URL}/api/v1/billing/webhook`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Stripe-Signature': signature,
    },
    body: payload,
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '<unreadable>');
    throw new Error(`webhook POST for ${eventType} returned ${res.status}: ${body}`);
  }
}

// ── Direct DB setup/restore (fake stripe_subscription_id + cleanup) ────────

function psql(sql) {
  return execFileSync('psql', [DATABASE_URL, '-t', '-A', '-c', sql], {
    encoding: 'utf8',
  }).trim();
}

function captureWorkspaceState() {
  const row = psql(
    `SELECT subscription_status, subscription_tier, COALESCE(billing_cycle, ''), ` +
      `COALESCE(stripe_subscription_id, ''), COALESCE(stripe_customer_id, ''), ` +
      `COALESCE(subscription_period_start::text, ''), COALESCE(subscription_period_end::text, ''), ` +
      `user_limit, ai_credits_used_usd ` +
      `FROM workspaces WHERE workspace_id = '${WORKSPACE_ID}'`
  );
  const [
    status,
    tier,
    billingCycle,
    subId,
    custId,
    periodStart,
    periodEnd,
    userLimit,
    aiCredits,
  ] = row.split('|');
  return { status, tier, billingCycle, subId, custId, periodStart, periodEnd, userLimit, aiCredits };
}

function restoreWorkspaceState(state) {
  psql(
    `UPDATE workspaces SET ` +
      `subscription_status = '${state.status}', ` +
      `subscription_tier = '${state.tier}', ` +
      `billing_cycle = ${state.billingCycle ? `'${state.billingCycle}'` : 'NULL'}, ` +
      `stripe_subscription_id = ${state.subId ? `'${state.subId}'` : 'NULL'}, ` +
      `stripe_customer_id = ${state.custId ? `'${state.custId}'` : 'NULL'}, ` +
      `subscription_period_start = ${state.periodStart ? `'${state.periodStart}'` : 'NULL'}, ` +
      `subscription_period_end = ${state.periodEnd ? `'${state.periodEnd}'` : 'NULL'}, ` +
      `user_limit = ${state.userLimit}, ` +
      `ai_credits_used_usd = ${state.aiCredits} ` +
      `WHERE workspace_id = '${WORKSPACE_ID}'`
  );
}

/** Give the seeded workspace a `stripe_subscription_id` to match the
 * webhook fixtures against — see the header comment for why this one direct
 * mutation is unavoidable setup rather than the thing under test. */
function seedFakeSubscriptionId() {
  psql(
    `UPDATE workspaces SET stripe_subscription_id = '${FAKE_SUBSCRIPTION_ID}' ` +
      `WHERE workspace_id = '${WORKSPACE_ID}'`
  );
}

// ── Playwright helpers ──────────────────────────────────────────────────────

async function login(page, email, password) {
  await page.goto(`${BASE_URL}/login`, { waitUntil: 'networkidle', timeout: 30000 });
  await page.fill('input[type="email"]', email, { timeout: 8000 });
  await page.fill('input[type="password"]', password, { timeout: 8000 });
  await page.click('button[type="submit"]', { timeout: 8000 });
  await page.waitForURL((u) => !u.toString().includes('/login'), { timeout: 20000 });
}

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

/** `performance.timeOrigin` is fixed at the moment the current document was
 * created — a full page reload/navigation creates a new document and a new
 * (different) timeOrigin; an in-page WebSocket-driven refetch does not. */
async function timeOrigin(page) {
  return page.evaluate(() => performance.timeOrigin);
}

async function assertNoNavigation(page, before, label) {
  const after = await timeOrigin(page);
  if (after !== before) {
    fail(`${label}: performance.timeOrigin changed (${before} -> ${after}) — a reload/navigation occurred`);
  } else {
    ok(`${label}: no navigation occurred (performance.timeOrigin unchanged)`);
  }
}

// ── Main scenario ────────────────────────────────────────────────────────────

(async () => {
  const originalState = captureWorkspaceState();
  const browser = await chromium.launch({ headless: true });

  // Two independent tabs (contexts), both logged in as the workspace owner,
  // both already showing the normal app shell before anything is lapsed.
  const contextA = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const contextB = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const tabA = await contextA.newPage();
  const tabB = await contextB.newPage();

  try {
    await login(tabA, OWNER_EMAIL, OWNER_PASSWORD);
    await login(tabB, OWNER_EMAIL, OWNER_PASSWORD);

    await tabA.goto(`${BASE_URL}/dashboards`, { waitUntil: 'networkidle', timeout: 30000 });
    await tabB.goto(`${BASE_URL}/dashboards`, { waitUntil: 'networkidle', timeout: 30000 });
    await tabA.waitForTimeout(1500);
    await tabB.waitForTimeout(1500);

    if ((await paywallIsPresent(tabA)) || (await paywallIsPresent(tabB))) {
      fail('setup: paywall already showing before the workspace was lapsed');
      return;
    }
    if (!(await sidebarIsPresent(tabA)) || !(await sidebarIsPresent(tabB))) {
      fail('setup: sidebar/nav missing before the workspace was lapsed');
      return;
    }
    ok('setup: both tabs show the normal app shell, no paywall');

    const timeOriginABeforeLapse = await timeOrigin(tabA);
    const timeOriginBBeforeLapse = await timeOrigin(tabB);

    // ── Lapse: POST a signed invoice.payment_failed event ──────────────────
    seedFakeSubscriptionId();
    const invoicePayload = loadFixture('invoice-payment-failed.json', {
      __INVOICE_ID__: `in_kyo807_${crypto.randomBytes(6).toString('hex')}`,
      __SUBSCRIPTION_ID__: FAKE_SUBSCRIPTION_ID,
    });
    await postSignedWebhook('invoice.payment_failed', invoicePayload);
    ok('lapse: signed invoice.payment_failed POSTed and accepted (2xx)');

    // Give the broadcast + client refetch + re-render time to land.
    await tabA.waitForTimeout(3000);
    await tabB.waitForTimeout(3000);

    if (!(await paywallIsPresent(tabA))) {
      fail('lapse: tab A did not show the paywall after invoice.payment_failed');
    } else {
      ok('lapse: tab A shows the paywall');
    }
    if (!(await paywallIsPresent(tabB))) {
      fail('lapse: tab B did not show the paywall after invoice.payment_failed');
    } else {
      ok('lapse: tab B shows the paywall');
    }
    await assertNoNavigation(tabA, timeOriginABeforeLapse, 'lapse (tab A)');
    await assertNoNavigation(tabB, timeOriginBBeforeLapse, 'lapse (tab B)');

    await tabA.screenshot({ path: '/tmp/kyo-807-lapsed-tab-a.png', fullPage: true });
    await tabB.screenshot({ path: '/tmp/kyo-807-lapsed-tab-b.png', fullPage: true });

    const timeOriginABeforeUnlock = await timeOrigin(tabA);
    const timeOriginBBeforeUnlock = await timeOrigin(tabB);

    // ── Unlock: POST a signed customer.subscription.updated event ──────────
    const subscriptionPayload = loadFixture('subscription-updated.json', {
      __SUBSCRIPTION_ID__: FAKE_SUBSCRIPTION_ID,
      __WORKSPACE_ID__: WORKSPACE_ID,
    });
    await postSignedWebhook('customer.subscription.updated', subscriptionPayload);
    ok('unlock: signed customer.subscription.updated POSTed and accepted (2xx)');

    await tabA.waitForTimeout(3000);
    await tabB.waitForTimeout(3000);

    if (await paywallIsPresent(tabA)) {
      fail('unlock: tab A still shows the paywall after customer.subscription.updated');
    } else {
      ok('unlock: tab A paywall cleared');
    }
    if (await paywallIsPresent(tabB)) {
      fail('unlock: tab B still shows the paywall after customer.subscription.updated');
    } else {
      ok('unlock: tab B paywall cleared');
    }
    if (!(await sidebarIsPresent(tabA)) || !(await sidebarIsPresent(tabB))) {
      fail('unlock: sidebar/nav did not return after unlocking');
    } else {
      ok('unlock: normal app shell (sidebar/nav) returned in both tabs');
    }
    await assertNoNavigation(tabA, timeOriginABeforeUnlock, 'unlock (tab A)');
    await assertNoNavigation(tabB, timeOriginBBeforeUnlock, 'unlock (tab B)');

    await tabA.screenshot({ path: '/tmp/kyo-807-unlocked-tab-a.png', fullPage: true });
    await tabB.screenshot({ path: '/tmp/kyo-807-unlocked-tab-b.png', fullPage: true });
  } catch (e) {
    fail(`unexpected error: ${e.message.split('\n')[0]}`);
  } finally {
    await tabA.close().catch(() => {});
    await tabB.close().catch(() => {});
    await browser.close();
    restoreWorkspaceState(originalState);
    console.log(`Restored workspace ${WORKSPACE_ID} to its original subscription state.`);
  }

  if (failed) {
    console.log('\nKYO-807 REGRESSED: billing-status-over-WebSocket is not behaving correctly.');
    process.exit(1);
  }
  console.log(
    '\nKYO-807 OK: both open tabs locked and unlocked the paywall over an already-open WebSocket, with no reload.'
  );
  process.exit(0);
})();
