// KYO-800 viewer regression: WebSocket-driven dashboard refetches must keep
// Copilot mounted, preserve a valid filter and scroll position, and reset a
// filter whose option was removed. No LLM call is made.
//
// Run against a built server with the seeded e2e user:
//   NODE_PATH=/home/jason/repos/kyomi/node_modules \
//     E2E_BASE_URL=http://localhost:3000 \
//     node scripts/e2e-regression/kyo-800-viewer-copilot.cjs
// Optional: E2E_TEST_EMAIL, E2E_TEST_PASSWORD.

const assert = require('node:assert/strict');
const { chromium } = require('playwright');

const base = process.env.E2E_BASE_URL || 'http://localhost:3000';
const email = process.env.E2E_TEST_EMAIL || 'e2e-test@kyomi.dev';
const password = process.env.E2E_TEST_PASSWORD || 'E2eTestPass123!';
const suffix = `${Date.now()}-${Math.floor(Math.random() * 1e6)}`;

function content(revision, options) {
  // Use the same ChartML params shape exercised by dashboard_viewer's unit
  // tests. Extra prose makes the viewer's own content container scrollable.
  const parameters = [
    '```chartml',
    'type: params',
    'version: 1',
    'params:',
    '  - id: region',
    '    type: select',
    '    label: Region',
    '    default: East',
    `    options: [${options.join(', ')}]`,
    '```',
  ].join('\n');
  return [
    parameters,
    `# KYO-800 revision ${revision} ${suffix}`,
    ...Array.from({ length: 35 }, (_, i) => `Paragraph ${i + 1}: viewer scroll regression content for ${suffix}.`),
  ].join('\n\n');
}

async function api(cookie, path, method, body) {
  const response = await fetch(`${base}${path}`, {
    method,
    headers: { cookie, 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!response.ok) {
    throw new Error(`${method} ${path}: ${response.status} ${(await response.text()).slice(0, 500)}`);
  }
  const text = await response.text();
  if (!text) return null;
  try { return JSON.parse(text); } catch { return text; }
}

async function waitForFrame(frames, id) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    if (frames.some((frame) => frame.type === 'dashboard_update'
      && frame.data?.dashboard_id === id && frame.data?.action === 'updated')) return;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`dashboard_update frame did not reach viewer for ${id}`);
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  const frames = [];
  let websocketOpened = false;
  let dashboardId;
  let cookie;
  try {
    page.on('websocket', (ws) => {
      websocketOpened = true;
      ws.on('framereceived', ({ payload }) => {
        try { frames.push(JSON.parse(payload.toString())); } catch { /* non-JSON frame */ }
      });
    });

    await page.goto(`${base}/login`, { waitUntil: 'domcontentloaded' });
    await page.locator('input[type="email"]').fill(email);
    await page.locator('input[type="password"]').fill(password);
    await page.locator('button[type="submit"]').click();
    await page.waitForURL((url) => !url.pathname.includes('/login'));
    cookie = (await context.cookies(base)).map(({ name, value }) => `${name}=${value}`).join('; ');

    const created = await api(cookie, '/api/v1/dashboards', 'POST', {
      title: `KYO-800 viewer fixture ${suffix}`,
      content: content(1, ['East', 'West']),
    });
    dashboardId = created.dashboard_id;
    assert.ok(dashboardId, 'dashboard REST create returned an id');

    await page.goto(`${base}/dashboard/${dashboardId}`, { waitUntil: 'domcontentloaded' });
    const body = page.locator('.dashboard-content');
    await body.getByText(`KYO-800 revision 1 ${suffix}`).waitFor();
    await page.getByRole('button', { name: 'Toggle Copilot' }).click();
    const draft = page.locator('textarea[placeholder="Ask about this data or request a dashboard change…"]');
    await draft.waitFor();
    await draft.fill('Unsent question kept across dashboard updates');
    await draft.evaluate((node) => { window.__kyo800DraftNode = node; });

    const filter = page.locator('.dashboard-filters');
    await filter.getByRole('button').click();
    await page.getByRole('option', { name: 'West' }).click();
    await filter.getByRole('button', { name: 'West' }).waitFor();
    await page.getByText('Active filters: region: West').waitFor();

    const scroller = page.locator('.dashboard-content')
      .locator('xpath=ancestor::div[contains(@class, "overflow-y-auto")][1]');
    const beforeScroll = await scroller.evaluate((node) => {
      node.scrollTop = 400;
      return node.scrollTop;
    });
    assert.ok(beforeScroll > 0, 'viewer content must be scrollable');
    assert.ok(websocketOpened, 'viewer must have an open WebSocket before mutation');

    for (const [revision, options, expectedFilter] of [
      [2, ['East', 'West'], 'West'],
      [3, ['East', 'North'], 'East'],
      [4, ['East', 'North'], 'East'],
    ]) {
      frames.length = 0;
      await api(cookie, `/api/v1/dashboards/${dashboardId}`, 'PATCH', {
        content: content(revision, options),
      });
      await waitForFrame(frames, dashboardId);
      await body.getByText(`KYO-800 revision ${revision} ${suffix}`).waitFor();
      await filter.getByRole('button', { name: expectedFilter }).waitFor();
      await page.getByText(`Active filters: region: ${expectedFilter}`).waitFor();
      assert.equal(await draft.inputValue(), 'Unsent question kept across dashboard updates');
      assert.ok(await draft.evaluate((node) => window.__kyo800DraftNode === node),
        `Copilot input remounted at revision ${revision}`);
      const afterScroll = await scroller.evaluate((node) => node.scrollTop);
      assert.ok(Math.abs(afterScroll - beforeScroll) <= 10,
        `viewer scroll moved from ${beforeScroll} to ${afterScroll} at revision ${revision}`);
    }
    console.log('PASS KYO-800: viewer updates, Copilot draft/mount, filters, and scroll');
  } finally {
    if (dashboardId && cookie) {
      try { await api(cookie, `/api/v1/dashboards/${dashboardId}`, 'DELETE'); }
      catch (error) { console.error(`fixture cleanup failed: ${error.message}`); }
    }
    await context.close();
    await browser.close();
  }
})().catch((error) => { console.error(error); process.exitCode = 1; });
