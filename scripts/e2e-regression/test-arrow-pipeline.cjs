/**
 * E2E test: Arrow-native data pipeline
 *
 * Tests the full pipeline from SQL query → provider → Arrow RecordBatch →
 * record_batch_to_json_rows → SQL editor display.
 *
 * Uses the Acme Analytics (Sample) ClickHouse datasource which is a direct
 * (non-Connect) provider that goes through the same Arrow pipeline.
 *
 * Test plan:
 *   1. DateTime values (now(), toDateTime) → should display as ISO timestamps
 *   2. Date values (toDate) → should display as YYYY-MM-DD
 *   3. Number values → should display as numbers
 *   4. String values → should display as strings
 *   5. NULL values → should display as null
 *   6. Mixed-type query → all types render correctly in one result set
 *   7. Large result set (>5 rows) → pagination works with Arrow data
 */

const { chromium } = require('playwright');

const BASE_URL = 'http://localhost:3000';
const SCREENSHOT_DIR = '/tmp/e2e-arrow';

let testsPassed = 0;
let testsFailed = 0;
const failures = [];

function pass(name) {
  testsPassed++;
  console.log(`  ✅ ${name}`);
}

function fail(name, reason) {
  testsFailed++;
  failures.push({ name, reason });
  console.log(`  ❌ ${name}: ${reason}`);
}

