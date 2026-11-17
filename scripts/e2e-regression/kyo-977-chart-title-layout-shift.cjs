// KYO-977: titled charts shift down when their data loads.
//
// Bug: `.chartml-loading` in crates/kyomi-ui/style/main.css was forced to
// `position: absolute; inset: 0` for every loading state. chartml-leptos
// already reserves `min-height: H` in flow for the FIRST load and only goes
// absolute itself on a re-fetch (old chart still on screen). The CSS made the
// first-load placeholder take no space, so the card was shorter while loading
// than once rendered: it grew by the title row (and, for untitled charts, by
// the wrapper's padding) when the data arrived.
//
// The script builds a real dashboard (a Postgres datasource running constant
// `generate_series` queries) and measures every chart card while the chart
// data requests are HELD, so the loading state is stable, and again once they
// are released. It asserts, in the light and the dark theme:
//
//   * first load, dashboard viewer AND editor: card height and top are
//     identical loading vs rendered for a titled metric, a titled 400px chart,
//     a titled chart with an explicit `visualize.style.height`, untitled
//     charts and a table;
//   * the loading logo is vertically centred in the in-flow reservation, which
//     sits below the title;
//   * refresh (per-chart button on every wide card, and the dashboard-wide
//     button): the old chart stays on screen, the loading overlay covers it,
//     and nothing changes size;
//   * changing the chart type (line -> table -> metric): a rendered card
//     carries no `data-chart-type` tag, and the table styling and the tile's
//     reserved height follow the type;
//   * a chart whose query fails keeps the tile at the chart's nominal height
//     (402px for a 400px chart), as before; that is shorter than the loading
//     reservation (title + chart height) because the error block reserves none;
//   * the loading background tracks the card in both themes.
//
// Real requests are only DELAYED (held, then continued); no response is
// faked or altered.
//
// Run against a dev server whose `crates/kyomi-ui/dist` is current (`trunk
// build`):
//     NODE_PATH=<repo>/node_modules PORT=3177 \
//         node scripts/e2e-regression/kyo-977-chart-title-layout-shift.cjs
//
// Environment (all optional):
//   E2E_BASE_URL       app base URL    (default http://localhost:$PORT, else :3000)
//   E2E_TEST_EMAIL / E2E_TEST_PASSWORD
//                      workspace admin (default e2e-test@kyomi.dev, see seed-test-user.py)
//   E2E_PG_HOST / E2E_PG_PORT / E2E_PG_DATABASE / E2E_PG_USER / E2E_PG_PASSWORD
//                      a Postgres the SERVER can reach (default localhost:5433
//                      kyomi_kyo977 kyomi/password); no table is read.
//
// Creates a datasource and a dashboard and deletes both when it finishes; the
// test user's theme is reset to `system`. Run it once per server start: the
// auth endpoint rate-limits logins per IP. Exits 0 when every assertion
// passes, 1 otherwise.

const { chromium } = require('playwright');

const BASE =
  process.env.E2E_BASE_URL || `http://localhost:${process.env.PORT || '3000'}`;
const EMAIL = process.env.E2E_TEST_EMAIL || 'e2e-test@kyomi.dev';
const PASSWORD = process.env.E2E_TEST_PASSWORD || 'E2eTestPass123!';
const PG = {
  host: process.env.E2E_PG_HOST || 'localhost',
  port: Number(process.env.E2E_PG_PORT || 5433),
  database: process.env.E2E_PG_DATABASE || 'kyomi_kyo977',
  user: process.env.E2E_PG_USER || 'kyomi',
  password: process.env.E2E_PG_PASSWORD || 'password',
};
const SLUG = `kyo977-${Date.now()}`;
const EPS = 0.5; // sub-pixel tolerance for getBoundingClientRect comparisons

const SERIES =
  "SELECT d::date AS day, EXTRACT(day FROM d)::int * 3 AS signups " +
  "FROM generate_series('2026-01-01'::date, '2026-01-20'::date, '1 day') d";

