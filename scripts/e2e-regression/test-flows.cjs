#!/usr/bin/env node
/**
 * E2E Regression Tests — Flows Group
 * Flows: flows-01 (home redirect)
 *
 * Run: NODE_PATH=/home/jason/repos/kyomi/node_modules node /home/jason/repos/kyomi/scripts/e2e-regression/test-flows.cjs
 */

'use strict';

const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

// ── Config ────────────────────────────────────────────────────────────────────

const config = {
  servers: {
    react: 'http://localhost:8002',
    leptos: 'http://localhost:3000',
  },
  testUser: {
    email: 'e2e-test@kyomi.dev',
    password: 'E2eTestPass123!',
  },
  timeouts: {
    navigation: 15000,
    action: 8000,
    // Leptos WASM needs time to boot + run async effects before redirects kick in
    wasm_settle: 3000,
  },
  screenshotsDir: '/tmp/e2e-regression/flows',
};

// ── Helpers ───────────────────────────────────────────────────────────────────

function mkdirp(dir) {
  fs.mkdirSync(dir, { recursive: true });
}

async function launchPage(server) {
  const baseUrl = config.servers[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 800 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  return { browser, context, page, baseUrl };
}

async function loginAndGetPage(server) {
  const { browser, context, page, baseUrl } = await launchPage(server);
  await page.goto(`${baseUrl}/login`, {
    waitUntil: 'networkidle',
    timeout: config.timeouts.navigation,
  });
  await page.fill('input[type="email"], input[name="email"]', config.testUser.email, {
    timeout: config.timeouts.action,
  });
  await page.fill('input[type="password"], input[name="password"]', config.testUser.password, {
    timeout: config.timeouts.action,
  });
  await page.click('button[type="submit"]', { timeout: config.timeouts.action });
  await page.waitForURL((url) => !url.toString().includes('/login'), {
    timeout: config.timeouts.navigation,
  });
  return { browser, context, page, baseUrl };
}

async function saveScreenshot(page, flowId, server) {
  const dir = path.join(config.screenshotsDir, flowId);
  mkdirp(dir);
  const filepath = path.join(dir, `${server}.png`);
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
}

function pass(message) {
  return { pass: true, message };
}

function fail(message) {
  return { pass: false, message };
}

// ── Flow: flows-01 — Home redirect ───────────────────────────────────────────
//
// After login, navigate to `/`. The app should display the user's configured
// landing page content (default: chat). The URL must NOT redirect to /login.
//
// IMPORTANT — React vs Leptos behavioral difference:
//   React (LandingRedirect.jsx): When landing_page is "chat" (default), renders
//   <Chat /> DIRECTLY at "/" — NO URL change. The URL stays at "/". This is
//   intentional design in LandingRedirect.jsx (see line 27-29).
//
//   Leptos (home.rs): ALWAYS navigates away from "/" to the resolved target
//   (e.g. "/chat") using use_navigate() with replace:true. URL changes.
//
// This means React PASSES the spec assertion "URL does NOT stay on /" only when
// landing_page != "chat". For "chat" (the default), React intentionally keeps
// the URL at "/" while rendering chat content. Leptos always redirects.
//
// Test strategy: we test the user-visible outcome, not the URL mechanism:
//   1. User is not redirected to /login (remains authenticated)
//   2. Chat content is visible (or appropriate landing page content)
//   3. Page is not blank/crash
//   4. Logo renders correctly (no broken image)
//   URL-change assertion is noted as a behavioral diff but is NOT a failure
//   for React since it's intentional design.

async function testHomeRedirect(server) {
  let ctx;
  const assertions = [];
  let screenshot = null;

  try {
    // Login first
    ctx = await loginAndGetPage(server);
    const { browser, page, baseUrl } = ctx;

    // Navigate to root
    await page.goto(`${baseUrl}/`, {
      waitUntil: 'networkidle',
      timeout: config.timeouts.navigation,
    });

    // Wait for WASM to settle and the Effect/useEffect redirect to fire
    await page.waitForTimeout(config.timeouts.wasm_settle);
    await page.waitForLoadState('networkidle', { timeout: 5000 }).catch(() => {});

    const finalUrl = page.url();
    const finalPath = new URL(finalUrl).pathname;

    // Assert 1: Must NOT redirect to /login (user is authenticated)
    assertions.push(
      !finalPath.includes('/login')
        ? pass(`Did not redirect to /login — user remains authenticated (final path: ${finalPath})`)
        : fail(`Redirected to /login — user lost authentication during home redirect`)
    );

    // Assert 2: Must land on a known valid page OR stay at "/" with chat content rendered.
    // React intentionally stays at "/" when landing_page is "chat" and renders Chat there.
    // Leptos always navigates to "/chat".
    const validPaths = ['/chat', '/watches', '/sql-editor', '/dashboards', '/'];
    const isValidTarget =
      validPaths.some((p) => finalPath === p || finalPath.startsWith(p + '/')) ||
      finalPath.match(/^\/dashboard\/[a-z0-9-]+$/);
    assertions.push(
      isValidTarget
        ? pass(`Final path is acceptable: ${finalPath}`)
        : fail(`Final path is unexpected: ${finalPath} — not /chat, /watches, /sql-editor, /dashboards, /, or /dashboard/<id>`)
    );

    // Assert 3: Page has substantive content (not blank/crash)
    const bodyText = await page.evaluate(
      () => document.body.innerText || document.body.textContent || ''
    );
    assertions.push(
      bodyText && bodyText.trim().length > 100
        ? pass(`Page has substantive content (${bodyText.trim().length} chars)`)
        : fail(`Page body too short after redirect — possible blank/crash state (${bodyText.trim().length} chars)`)
    );

    // Assert 4: Chat UI or chat empty state is visible (default landing page is "chat").
    // React renders Chat at "/"; Leptos renders it at "/chat".
    // The test user has no datasource connected, so we expect the empty state.
    // Possible text markers depending on datasource state:
    //   - No datasource: "Connect a datasource to get started" (React) or "Ready to dive into the data" (Leptos)
    //   - With datasource: chat input (textarea) visible
    const chatKeywords = [
      'connect a datasource',
      'ready to dive',
      'ask me anything',
      'new chat',
    ];
    const lowerBody = bodyText.toLowerCase();
    const chatVisible = chatKeywords.some((kw) => lowerBody.includes(kw));
    assertions.push(
      chatVisible
        ? pass('Chat content is visible (chat empty state or chat UI text found in page body)')
        : fail(`Chat content NOT visible — expected chat page content after home redirect. Body excerpt: "${bodyText.trim().slice(0, 200)}"`)
    );

    // Assert 5 (visual): Logo renders correctly — check naturalWidth > 0
    const logos = await page.$$('img[alt="Kyomi"], img[alt*="yomi"], img[alt*="logo"]');
    let logoOk = false;
    let logoBroken = false;
    for (const logo of logos) {
      const naturalWidth = await logo.evaluate((el) => el.naturalWidth).catch(() => 0);
      if (naturalWidth > 0) {
        logoOk = true;
      } else {
        logoBroken = true;
      }
    }
    if (logos.length === 0) {
      // Fallback: check if any <img> in the nav/header area loaded
      const anyImg = await page.$$('nav img, header img, aside img');
      let anyImgOk = false;
      for (const img of anyImg) {
        const nw = await img.evaluate((el) => el.naturalWidth).catch(() => 0);
        if (nw > 0) anyImgOk = true;
      }
      assertions.push(
        anyImgOk
          ? pass('No img[alt="Kyomi"] found but other nav images load correctly')
          : fail('No logo image found in navigation area — logo may be missing entirely')
      );
    } else if (logoOk) {
      assertions.push(pass(`Logo renders correctly (naturalWidth > 0; ${logos.length} logo(s) found)`));
    } else {
      assertions.push(fail(`Visual: Logo image broken — ${logos.length} logo img(s) all have naturalWidth=0 (shows alt text placeholder instead of rendering)`));
    }

    screenshot = await saveScreenshot(page, 'flows-01-home-redirect', server);
    await browser.close();
  } catch (e) {
    assertions.push(fail(`Exception: ${e.message}`));
    if (ctx) await ctx.browser.close().catch(() => {});
  }

  return { assertions, screenshot };
}

// ── Flow runner ───────────────────────────────────────────────────────────────

const FLOWS = [
  {
    id: 'flows-01',
    name: 'Home "/" redirects to user landing page (default /chat)',
    fn: testHomeRedirect,
  },
];

function classify(reactResult, leptosResult) {
  const rPass = reactResult.status === 'pass';
  const lPass = leptosResult.status === 'pass';
  if (rPass && lPass) return 'PASS';
  if (rPass && !lPass) return 'REGRESSION';
  if (!rPass && lPass) return 'BEHAVIORAL_DIFF';
  return 'BOTH_FAIL';
}

async function runFlow(flow) {
  console.log(`\nRunning: ${flow.id} — ${flow.name}`);
  const flowResult = { id: flow.id, name: flow.name };

  for (const server of ['react', 'leptos']) {
    console.log(`  [${server}] starting...`);
    try {
      const { assertions, screenshot } = await flow.fn(server);
      const allPass = assertions.every((a) => a.pass);
      flowResult[server] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter((a) => !a.pass).map((a) => a.message).join('; '),
      };
      if (allPass) {
        console.log(`  [${server}] PASS`);
      } else {
        console.log(`  [${server}] FAIL`);
        assertions.filter((a) => !a.pass).forEach((a) => console.log(`    x ${a.message}`));
      }
    } catch (e) {
      flowResult[server] = {
        status: 'fail',
        assertions: [fail(`Unhandled exception: ${e.message}`)],
        screenshot: null,
        error: e.message,
      };
      console.log(`  [${server}] EXCEPTION: ${e.message}`);
    }
  }

  flowResult.classification = classify(flowResult.react, flowResult.leptos);

  // Annotate known behavioral differences
  if (flow.id === 'flows-01') {
    const reactPath = flowResult.react.assertions.find((a) => a.message.includes('Final path'))?.message || '';
    const leptosPath = flowResult.leptos.assertions.find((a) => a.message.includes('Final path'))?.message || '';
    flowResult.notes = [
      'React (LandingRedirect.jsx) intentionally renders Chat at "/" without URL change when landing_page=chat.',
      'Leptos (home.rs) always navigates away from "/" to the resolved target (e.g. /chat) via use_navigate(replace:true).',
      'This URL-change difference is by design and is NOT a regression.',
      reactPath ? `React: ${reactPath}` : '',
      leptosPath ? `Leptos: ${leptosPath}` : '',
    ].filter(Boolean).join(' | ');
  } else {
    flowResult.notes = '';
  }

  console.log(`  -> ${flowResult.classification}`);
  return flowResult;
}

// ── Main ──────────────────────────────────────────────────────────────────────

async function main() {
  console.log('E2E Regression — Flows Group');
  console.log('============================');

  mkdirp(config.screenshotsDir);

  const flows = [];
  for (const flow of FLOWS) {
    const result = await runFlow(flow);
    flows.push(result);
  }

  // Save results
  const resultsPath = path.join(config.screenshotsDir, 'results.json');
  const output = {
    group: 'flows',
    timestamp: new Date().toISOString(),
    flows,
  };
  fs.writeFileSync(resultsPath, JSON.stringify(output, null, 2));

  // Print summary
  console.log('\n=== SUMMARY ===');
  const counts = { PASS: 0, REGRESSION: 0, BEHAVIORAL_DIFF: 0, BOTH_FAIL: 0 };
  flows.forEach((f) => {
    const icon = f.classification === 'PASS' ? 'v' : 'x';
    console.log(`${icon} [${f.classification}] ${f.name}`);
    counts[f.classification] = (counts[f.classification] || 0) + 1;
  });
  console.log('\nCounts:', JSON.stringify(counts));
  console.log(`\nResults saved to: ${resultsPath}`);
}

main().catch((e) => {
  console.error('Fatal error:', e);
  process.exit(1);
});