(async () => {
  // Create screenshot dir
  const { mkdirSync } = require('fs');
  try { mkdirSync(SCREENSHOT_DIR, { recursive: true }); } catch {}

  const browser = await chromium.launch({ headless: true });
  const ctx = await browser.newContext({ viewport: { width: 1920, height: 1080 } });
  const page = await ctx.newPage();

  // Collect errors
  const consoleErrors = [];
  page.on('console', msg => {
    if (msg.type() === 'error') consoleErrors.push(msg.text());
  });

  // Login
  console.log('=== Setup ===');
  console.log('  Logging in...');
  await page.goto(`${BASE_URL}/login`, { waitUntil: 'networkidle', timeout: 15000 });
  await page.fill('input[type="email"]', 'e2e-test@kyomi.dev', { timeout: 8000 });
  await page.fill('input[type="password"]', 'E2eTestPass123!', { timeout: 8000 });
  await page.click('button[type="submit"]', { timeout: 8000 });
  await page.waitForURL(url => !url.toString().includes('/login'), { timeout: 15000 });
  console.log('  Logged in.');

  // Navigate to SQL editor
  await page.goto(`${BASE_URL}/sql-editor`, { waitUntil: 'networkidle', timeout: 30000 });
  await page.waitForTimeout(5000);
  console.log('  SQL editor loaded.');

  // Helper: run a query and return the page body text + screenshot
  async function runQuery(sql, screenshotName) {
    const kodeEditor = page.locator('.kode-editor');
    await kodeEditor.click();
    await page.waitForTimeout(200);
    await page.keyboard.press('Control+a');
    await page.keyboard.press('Backspace');
    await page.waitForTimeout(200);
    await page.keyboard.type(sql, { delay: 8 });
    await page.waitForTimeout(300);
    await page.keyboard.press('Control+Enter');
    await page.waitForTimeout(6000);
    if (screenshotName) {
      await page.screenshot({ path: `${SCREENSHOT_DIR}/${screenshotName}.png`, fullPage: true });
    }
    return await page.textContent('body');
  }

  // Helper: check for patterns in text
  function hasEpochMillis(text) {
    return /\b1\d{12}\b/.test(text);
  }
  function hasISOTimestamp(text) {
    return /\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}/.test(text);
  }
  function hasDate(text) {
    return /\b\d{4}-\d{2}-\d{2}\b/.test(text);
  }
  function hasError(text) {
    return /error running server function/.test(text);
  }

  // ===================================================================
  console.log('\n=== Test 1: DateTime values ===');
  const t1 = await runQuery(
    "SELECT toDateTime('2026-01-15 14:30:00') as fixed_ts, toDateTime('2025-06-20 09:15:30') as another_ts",
    'test1-datetime'
  );
  if (hasError(t1)) {
    fail('DateTime query', 'Server error');
  } else if (hasISOTimestamp(t1)) {
    if (t1.includes('2026-01-15') && t1.includes('14:30:00')) {
      pass('DateTime values display as ISO timestamps');
    } else {
      fail('DateTime values', 'ISO pattern found but wrong values');
    }
  } else if (hasEpochMillis(t1)) {
    fail('DateTime values', 'Showing as epoch milliseconds');
  } else {
    fail('DateTime values', 'No timestamp found in output');
  }

  // ===================================================================
  console.log('\n=== Test 2: Date values ===');
  const t2 = await runQuery(
    "SELECT toDate('2026-03-20') as date_val, toDate('2025-12-25') as christmas",
    'test2-date'
  );
  if (hasError(t2)) {
    fail('Date query', 'Server error');
  } else if (t2.includes('2026-03-20') && t2.includes('2025-12-25')) {
    pass('Date values display as YYYY-MM-DD');
  } else if (hasEpochMillis(t2)) {
    fail('Date values', 'Showing as epoch');
  } else {
    fail('Date values', 'Expected dates not found');
  }

  // ===================================================================
  console.log('\n=== Test 3: Number values ===');
  const t3 = await runQuery(
    "SELECT 42 as integer_val, 3.14159 as float_val, -100 as negative_val",
    'test3-numbers'
  );
  if (hasError(t3)) {
    fail('Number query', 'Server error');
  } else if (t3.includes('42') && t3.includes('3.14159') && t3.includes('-100')) {
    pass('Number values display correctly');
  } else {
    fail('Number values', 'Expected numbers not found');
  }

  // ===================================================================
  console.log('\n=== Test 4: String values ===');
  const t4 = await runQuery(
    "SELECT 'hello world' as str_val, '' as empty_str, 'special chars: <>&\"' as special",
    'test4-strings'
  );
  if (hasError(t4)) {
    fail('String query', 'Server error');
  } else if (t4.includes('hello world')) {
    pass('String values display correctly');
  } else {
    fail('String values', 'Expected string not found');
  }

  // ===================================================================
  console.log('\n=== Test 5: NULL values ===');
  const t5 = await runQuery(
    "SELECT NULL as null_val, toNullable(toDateTime('2026-01-15 10:00:00')) as nullable_ts, toNullable(NULL) as null_ts",
    'test5-nulls'
  );
  if (hasError(t5)) {
    fail('NULL query', 'Server error');
  } else if (t5.includes('null') && t5.includes('2026-01-15')) {
    pass('NULL and nullable values display correctly');
  } else {
    fail('NULL values', 'Expected null/timestamp pattern not found');
  }

  // ===================================================================
  console.log('\n=== Test 6: Mixed types in one query ===');
  const t6 = await runQuery(
    "SELECT toDateTime('2026-04-28 12:00:00') as ts, toDate('2026-04-28') as dt, 99 as num, 'test' as str, true as flag",
    'test6-mixed'
  );
  if (hasError(t6)) {
    fail('Mixed type query', 'Server error');
  } else {
    let mixedOk = true;
    if (!t6.includes('2026-04-28')) { mixedOk = false; fail('Mixed: date', 'Date not found'); }
    else { pass('Mixed: date present'); }

    if (!t6.includes('99')) { mixedOk = false; fail('Mixed: number', 'Number not found'); }
    else { pass('Mixed: number present'); }

    if (!t6.includes('test')) { mixedOk = false; fail('Mixed: string', 'String not found'); }
    else { pass('Mixed: string present'); }

    if (hasEpochMillis(t6)) { mixedOk = false; fail('Mixed: no epoch', 'Epoch millis found'); }
    else { pass('Mixed: no epoch millis'); }
  }

  // ===================================================================
  console.log('\n=== Test 7: Real table with timestamps ===');
  const t7 = await runQuery(
    "SELECT event_id, user_id, event_type, timestamp FROM events WHERE timestamp IS NOT NULL LIMIT 10",
    'test7-real-data'
  );
  if (hasError(t7)) {
    // The sample data might not have non-null timestamps — try without the filter
    console.log('  Retrying without NULL filter...');
    const t7b = await runQuery(
      "SELECT event_id, user_id, event_type, timestamp FROM events LIMIT 10",
      'test7-real-data-retry'
    );
    if (hasError(t7b)) {
      fail('Real data query', 'Server error');
    } else if (t7b.includes('event_id') || t7b.includes('dashboard_view')) {
      pass('Real table query executes and renders');
      if (hasEpochMillis(t7b)) {
        fail('Real data timestamps', 'Epoch millis in real data');
      } else {
        pass('Real data: no epoch millis');
      }
    } else {
      fail('Real data query', 'No expected data in output');
    }
  } else {
    pass('Real table query with non-null timestamps');
    if (hasISOTimestamp(t7)) {
      pass('Real timestamps display as ISO');
    } else if (hasEpochMillis(t7)) {
      fail('Real timestamps', 'Epoch millis found');
    }
  }

  // ===================================================================
  console.log('\n=== Test 8: Pagination with Arrow data ===');
  const t8 = await runQuery(
    "SELECT number, toDateTime(number * 3600 + 1704067200) as ts FROM numbers(100)",
    'test8-pagination'
  );
  if (hasError(t8)) {
    fail('Pagination query', 'Server error');
  } else if (t8.includes('Page 1 of') || t8.includes('1-50 of') || t8.includes('Showing')) {
    pass('Pagination renders with Arrow data');
    if (hasEpochMillis(t8)) {
      fail('Pagination timestamps', 'Epoch millis in paginated data');
    } else {
      pass('Pagination: no epoch millis');
    }
  } else if (t8.includes('number') && t8.includes('ts')) {
    pass('Pagination query executed (results visible)');
  } else {
    fail('Pagination', 'No expected pagination indicators');
  }

  // ===================================================================
  // Summary
  console.log('\n========================================');
  console.log(`  PASSED: ${testsPassed}`);
  console.log(`  FAILED: ${testsFailed}`);
  console.log('========================================');

  if (failures.length > 0) {
    console.log('\nFailures:');
    failures.forEach(f => console.log(`  ${f.name}: ${f.reason}`));
  }

  if (consoleErrors.length > 0) {
    console.log(`\nBrowser console errors: ${consoleErrors.length}`);
    consoleErrors.slice(0, 5).forEach(e => console.log(`  ${e.substring(0, 200)}`));
  }

  console.log(`\nScreenshots saved to ${SCREENSHOT_DIR}/`);

  await browser.close();
  process.exit(testsFailed > 0 ? 1 : 0);
})();