const DASHBOARD = `# KYO-977 layout shift fixture

\`\`\`chartml
- type: chart
  version: 1
  title: "Unique Visitors Today"
  layout:
    colSpan: 3
  data:
    datasource: ${SLUG}
    query: SELECT 1234 AS current, 1100 AS previous
  visualize:
    type: metric
    value: current
    compareWith: previous
- type: chart
  version: 1
  layout:
    colSpan: 3
  data:
    datasource: ${SLUG}
    query: SELECT 4321 AS current, 4000 AS previous
  visualize:
    type: metric
    value: current
    compareWith: previous
\`\`\`

\`\`\`chartml
type: chart
version: 1
title: "Daily Signups"
data:
  datasource: ${SLUG}
  query: ${SERIES}
visualize:
  type: line
  columns: day
  rows: signups
\`\`\`

\`\`\`chartml
type: chart
version: 1
title: "Short Signups"
data:
  datasource: ${SLUG}
  query: ${SERIES}
visualize:
  type: bar
  columns: day
  rows: signups
  style:
    height: 250
\`\`\`

\`\`\`chartml
type: chart
version: 1
data:
  datasource: ${SLUG}
  query: ${SERIES}
visualize:
  type: bar
  columns: day
  rows: signups
\`\`\`

\`\`\`chartml
type: chart
version: 1
title: "Signup Table"
data:
  datasource: ${SLUG}
  query: ${SERIES}
visualize:
  type: table
  columns:
    - field: day
      label: "Day"
    - field: signups
      label: "Signups"
\`\`\`

\`\`\`chartml
type: chart
version: 1
title: "Failing Chart"
data:
  datasource: ${SLUG}
  query: SELECT 1 / (EXTRACT(day FROM now())::int * 0) AS x
visualize:
  type: line
  columns: x
  rows: x
\`\`\`
`;

// Card order matches the fixture above. `wide` cards show the header bar's
// buttons (the metric cards are too narrow for them).
const CARDS = [
  { name: 'titled metric' },
  { name: 'untitled metric' },
  { name: 'titled 400px line chart', wide: true, titled: true },
  { name: 'titled chart, explicit height 250', wide: true, titled: true },
  { name: 'untitled 400px bar chart', wide: true },
  { name: 'table (title hidden by CSS)', wide: true },
  { name: 'failing chart', failing: true },
];
CARDS.forEach((c, i) => { c.index = i; });
const OK_CARDS = CARDS.filter((c) => !c.failing);
const LINE = 2;
const FAILING = 6;

