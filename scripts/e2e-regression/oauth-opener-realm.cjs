/**
 * KYO-436 regression — a popup must be able to resolve and message its opener.
 *
 * The defect: both OAuth callback pages resolved `window.opener` with
 * `dyn_into::<web_sys::Window>()`, which compiles to `instanceof Window`
 * against the *popup's own* realm. `window.opener` is a WindowProxy from a
 * different realm, so the check is always false — the opener resolved to
 * None, no postMessage was ever sent, and every provider's connect button
 * hung on "Connecting..." forever.
 *
 * This asserts the browser behaviour the Rust source guards in
 * crates/kyomi-ui/src/utils/oauth_popup.rs are protecting against.
 *
 * Usage: NODE_PATH=<repo>/node_modules node scripts/e2e-regression/oauth-opener-realm.cjs
 */
const { chromium } = require('playwright');

const BASE = process.env.KYOMI_BASE_URL || 'http://localhost:3000';
const results = [];
const check = (name, pass, detail) => {
  results.push({ name, pass: !!pass });
  console.log(`${pass ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
};

(async () => {
  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext();
  const page = await ctx.newPage();

  try {
    await page.goto(`${BASE}/login`, { waitUntil: 'domcontentloaded', timeout: 30000 });

    const [popup] = await Promise.all([
      ctx.waitForEvent('page'),
      page.evaluate(() => { window.open('/login', 'kyo436-oauth', 'width=500,height=600'); }),
    ]);
    await popup.waitForLoadState('domcontentloaded');

    const probe = await popup.evaluate(() => ({
      openerExists:     window.opener != null,
      openerType:       Object.prototype.toString.call(window.opener),
      canPostMessage:   typeof window.opener?.postMessage === 'function',
      instanceofWindow: (() => {
        try { return window.opener instanceof Window; } catch { return 'threw'; }
      })(),
      selfInstanceof:   window instanceof Window,
    }));

    check('popup has a live opener', probe.openerExists, probe.openerType);
    check('opener.postMessage is callable', probe.canPostMessage);
    check('sanity: own window IS instanceof Window (same realm)', probe.selfInstanceof === true);

    // The crux. If this ever becomes true, the realm boundary has changed and
    // the dyn_into cast would start working — but the code must not depend on it.
    check(
      'opener is NOT instanceof Window (cross-realm) — why dyn_into cannot be used',
      probe.instanceofWindow === false,
      `instanceof=${probe.instanceofWindow}`,
    );

    // End-to-end: the popup can actually deliver a message to its opener the
    // way the callback pages do, addressed to its own origin.
    await page.evaluate(() => {
      window.__kyo436 = null;
      window.addEventListener('message', e => { window.__kyo436 = e.data; });
    });
    await popup.evaluate(() => {
      window.opener.postMessage({ type: 'KYO436_PROBE' }, window.location.origin);
    });
    await page.waitForTimeout(500);
    const received = await page.evaluate(() => window.__kyo436);
    check('opener receives a same-origin postMessage from the popup',
      received && received.type === 'KYO436_PROBE', JSON.stringify(received));

  } catch (e) {
    check('script completed without throwing', false, e.message.split('\n')[0]);
  } finally {
    const failed = results.filter(r => !r.pass);
    console.log(`\n===== ${results.length - failed.length}/${results.length} passed =====`);
    await browser.close();
    process.exit(failed.length ? 1 : 0);
  }
})();
