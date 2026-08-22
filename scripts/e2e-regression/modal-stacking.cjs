// KYO-434 modal-over-modal stacking regression.
//
// Bug: clicking "Request access" in the BigQuery `kyomi_oauth` notice (shown
// inside the Add Datasource modal) opens the feedback modal *behind* the
// still-open Add Datasource modal. To the user nothing visibly happens — the
// feedback modal only becomes visible after the datasource modal is
// dismissed, by which point the connection between the two is lost.
//
// Root cause: both modals rendered at the same z-[1000] stacking level, so
// paint order fell through to DOM order (`Sidebar`, which owns
// `FeedbackModal`, renders before `<main>`, which owns the datasource
// modal — see `crates/kyomi-ui/src/components/layout.rs`). The fix gives
// `Modal` an explicit `layer` prop (`ModalLayer`); `FeedbackModal` now opts
// into `ModalLayer::Elevated` (`z-[1050]`), which paints above
// `ModalLayer::Base` (`z-[1000]`) and below `Tooltip`'s `z-[1100]`.
//
// IMPORTANT — why this script does NOT use `isVisible()`:
//
// Playwright's `locator.isVisible()` returns `true` for an element that is
// completely painted over by another element — visibility in the
// accessibility-tree sense, not "is this what a real user would see or be
// able to click." An `isVisible()`-based assertion against this exact bug
// PASSED (false green) before this script existed, because the covered
// feedback modal was still "visible" by that definition. This script
// instead asserts **topmost-ness**, two independent ways:
//
//   1. `document.elementFromPoint()` at the feedback panel's own centre —
//      if the topmost element there isn't inside the panel, something is
//      painted over it.
//   2. A real Playwright `click()` on a control inside the panel (its Close
//      button) — Playwright's actionability check fails with "subtree
//      intercepts pointer events" if another element is on top, which is
//      the exact failure mode a real user hits.
//
// Run (default PORT is 3000, the standard local dev-server port):
//     NODE_PATH=/home/jason/repos/kyomi/node_modules \
//         node scripts/e2e-regression/modal-stacking.cjs
//
// Exits 0 when the feedback panel is topmost by both checks, 1 otherwise
// (including when the DOM path to reproduce the scenario can't be found —
// treated as inconclusive-and-failing, not silently skipped).

const { chromium } = require('playwright');

const PORT = process.env.PORT || '3000';
const BASE_URL = process.env.BASE_URL || `http://localhost:${PORT}`;
const SCREENSHOT_PATH = '/tmp/kyo-434-modal-stacking.png';

(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await (
    await browser.newContext({ viewport: { width: 1920, height: 1080 } })
  ).newPage();

  let failed = false;
  const fail = (msg) => {
    failed = true;
    console.log(`FAIL: ${msg}`);
  };

  try {
    // ── Auth + navigate to the scenario ──────────────────────────────────
    await page.goto(`${BASE_URL}/login`, { waitUntil: 'networkidle', timeout: 30000 });
    await page.fill('input[type="email"]', 'e2e-admin@kyomi.dev', { timeout: 8000 });
    await page.fill('input[type="password"]', 'E2eAdminPass123!', { timeout: 8000 });
    await page.click('button[type="submit"]', { timeout: 8000 });
    await page.waitForURL((u) => !u.toString().includes('/login'), { timeout: 20000 });

    await page.goto(`${BASE_URL}/settings/datasources`, {
      waitUntil: 'networkidle',
      timeout: 30000,
    });
    await page.waitForTimeout(3000);

    // Open the Add Datasource modal, then trigger the BigQuery kyomi_oauth
    // "Request access" link — this is the exact user action from the bug
    // report. Both modals are expected to be open simultaneously afterward.
    await page.locator('button:has-text("Add Datasource")').first().click();
    await page.waitForTimeout(1500);
    await page.locator('button:has-text("Request access")').first().click();
    await page.waitForTimeout(1800);

    // ── Locate the feedback panel and mark its Close button ─────────────
    const located = await page.evaluate(() => {
      const titles = [...document.querySelectorAll('*')].filter(
        (e) => e.textContent?.trim() === 'Send Feedback' && e.children.length === 0
      );
      if (!titles.length) {
        return { ok: false, reason: 'feedback modal title not found in DOM' };
      }
      let panel = titles[0];
      while (panel && !(panel.className || '').toString().includes('rounded-lg')) {
        panel = panel.parentElement;
      }
      if (!panel) {
        return { ok: false, reason: 'feedback modal panel not found' };
      }
      const closeBtn = panel.querySelector('button[aria-label="Close"]');
      if (!closeBtn) {
        return { ok: false, reason: 'Close button not found inside the feedback panel' };
      }
      closeBtn.setAttribute('data-kyo434-probe', 'feedback-close');

      const r = panel.getBoundingClientRect();
      const cx = r.left + r.width / 2;
      const cy = r.top + r.height / 2;
      const top = document.elementFromPoint(cx, cy);
      const coveredBy = Boolean(top) && !panel.contains(top) && top !== panel;

      let owner = top;
      let coveringLabel = '';
      while (owner) {
        if ((owner.textContent || '').includes('Add Datasource')) {
          coveringLabel = 'Add Datasource modal';
          break;
        }
        owner = owner.parentElement;
      }

      return {
        ok: true,
        panelRect: { w: Math.round(r.width), h: Math.round(r.height) },
        topmostTag: top ? `${top.tagName}.${String(top.className).slice(0, 40)}` : null,
        coveredBy,
        coveringLabel,
      };
    });

    if (!located.ok) {
      fail(`could not reach the modal-stacking scenario: ${located.reason}`);
    } else {
      console.log(JSON.stringify(located, null, 2));

      // Check 1 — document.elementFromPoint() at the panel's own centre.
      if (located.coveredBy) {
        fail(
          `feedback panel is covered at its own centre by ${
            located.coveringLabel || located.topmostTag || 'another element'
          } (elementFromPoint check)`
        );
      } else {
        console.log('PASS: feedback panel is topmost at its own centre (elementFromPoint check).');
      }

      // Check 2 — a real click through Playwright's actionability/hit-test
      // pipeline. This is the check that models an actual user click and
      // fails the way a real click does ("subtree intercepts pointer
      // events") rather than merely inspecting computed styles.
      try {
        await page.locator('[data-kyo434-probe="feedback-close"]').click({ timeout: 4000 });
        console.log('PASS: a real click on the feedback panel\'s Close button was not intercepted.');
      } catch (e) {
        const message = e.message.split('\n')[0];
        fail(`click on the feedback panel's Close button was intercepted: ${message}`);
      }
    }

    await page.screenshot({ path: SCREENSHOT_PATH, fullPage: true });
    console.log(`Screenshot saved to ${SCREENSHOT_PATH}`);
  } catch (e) {
    fail(`unexpected error: ${e.message.split('\n')[0]}`);
  } finally {
    await browser.close();
  }

  if (failed) {
    console.log('\nKYO-434 REGRESSED: the feedback modal is not reliably topmost over the Add Datasource modal.');
    process.exit(1);
  }
  console.log('\nKYO-434 OK: the feedback modal opens on top of the Add Datasource modal.');
  process.exit(0);
})();