let failures = 0;
function check(ok, label, detail) {
  if (!ok) failures += 1;
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${label}${detail ? '  ' + detail : ''}`);
}
const fmt = (n) => (Math.round(n * 10) / 10).toString();

// ── Server functions ─────────────────────────────────────────────────────
// There is no REST route for creating datasources, so the fixture uses the
// app's own server functions. Their URLs end in a per-module hash that is only
// present in the served wasm, so it is read from there once per run.
let wasmText = null;
async function postServerFn(page, name, body) {
  if (wasmText === null) {
    const html = await (await page.request.get(BASE + '/')).text();
    const wasmPath = html.match(/\/kyomi-ui-[0-9a-f]+_bg\.wasm/)[0];
    wasmText = (await (await page.request.get(BASE + wasmPath)).body()).toString('latin1');
  }
  const m = wasmText.match(new RegExp(`/leptos-api/${name}(\\d{19,20})`));
  if (!m) throw new Error(`server fn ${name} not found in the served wasm`);
  // The string table has no separators, so a 20-digit match may carry the
  // first digit of the next string: try the full match, then one digit less.
  let res;
  for (const path of [m[0], m[0].slice(0, -1)]) {
    res = await page.request.post(BASE + path, body);
    if (res.status() !== 400 || !/Could not find a server function/.test(await res.text())) {
      return res;
    }
  }
  return res;
}

async function login(page) {
  await page.goto(BASE + '/login');
  await page.waitForSelector('input[type="email"]');
  // The form only submits once the wasm has hydrated; there is no DOM marker
  // for that, so wait as BUILD_AND_TESTING.md's Playwright recipe does.
  await page.waitForTimeout(5000);
  await page.fill('input[type="email"]', EMAIL);
  await page.fill('input[type="password"]', PASSWORD);
  await page.click('button[type="submit"]');
  await page.waitForURL((u) => !u.toString().includes('/login'), { timeout: 20000 });
}

async function expectOk(res, what) {
  if (res.status() !== 200) throw new Error(`${what} failed: ${res.status()} ${await res.text()}`);
  const text = await res.text();
  return text ? JSON.parse(text) : null;
}

// Records each id on `fixture` as soon as it exists so a failure part-way
// through still lets the cleanup delete what was created.
async function createFixture(page, fixture) {
  const ds = await expectOk(await postServerFn(page, 'create_datasource_modal', {
    data: {
      name: `KYO-977 layout fixture ${SLUG}`,
      slug: SLUG,
      datasource_type: 'postgres',
      connection_config: { host: PG.host, port: PG.port, database: PG.database, ssl_mode: 'disable' },
      credentials: { username: PG.user, password: PG.password },
    },
  }), 'datasource create');
  fixture.datasourceId = ds.id;
  fixture.dashboardId = await expectOk(await postServerFn(page, 'create_dashboard', {
    form: { title: `KYO-977 layout shift ${SLUG}`, content: DASHBOARD },
  }), 'dashboard create');
}

async function setTheme(page, theme) {
  await expectOk(await postServerFn(page, 'update_theme', { form: { theme } }), 'update_theme');
}

// Each step runs even if an earlier one failed; the first error is rethrown.
async function deleteFixture(page, fixture) {
  const steps = [];
  if (fixture.dashboardId) {
    steps.push(() => postServerFn(page, 'delete_dashboard', { form: { dashboard_id: fixture.dashboardId } }).then((r) => expectOk(r, 'dashboard delete')));
  }
  if (fixture.datasourceId) {
    steps.push(() => postServerFn(page, 'delete_datasource', { form: { datasource_id: fixture.datasourceId } }).then((r) => expectOk(r, 'datasource delete')));
  }
  if (steps.length > 0) steps.push(() => setTheme(page, 'system'));
  let firstError = null;
  for (const step of steps) {
    try { await step(); } catch (e) { firstError = firstError || e; }
  }
  if (firstError) throw firstError;
}

// ── In-page measurement ──────────────────────────────────────────────────
// Runs in the page. One record per `.chart-card`.
function snapshotCards() {
  // `top` is relative to the first card: the page scrolls inside an inner
  // container, so viewport-relative tops move when a click scrolls.
  const origin = document.querySelector('.chart-card').getBoundingClientRect().top;
  return [...document.querySelectorAll('.chart-card')].map((card) => {
    const rect = card.getBoundingClientRect();
    const loading = card.querySelector('.chartml-loading');
    const title = card.querySelector('.chart-title');
    const wrapper = card.querySelector(':scope > div:not(:first-child)');
    const out = {
      height: rect.height,
      top: rect.top - origin,
      loading: !!loading,
      rendered: !!card.querySelector('.chartml-svg-host'),
      table: !!card.querySelector('.chartml-table'),
      error: !!card.querySelector('.chartml-error'),
      titleShown: !!(title && title.offsetParent),
      titleBottom: title && title.offsetParent ? title.getBoundingClientRect().bottom : null,
      typeAttr: card.getAttribute('data-chart-type'),
      wrapperMinHeight: wrapper ? getComputedStyle(wrapper).minHeight : null,
      wrapperPadding: wrapper ? getComputedStyle(wrapper).paddingTop : null,
    };
    if (loading) {
      const lr = loading.getBoundingClientRect();
      const logo = loading.querySelector('.chartml-spinner').getBoundingClientRect();
      const cr = loading.parentElement.getBoundingClientRect();
      const cs = getComputedStyle(loading);
      out.loadingBox = { top: lr.top, height: lr.height, bottom: lr.bottom };
      out.containerBox = { top: cr.top, height: cr.height };
      out.logoCentreOffset = logo.top + logo.height / 2 - (lr.top + lr.height / 2);
      out.loadingPosition = cs.position;
      out.loadingBg = cs.backgroundColor;
      out.cardBg = getComputedStyle(card).backgroundColor;
    }
    return out;
  });
}

// `predicate` runs in the page, so everything it needs comes in through `arg`.
async function waitFor(page, predicate, arg, label) {
  try {
    await page.waitForFunction(predicate, arg, { timeout: 60000 });
  } catch (e) {
    const state = await page.evaluate(() => ({
      url: location.href,
      cards: document.querySelectorAll('.chart-card').length,
      loading: document.querySelectorAll('.chartml-loading').length,
    }));
    throw new Error(`timed out waiting for: ${label} (${JSON.stringify(state)})`);
  }
}

const allLoading = (n) => {
  const cards = document.querySelectorAll('.chart-card');
  return cards.length === n && [...cards].every((c) => c.querySelector('.chartml-loading .chartml-spinner'));
};
const allSettled = (n) => {
  const cards = document.querySelectorAll('.chart-card');
  return cards.length === n && [...cards].every((c) =>
    !c.querySelector('.chartml-loading') &&
    c.querySelector('.chartml-svg-host, .chartml-table, .chartml-error'));
};
const cardsLoading = (indexes) => {
  const cards = document.querySelectorAll('.chart-card');
  return indexes.every((i) => cards[i].querySelector('.chartml-loading'));
};

// Holds every chart data request until released. Requests are continued
// untouched, only later.
async function installHold(page) {
  let held = [];
  let holding = true;
  await page.route('**/api/v1/query-arrow', (route) => {
    if (holding) held.push(route); else route.continue();
  });
  return {
    hold: () => { holding = true; },
    release: async () => {
      holding = false;
      const pending = held; held = [];
      await Promise.all(pending.map((r) => r.continue()));
    },
  };
}

async function settle(page, n, label) {
  await waitFor(page, allSettled, n, label);
  await page.waitForTimeout(500); // let SVG animations settle layout
}

// The header bar is a hover zone: its children only take pointer events once
// the bar itself is hovered.
async function openHeader(card) {
  const header = card.locator(':scope > div').first();
  await header.hover();
  return header;
}

// ── Scenarios ────────────────────────────────────────────────────────────
async function firstLoad(page, theme, surface, url) {
  const tag = `${theme} ${surface}`;
  const { hold, release } = await installHold(page);
  hold();
  await page.goto(url);
  const n = CARDS.length;
  await waitFor(page, allLoading, n, `${tag}: all cards in loading state`);
  const isDark = await page.evaluate(() => document.documentElement.classList.contains('dark'));
  check(isDark === (theme === 'dark'), `${tag}: page is in ${theme} theme`);

  const loading = await page.evaluate(snapshotCards);
  await release();
  await settle(page, n, `${tag}: all cards settled`);
  const rendered = await page.evaluate(snapshotCards);

  OK_CARDS.forEach((c) => {
    const l = loading[c.index]; const r = rendered[c.index];
    check(
      Math.abs(l.height - r.height) <= EPS && Math.abs(l.top - r.top) <= EPS,
      `${tag}: first load, ${c.name}: no shift`,
      `loading ${fmt(l.height)}px @${fmt(l.top)} -> rendered ${fmt(r.height)}px @${fmt(r.top)}`,
    );
  });

  // The logo is centred in the in-flow reservation, which is below the title.
  CARDS.forEach((c) => {
    const l = loading[c.index];
    check(Math.abs(l.logoCentreOffset) <= 1, `${tag}: ${c.name}: logo vertically centred in loading box`,
      `offset ${fmt(l.logoCentreOffset)}px`);
    check(l.loadingPosition !== 'absolute', `${tag}: ${c.name}: first-load placeholder is in flow`,
      `position ${l.loadingPosition}`);
    if (c.titled) {
      check(l.titleBottom !== null && l.loadingBox.top >= l.titleBottom - EPS,
        `${tag}: ${c.name}: loading box sits below the title`,
        `title bottom ${fmt(l.titleBottom)} box top ${fmt(l.loadingBox.top)}`);
    }
  });
  check(loading.every((l) => l.loadingBg === l.cardBg),
    `${tag}: loading background equals card background`, `${loading[0].loadingBg} vs ${loading[0].cardBg}`);

  // The viewer keeps an errored tile at the chart's nominal height (a 400px
  // chart plus the card's 2px border), as it did before the placeholder fix.
  // That is below the loading reservation (title + 400px), which an error does
  // not reserve. The editor has no height wrapper, so only the viewer asserts it.
  if (surface === 'viewer') {
    const err = rendered[FAILING];
    check(err.error && Math.abs(err.height - 402) <= EPS,
      `${tag}: failing chart shows its error at the nominal tile height`,
      `error=${err.error} height ${fmt(err.height)}px (expected 402px)`);
  }
  return rendered;
}

async function refreshScenarios(page, theme, rendered) {
  const n = CARDS.length;
  const { hold, release } = await installHold(page);

  async function refreshAndCheck(label, cards, trigger) {
    hold();
    await trigger();
    await waitFor(page, cardsLoading, cards.map((c) => c.index), `${label}: overlay shown`);
    const during = await page.evaluate(snapshotCards);
    cards.forEach((c) => {
      const d = during[c.index]; const r = rendered[c.index];
      const tag = `${theme}: ${label}, ${c.name}`;
      check(d.rendered || d.table, `${tag}: old chart stays on screen`);
      check(d.loadingPosition === 'absolute', `${tag}: overlay is absolutely positioned`, `position ${d.loadingPosition}`);
      check(Math.abs(d.loadingBox.top - d.containerBox.top) <= EPS &&
        Math.abs(d.loadingBox.height - d.containerBox.height) <= EPS,
        `${tag}: overlay covers the old chart`,
        `overlay ${fmt(d.loadingBox.height)}px vs chart area ${fmt(d.containerBox.height)}px`);
      check(Math.abs(d.height - r.height) <= EPS && Math.abs(d.top - r.top) <= EPS,
        `${tag}: no shift while refreshing`, `rendered ${fmt(r.height)}px -> refreshing ${fmt(d.height)}px`);
    });
    await release();
    await settle(page, n, `${label}: refresh finished`);
    const after = await page.evaluate(snapshotCards);
    cards.forEach((c) => {
      check(Math.abs(after[c.index].height - rendered[c.index].height) <= EPS,
        `${theme}: ${label}, ${c.name}: no shift after refresh`,
        `${fmt(rendered[c.index].height)}px -> ${fmt(after[c.index].height)}px`);
    });
  }

  for (const c of OK_CARDS.filter((x) => x.wide)) {
    await refreshAndCheck('per-chart refresh', [c], async () => {
      const header = await openHeader(page.locator('.chart-card').nth(c.index));
      await header.locator('button[title="Refresh"]').click();
    });
  }
  await refreshAndCheck('dashboard refresh', OK_CARDS, async () => {
    await page.locator('[aria-label="Refresh all charts"]').click();
  });
}

async function typeOverride(page, theme) {
  const card = page.locator('.chart-card').nth(LINE);

  async function pick(label) {
    const header = await openHeader(card);
    await header.locator('button').first().click(); // chart type selector
    // The menu is a popover rendered outside the card.
    await page.locator(`button:has(> span:text-is("${label}"))`).click();
  }
  const state = async () => (await page.evaluate(snapshotCards))[LINE];

  // Once a chart has rendered, the card is no longer tagged with its type, so
  // its styling follows the content on screen and an override cannot restyle
  // the old chart before the new one has rendered. (The new data is already
  // cached, so the refetch finishes too fast to hold; the end states are what
  // can be asserted.)
  let s = await state();
  check(s.typeAttr === null, `${theme}: type override: rendered card carries no data-chart-type tag`,
    `attr ${s.typeAttr}`);

  // line -> table
  await pick('Table');
  await waitFor(page, (i) => document.querySelectorAll('.chart-card')[i].querySelector('.chartml-table'), LINE,
    'type override to table: table rendered');
  s = await state();
  check(s.table && !s.titleShown && s.wrapperPadding === '0px' && s.typeAttr === null,
    `${theme}: type override line -> table: table card has no padding and no title`,
    `table=${s.table} title=${s.titleShown} padding=${s.wrapperPadding} attr=${s.typeAttr}`);
  check(s.wrapperMinHeight === '400px', `${theme}: type override -> table: tile reservation is 400px`,
    `min-height ${s.wrapperMinHeight}`);

  // table -> metric: the tile's reserved height follows the new type.
  await pick('Metric');
  // The line chart's columns/rows are not a metric spec, so chartml may show
  // an error instead of a metric; the reservation must follow the type either way.
  await waitFor(page, (i) => {
    const c = document.querySelectorAll('.chart-card')[i];
    return !c.querySelector('.chartml-table') && !c.querySelector('.chartml-loading') &&
      c.querySelector('.chartml-svg-host, .chartml-error');
  }, LINE, 'type override to metric: metric (or its error) shown');
  s = await state();
  check(s.wrapperMinHeight === '150px' && s.titleShown && s.wrapperPadding === '16px',
    `${theme}: type override table -> metric: tile reservation and styling follow the type`,
    `min-height ${s.wrapperMinHeight} title=${s.titleShown} padding=${s.wrapperPadding} (expected 150px)`);
}

async function runTheme(browser, storageState, theme, fixture, setupPage) {
  console.log(`\n=== theme: ${theme} ===`);
  // The theme is read from localStorage before first paint (index.html) and
  // synced from the user's profile once the layout loads, so set both.
  await setTheme(setupPage, theme);
  // A fresh context per scenario: nothing cached by an earlier one (query
  // results, local-first stores) may turn a first load into an instant one.
  async function fresh() {
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, storageState });
    await ctx.addInitScript((t) => {
      try { localStorage.setItem('kyomi-theme', t); } catch (e) { /* storage blocked */ }
    }, theme);
    const page = await ctx.newPage();
    await page.emulateMedia({ colorScheme: theme });
    return { ctx, page };
  }

  let s = await fresh();
  try {
    const rendered = await firstLoad(s.page, theme, 'viewer', `${BASE}/dashboard/${fixture.dashboardId}`);
    await refreshScenarios(s.page, theme, rendered);
    await typeOverride(s.page, theme);
  } finally { await s.ctx.close(); }

  s = await fresh();
  try {
    await firstLoad(s.page, theme, 'editor', `${BASE}/dashboard/${fixture.dashboardId}/edit`);
  } finally { await s.ctx.close(); }
}

(async () => {
  const browser = await chromium.launch({ headless: true });
  let setup = null;
  let setupPage = null;
  const fixture = {};
  try {
    // One login for the whole run; every scenario context reuses its cookies.
    setup = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    setupPage = await setup.newPage();
    await login(setupPage);
    await createFixture(setupPage, fixture);
    const storageState = await setup.storageState();
    for (const theme of ['light', 'dark']) {
      await runTheme(browser, storageState, theme, fixture, setupPage);
    }
  } catch (e) {
    failures += 1;
    console.log(`FAIL  script error: ${e.message}`);
  } finally {
    if (setupPage) {
      try { await deleteFixture(setupPage, fixture); } catch (e) {
        failures += 1;
        console.log(`FAIL  fixture cleanup: ${e.message}`);
      }
    }
    await browser.close();
  }
  console.log(failures === 0 ? '\nALL PASS' : `\n${failures} FAILURE(S)`);
  process.exit(failures === 0 ? 0 : 1);
})();
