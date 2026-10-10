// KYO-495: browser regression for route-independent chat run state.
// Uses the real authenticated app, router, WebSocket dispatcher and rendering.
// Session history and incoming generation frames are controlled so a DB reload
// cannot conceal dropped frames. No LLM call or production data is required.
// Run against a freshly built SaaS dev server:
// PORT=3595 NODE_PATH=/home/jason/repos/kyomi/node_modules node scripts/e2e-regression/chat-run-navigation.cjs
const { chromium, expect } = require('playwright/test');
const fs = require('fs');
const BASE = process.env.E2E_BASE_URL || `http://localhost:${process.env.PORT || '3595'}`;
const A = '49500000-0000-4000-8000-000000000001';
const B = '49500000-0000-4000-8000-000000000002';
const C = '49500000-0000-4000-8000-000000000003';
const screenshots = [];
const loads = new Map();

function message(id, role, content, status = 'complete') {
  return { message_id: id, message_type: role, content, status, timestamp: new Date().toISOString(),
    pinned: false, sent_by: null, thinking_events: [], token_usage: null };
}
function history(sid) {
  const label = sid === A ? 'Alpha' : 'Beta';
  return { messages: [message(`question-${sid}`, 'user', `KYO-495 ${label} navigation test`),
    message(`answer-${sid}`, 'assistant', `${label} before `, 'in_progress')],
    session: { title: `KYO-495 ${label}`, shared: false, created_by: null, slack_channel_id: null } };
}
async function navigate(page, path) {
  // Click a real anchor so Leptos performs SPA navigation (not page.goto).
  await page.evaluate(path => {
    const link = document.createElement('a');
    link.href = path;
    document.querySelector('main').append(link);
    link.click();
    link.remove();
  }, path);
  await expect(page).toHaveURL(`${BASE}${path}`);
}
async function shot(page, name) {
  const path = `/tmp/kyo-495-${name}.png`;
  await page.screenshot({ path, fullPage: true });
  screenshots.push(path);
}
async function flushRendering(page) {
  await page.evaluate(() => new Promise(resolve =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  const authPath = process.env.E2E_STORAGE_STATE;
  const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 },
    ...(authPath && fs.existsSync(authPath) ? { storageState: authPath } : {}) });
  const page = await ctx.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(error.message));
  let socket;
  let socketCount = 0;
  let failAlphaHistory = false;
  let failedHistoryLoads = 0;
  let delayedPrompt;
  let delayedClientId;
  let pendingSend;
  const rejectedSessions = [];
  await page.route(/\/leptos-api\/send_chat_message[^/?]*(?:\?.*)?$/, async route => {
    const params = new URLSearchParams(route.request().postData());
    const sid = params.get('session_id');
    if (sid === C) {
      delayedPrompt = params.get('message');
      delayedClientId = params.get('client_msg_id');
      await new Promise(resolve => {
        pendingSend = async () => {
          await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({
            session_id: C, user_message_id: 'persisted-delayed-user', assistant_message_id: `answer-${C}`,
            status: 'processing', thinking_events: [], token_usage: null, skip_ai: false,
          }) });
          resolve();
        };
      });
    } else if (params.get('is_new_session') === 'true') {
      rejectedSessions.push(sid);
      await route.fulfill({ status: 500, contentType: 'text/plain',
        body: 'ServerError|Controlled new-chat rejection' });
    } else throw new Error(`Unexpected real send attempted for ${sid}`);
  });
  await page.routeWebSocket(/.*/, route => {
    socket = route;
    socketCount++;
    const server = route.connectToServer();
    route.onMessage(raw => {
      const msg = JSON.parse(raw);
      if (msg.type === 'cancel_request' && [A, B].includes(msg.session_id)) {
        route.send(JSON.stringify({ type: 'request_cancelled', session_id: msg.session_id,
          message_id: `answer-${msg.session_id}`, timestamp: new Date().toISOString(), data: {} }));
      } else server.send(raw);
    });
  });
  await page.route(/\/leptos-api\/get_session_messages[^/?]*(?:\?.*)?$/, async route => {
    const params = new URLSearchParams(route.request().postData() || new URL(route.request().url()).search);
    const sid = params.get('session_id');
    if (sid === C) {
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({
        messages: delayedPrompt ? [message('persisted-delayed-user', 'user', delayedPrompt),
          message(`answer-${C}`, 'assistant', 'Delayed answer ', 'in_progress')] : [],
        session: { title: 'KYO-495 delayed send', shared: false, created_by: null, slack_channel_id: null },
      }) });
    }
    if (![A, B].includes(sid)) return route.continue();
    loads.set(sid, (loads.get(sid) || 0) + 1);
    if (sid === A && failAlphaHistory) {
      failedHistoryLoads++;
      return route.fulfill({ status: 500, body: 'Simulated history fetch failure' });
    }
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(history(sid)) });
  });
  const dashboardId = 'kyo-495-browser-fixture';
  let copilotSequence = 0;
  const deletedCopilots = [];
  await page.route(/\/leptos-api\/get_dashboard\d*(?:\?.*)?$/, async route => {
    const params = new URLSearchParams(route.request().postData() || '');
    if (params.get('dashboard_id') !== dashboardId) return route.continue();
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({
      dashboard_id: dashboardId, user_id: 'test-user', workspace_id: 'e2e-test-workspace-0001',
      title: 'KYO-495 Copilot fixture', content: '# Copilot lifecycle test', summary: null,
      created_at: new Date().toISOString(), updated_at: new Date().toISOString(), last_change_summary: null,
    }) });
  });
  await page.route(/\/leptos-api\/update_dashboard\d*(?:\?.*)?$/, async route => {
    const params = route.request().postDataJSON();
    if (params.dashboard_id !== dashboardId) return route.continue();
    await route.fulfill({ status: 200, contentType: 'application/json', body: 'null' });
  });
  await page.route(/\/leptos-api\/create_copilot_session\d*(?:\?.*)?$/, async route => {
    copilotSequence++;
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(`copilot-${copilotSequence}`) });
  });
  await page.route(/\/leptos-api\/delete_copilot_session\d*(?:\?.*)?$/, async route => {
    deletedCopilots.push(new URLSearchParams(route.request().postData()).get('session_id'));
    await route.fulfill({ status: 200, contentType: 'application/json', body: 'null' });
  });
  const emit = (sid, content, offset, type = 'chat_stream') => socket.send(JSON.stringify({
    type, session_id: sid, message_id: `answer-${sid}`, timestamp: new Date().toISOString(),
    data: { content, content_offset: offset, full_content: content, context_type: 'chat' },
  }));
  try {
    await page.goto(`${BASE}/login`, { waitUntil: 'domcontentloaded', timeout: 60000 });
    // SSR controls are visible before WASM attaches their event handlers.
    // Wait for hydration itself rather than unrelated background requests.
    await page.waitForFunction(() => !document.body.hasAttribute('data-ssr'), undefined,
      { timeout: 120000 });
    if (new URL(page.url()).pathname === '/login') {
      await page.locator('input[type="email"]').fill(process.env.E2E_TEST_EMAIL || 'e2e-test@kyomi.dev', { timeout: 30000 });
      await page.locator('input[type="password"]').fill(process.env.E2E_TEST_PASSWORD || 'E2eTestPass123!');
      await page.locator('button[type="submit"]').click();
    }
    await expect(page).not.toHaveURL(/\/login/, { timeout: 30000 });
    if (authPath) {
      fs.writeFileSync(authPath, JSON.stringify(await ctx.storageState()), { mode: 0o600 });
    }
    await expect(page.locator('main')).toBeVisible({ timeout: 30000 });
    await expect.poll(() => Boolean(socket)).toBe(true);
    await navigate(page, `/chat/${A}`);
    await expect(page.locator('main')).toContainText('Alpha before', { timeout: 30000 });
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeVisible();
    const origin = await page.evaluate(() => performance.timeOrigin);
    const initialSockets = socketCount;
    await shot(page, 'before-navigation');

    await navigate(page, '/dashboards');
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toHaveCount(0);
    emit(A, 'during absence ', 'Alpha before '.length);
    // The second session starts without any mounted chat view.
    emit(B, 'Beta before during absence ', 0);
    await navigate(page, `/chat/${A}`);
    await expect.poll(() => loads.get(A)).toBeGreaterThan(1);
    await expect(page.locator('main')).toContainText('Alpha before during absence');
    await expect(page.locator('main')).toContainText('KYO-495 Alpha navigation test');
    await expect(page.locator('main')).not.toContainText('Beta before');
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeVisible();
    emit(A, 'and after return.', 'Alpha before during absence '.length);
    await expect(page.locator('main')).toContainText('Alpha before during absence and after return.');
    await shot(page, 'after-return');

    // A failed reload must preserve the run that continued in memory.
    await navigate(page, '/dashboards');
    failAlphaHistory = true;
    const failedHistory = page.waitForResponse(response =>
      response.url().includes('/get_session_messages') && response.status() === 500);
    await navigate(page, `/chat/${A}`);
    await (await failedHistory).finished();
    await flushRendering(page);
    await expect.poll(() => failedHistoryLoads).toBeGreaterThan(0);
    await expect(page.locator('main')).toContainText('Alpha before during absence and after return.');
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeVisible();
    failAlphaHistory = false;

    await navigate(page, `/chat/${B}`);
    await expect(page.locator('main')).toContainText('Beta before during absence');
    await expect(page.locator('main')).not.toContainText('Alpha before');
    emit(A, ' Still running.', 'Alpha before during absence and after return.'.length);
    emit(B, 'independently.', 'Beta before during absence '.length);
    await expect(page.locator('main')).toContainText('Beta before during absence independently.');
    await shot(page, 'concurrent-session');
    await navigate(page, `/chat/${A}`);
    await expect(page.locator('main')).toContainText('Alpha before during absence and after return. Still running.');
    emit(A, 'Alpha before during absence and after return. Still running.', 0, 'chat_complete');
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeHidden();
    socket.send(JSON.stringify({ type: 'agent_thinking', session_id: A,
      message_id: `answer-${A}`, timestamp: new Date().toISOString(), data: { event: {
        event_id: 'late-reasoning', event_type: 'agent_thought', timestamp: new Date().toISOString(),
        title: 'Delayed terminal reasoning',
      } } }));
    emit(A, 'late duplicate', 'Alpha before during absence and after return. Still running.'.length);
    await flushRendering(page);
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeHidden();
    await expect(page.locator('main')).not.toContainText('late duplicate');
    await shot(page, 'completed');
    await navigate(page, `/chat/${B}`);
    await page.getByRole('button', { name: 'Stop generating', exact: true }).click();
    await expect(page.locator('main')).toContainText('Request cancelled by user.');
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeHidden();
    // Persisted history may arrive before the original send HTTP response.
    await navigate(page, `/chat/${C}`);
    const delayedText = 'KYO-495 delayed HTTP prompt';
    await page.locator('main textarea:visible').fill(delayedText);
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => Boolean(pendingSend)).toBe(true);
    await navigate(page, '/dashboards');
    await navigate(page, `/chat/${C}`);
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeVisible();
    // No stream has supplied a shared turn identity yet. A shared user
    // acknowledgement must reconcile the unresolved optimistic alias safely.
    socket.send(JSON.stringify({ type: 'shared_chat_message', session_id: C,
      message_id: 'persisted-delayed-user', timestamp: new Date().toISOString(), data: {
        message_id: 'persisted-delayed-user', client_msg_id: delayedClientId,
        content: delayedText, type: 'user', timestamp: new Date().toISOString(),
      } }));
    await expect(page.getByText(delayedText, { exact: true })).toHaveCount(1);
    await pendingSend();
    await flushRendering(page);
    await expect(page.getByText(delayedText, { exact: true })).toHaveCount(1);
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeVisible();
    await shot(page, 'delayed-http-return');
    emit(C, 'Delayed answer complete.', 0, 'chat_complete');
    await expect(page.getByRole('button', { name: 'Stop generating', exact: true })).toBeHidden();

    // A rejected unpublished session keeps its draft and error, then retries
    // with a fresh session identity rather than losing the failed prompt.
    await navigate(page, '/chat');
    const rejectedText = 'KYO-495 rejected first prompt';
    await page.locator('main textarea:visible').fill(rejectedText);
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect(page.locator('main')).toContainText('Controlled new-chat rejection');
    await expect(page.getByText(rejectedText, { exact: true })).toHaveCount(1);
    await page.locator('main textarea:visible').fill('KYO-495 rejected retry prompt');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => rejectedSessions.length).toBe(2);
    await flushRendering(page);
    expect(rejectedSessions[0]).not.toBe(rejectedSessions[1]);
    await expect(page.getByText(rejectedText, { exact: true })).toHaveCount(1);
    await expect(page.getByText('KYO-495 rejected retry prompt', { exact: true })).toHaveCount(1);
    await shot(page, 'rejected-draft-retry');
    await navigate(page, `/dashboard/${dashboardId}/edit`);
    await page.getByRole('button', { name: 'Toggle copilot', exact: true }).waitFor();
    let created = page.waitForResponse(response => response.url().includes('/create_copilot_session'));
    await page.getByRole('button', { name: 'Toggle copilot', exact: true }).click();
    await (await created).finished();
    await flushRendering(page);
    await expect.poll(() => copilotSequence).toBe(1);
    const copilotPanel = page.locator('aside').filter({ has: page.getByPlaceholder('Ask about your dashboard...') });
    await expect.poll(async () => (await copilotPanel.boundingBox())?.width || 0).toBeGreaterThanOrEqual(380);
    // Exercise incoming-run ownership and ephemeral cleanup independently of
    // the dashboard editor's save-before-send workflow.
    const copilotFrame = (sid, text) => socket.send(JSON.stringify({ type: 'chat_stream',
      session_id: sid, message_id: `reply-${sid}`, timestamp: new Date().toISOString(),
      data: { content: text, content_offset: 0, context_type: 'dashboard_copilot' } }));
    copilotFrame('copilot-1', 'Copilot first session reply');
    await expect(page.locator('main')).toContainText('Copilot first session reply');
    await shot(page, 'copilot');
    await page.getByRole('button', { name: 'Close copilot', exact: true }).click();
    await expect.poll(() => deletedCopilots).toContain('copilot-1');
    copilotFrame('copilot-1', 'Late closed session reply');
    created = page.waitForResponse(response => response.url().includes('/create_copilot_session'));
    await page.getByRole('button', { name: 'Toggle copilot', exact: true }).click();
    await (await created).finished();
    await flushRendering(page);
    await expect.poll(() => copilotSequence).toBe(2);
    await expect.poll(async () => (await copilotPanel.boundingBox())?.width || 0).toBeGreaterThanOrEqual(380);
    await expect(page.locator('main')).not.toContainText('Copilot first session reply');
    await expect(page.locator('main')).not.toContainText('Late closed session reply');
    await page.getByPlaceholder('Ask about your dashboard...').waitFor();
    copilotFrame('copilot-2', 'Copilot fresh session reply');
    await expect(page.locator('main')).toContainText('Copilot fresh session reply');
    await shot(page, 'copilot-reopened');
    await navigate(page, '/dashboards');
    await expect.poll(() => deletedCopilots).toContain('copilot-2');
    expect(await page.evaluate(() => performance.timeOrigin)).toBe(origin);
    expect(socketCount).toBe(initialSockets);
    expect(errors).toEqual([]);
    const report = { passed: true, screenshots, historyLoads: Object.fromEntries(loads),
      failedHistoryLoads, copilotSessionsCreated: copilotSequence, deletedCopilots, socketCount, rejectedSessions, delayedSendReconciled: true, errors };
    fs.writeFileSync('/tmp/kyo-495-browser-result.json', JSON.stringify(report, null, 2));
    console.log(JSON.stringify(report, null, 2));
  } catch (error) {
    await shot(page, 'failure');
    console.error(error);
    console.error(JSON.stringify({ errors, historyLoads: Object.fromEntries(loads), screenshots }));
    process.exitCode = 1;
  } finally { await browser.close(); }
})();
