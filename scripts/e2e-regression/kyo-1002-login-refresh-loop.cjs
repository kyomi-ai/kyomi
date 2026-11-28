// KYO-1002: a visitor with no cookies on /login must not reload the page over and
// over (each load fires the pre-boot `POST /api/v1/auth/refresh` from
// crates/kyomi-ui/index.html).
//
// Cause: in personal mode the server authenticates every request as the local
// user, so the /login page's "already authenticated?" probe succeeded and sent
// the browser to `/`; the server's cookie gate (`check_auth_cookie_or_redirect`
// in apps/server/src/leptos_frontend.rs) then redirected `/` back to `/login`
// because there was no cookie. That ping-pong reloaded the page about every
// 0.6 s, one refresh call per load.
//
// The script opens /login with no cookies for WATCH_SECS (default 30) and
// counts the real requests. It asserts:
//   * the page settles: no main-frame navigation in the last half of the window;
//   * at most one refresh call per main-frame document load;
//   * at most two main-frame document loads in total (/login, then the page it
//     forwards to in personal mode).
//
// Run against a server whose `crates/kyomi-ui/dist` is current, in each mode
// (a fresh empty data dir for personal mode):
//     DATA_DIR=<empty dir> KYOMI_MODE=personal PORT=3202 HOSTNAME=localhost target/dev-server/kyomi
//     NODE_PATH=<repo>/node_modules PORT=3202 node scripts/e2e-regression/kyo-1002-login-refresh-loop.cjs
const { chromium } = require('playwright');

const PORT = process.env.PORT || '3000';
const WATCH_SECS = Number(process.env.WATCH_SECS || 30);
const BASE = `http://localhost:${PORT}`;

(async () => {
  const browser = await chromium.launch({
    headless: true,
    args: ['--use-mock-keychain', '--password-store=basic'],
  });
  const page = await (await browser.newContext()).newPage();
  const t0 = Date.now();
  const refreshCalls = [];
  const documentLoads = [];
  page.on('request', (r) => {
    if (r.method() === 'POST' && r.url() === `${BASE}/api/v1/auth/refresh`) {
      refreshCalls.push(Date.now() - t0);
    }
    if (r.isNavigationRequest() && r.frame() === page.mainFrame()) {
      documentLoads.push({ at: Date.now() - t0, url: r.url() });
    }
  });

  await page.goto(`${BASE}/login`, { waitUntil: 'domcontentloaded' });
  await page.waitForTimeout(WATCH_SECS * 1000);
  await browser.close();

  const lateLoads = documentLoads.filter((d) => d.at > (WATCH_SECS * 1000) / 2);
  console.log(JSON.stringify({
    watchSecs: WATCH_SECS,
    refreshCalls: refreshCalls.length,
    documentLoads: documentLoads.length,
    loads: documentLoads.slice(0, 5),
  }));

  const failures = [];
  if (lateLoads.length > 0) failures.push(`${lateLoads.length} page loads in the second half of the window; the page never settles`);
  if (refreshCalls.length > documentLoads.length) failures.push(`${refreshCalls.length} refresh calls for ${documentLoads.length} page loads`);
  if (documentLoads.length > 2) failures.push(`${documentLoads.length} main-frame document loads, expected at most 2`);
  if (failures.length) {
    console.error('FAIL\n- ' + failures.join('\n- '));
    process.exit(1);
  }
  console.log('PASS');
})();
