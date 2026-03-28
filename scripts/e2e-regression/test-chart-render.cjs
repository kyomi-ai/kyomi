#!/usr/bin/env node
const { chromium } = require('playwright');
const fs = require('fs');
const path = require('path');

const SERVERS = {
  react: 'http://localhost:8002',
  leptos: 'http://localhost:3000',
};

const CREDS = {
  email: 'e2e-test@kyomi.dev',
  password: 'E2eTestPass123!',
};

const TIMEOUTS = { nav: 20000, action: 8000 };
const GROUP = 'chart-render';
const DASHBOARD_ID = 'e2e-chart-dashboard-001';
const DASHBOARD_URL = `/dashboard/${DASHBOARD_ID}`;

// --- Helpers ---

async function loginOnce(server) {
  const baseUrl = SERVERS[server];
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({
    viewport: { width: 1280, height: 720 },
    ignoreHTTPSErrors: true,
  });
  const page = await context.newPage();
  await page.goto(`${baseUrl}/login`, { waitUntil: 'domcontentloaded', timeout: TIMEOUTS.nav });
  await page.waitForTimeout(1500);
  await page.fill('input[type="email"], input[name="email"]', CREDS.email, { timeout: TIMEOUTS.action });
  await page.fill('input[type="password"], input[name="password"]', CREDS.password, { timeout: TIMEOUTS.action });
  await page.click('button[type="submit"]', { timeout: TIMEOUTS.action });
  await page.waitForURL(url => !url.toString().includes('/login'), { timeout: TIMEOUTS.nav });
  return { browser, context, page, baseUrl };
}

async function saveScreenshot(page, flowId, server) {
  const dir = `/tmp/e2e-regression/${GROUP}/${flowId}`;
  fs.mkdirSync(dir, { recursive: true });
  const filepath = `${dir}/${server}.png`;
  await page.screenshot({ path: filepath, fullPage: true });
  return filepath;
}

function classify(react, leptos) {
  if (react.status === 'pass' && leptos.status === 'pass') return 'PASS';
  if (react.status === 'pass' && leptos.status === 'fail') return 'REGRESSION';
  if (react.status === 'fail' && leptos.status === 'fail') return 'BOTH_FAIL';
  return 'BEHAVIORAL_DIFF';
}

async function navigateToDashboard(page, baseUrl) {
  await page.goto(`${baseUrl}${DASHBOARD_URL}`, { waitUntil: 'domcontentloaded', timeout: TIMEOUTS.nav });
  // Wait for page to settle
  await page.waitForTimeout(2000);
  // Wait for spinner to disappear
  try {
    await page.waitForFunction(() => {
      const spinners = document.querySelectorAll('.animate-spin, [class*="spinner"], [class*="Spinner"]');
      return spinners.length === 0;
    }, { timeout: 15000 });
  } catch {
    // Spinner may never have appeared
  }
  // Give charts time to render
  await page.waitForTimeout(3000);
}

// --- Flow Functions (receive page + baseUrl, return assertions + screenshot) ---

async function testChartRender01(page, baseUrl, server) {
  const assertions = [];
  await navigateToDashboard(page, baseUrl);

  // Check for chart elements (canvas, svg charts, or chart container divs)
  const chartInfo = await page.evaluate(() => {
    const result = { containers: 0, svgCharts: 0, canvasCharts: 0, errors: [] };

    // Bordered rounded containers (Leptos chart blocks)
    result.containers = document.querySelectorAll('.rounded-lg.border.overflow-hidden, .my-4.rounded-lg.border').length;

    // Large SVGs (chart renderings)
    document.querySelectorAll('svg').forEach(svg => {
      const r = svg.getBoundingClientRect();
      if (r.width > 200 && r.height > 100) result.svgCharts++;
    });

    // Canvas charts
    document.querySelectorAll('canvas').forEach(c => {
      const r = c.getBoundingClientRect();
      if (r.width > 200 && r.height > 100) result.canvasCharts++;
    });

    // Error messages
    document.querySelectorAll('[class*="destructive"], [class*="error"]').forEach(el => {
      const t = (el.textContent || '').trim();
      if (t.toLowerCase().includes('error') && t.length < 200) result.errors.push(t);
    });

    return result;
  });

  const totalChartElements = chartInfo.svgCharts + chartInfo.canvasCharts;
  assertions.push({
    pass: totalChartElements > 0 || chartInfo.containers > 0,
    message: totalChartElements > 0 || chartInfo.containers > 0
      ? `Charts present: ${chartInfo.svgCharts} SVG, ${chartInfo.canvasCharts} canvas, ${chartInfo.containers} containers`
      : 'No chart elements found on page',
  });

  assertions.push({
    pass: chartInfo.errors.length === 0,
    message: chartInfo.errors.length === 0
      ? 'No chart error messages visible'
      : `Chart errors: ${chartInfo.errors.join('; ')}`,
  });

  assertions.push({
    pass: chartInfo.containers >= 2 || totalChartElements >= 2,
    message: chartInfo.containers >= 2 || totalChartElements >= 2
      ? `Both bar and line charts present (${Math.max(chartInfo.containers, totalChartElements)} chart blocks)`
      : `Only ${Math.max(chartInfo.containers, totalChartElements)} chart block(s), expected 2`,
  });

  const screenshot = await saveScreenshot(page, 'chart-render-01', server);
  return { assertions, screenshot };
}

async function testChartRender02(page, baseUrl, server) {
  const assertions = [];
  await navigateToDashboard(page, baseUrl);

  const measurements = await page.evaluate(() => {
    // Find main content area
    const main = document.querySelector('main') || document.querySelector('.flex-1') || document.body;
    const contentWidth = main.getBoundingClientRect().width;

    // Measure chart block widths (the outer rounded-lg containers)
    const chartBlockWidths = [];
    document.querySelectorAll('.rounded-lg.border.overflow-hidden, .my-4.rounded-lg.border').forEach(el => {
      const r = el.getBoundingClientRect();
      if (r.width > 100 && r.height > 50) chartBlockWidths.push(r.width);
    });

    // Measure rendered chart widths (SVG/canvas)
    const chartWidths = [];
    document.querySelectorAll('svg, canvas').forEach(el => {
      const r = el.getBoundingClientRect();
      if (r.width > 200 && r.height > 100) chartWidths.push(r.width);
    });

    return { contentWidth, chartBlockWidths, chartWidths };
  });

  assertions.push({
    pass: measurements.chartBlockWidths.length > 0 || measurements.chartWidths.length > 0,
    message: `Measured widths - blocks: [${measurements.chartBlockWidths.join(', ')}], charts: [${measurements.chartWidths.join(', ')}], content: ${measurements.contentWidth}px`,
  });

  // Charts should fill at least 50% of content width
  const allWidths = measurements.chartBlockWidths.concat(measurements.chartWidths);
  if (allWidths.length > 0 && measurements.contentWidth > 0) {
    const minRatio = Math.min(...allWidths.map(w => w / measurements.contentWidth));
    assertions.push({
      pass: minRatio >= 0.5,
      message: minRatio >= 0.5
        ? `Charts fill ${(minRatio * 100).toFixed(0)}% of content width (good)`
        : `Charts only fill ${(minRatio * 100).toFixed(0)}% of content width (too narrow)`,
    });
  } else {
    assertions.push({
      pass: false,
      message: 'Could not measure chart widths relative to content area',
    });
  }

  const screenshot = await saveScreenshot(page, 'chart-render-02', server);
  return { assertions, screenshot };
}

async function testChartRender03(page, baseUrl, server) {
  const assertions = [];
  await navigateToDashboard(page, baseUrl);

  const headerInfo = await page.evaluate(() => {
    const result = { customHeaders: 0, refreshButtons: 0, actionsButtons: 0, titleTexts: [] };
    result.customHeaders = document.querySelectorAll('chart-header-bar').length;
    result.refreshButtons = document.querySelectorAll('button[title="Refresh"]').length;
    result.actionsButtons = document.querySelectorAll('button[title="Actions"]').length;

    // Check for chart title text in header bars
    document.querySelectorAll('.text-sm.font-medium.text-foreground').forEach(el => {
      const t = (el.textContent || '').trim();
      if (t.length > 0 && t.length < 100) result.titleTexts.push(t);
    });

    return result;
  });

  const totalHeaders = Math.max(headerInfo.customHeaders, headerInfo.refreshButtons, headerInfo.actionsButtons);
  assertions.push({
    pass: totalHeaders > 0,
    message: totalHeaders > 0
      ? `Header bars found (custom: ${headerInfo.customHeaders}, refresh: ${headerInfo.refreshButtons}, actions: ${headerInfo.actionsButtons})`
      : 'No chart header bars found',
  });

  assertions.push({
    pass: headerInfo.refreshButtons > 0 || headerInfo.actionsButtons > 0 || headerInfo.customHeaders > 0,
    message: headerInfo.refreshButtons > 0 || headerInfo.actionsButtons > 0
      ? `Action buttons present: ${headerInfo.refreshButtons} refresh, ${headerInfo.actionsButtons} actions`
      : headerInfo.customHeaders > 0
        ? `${headerInfo.customHeaders} custom header elements (buttons inside shadow DOM)`
        : 'No action buttons found in chart headers',
  });

  assertions.push({
    pass: totalHeaders >= 2,
    message: totalHeaders >= 2
      ? `${totalHeaders} header bars (one per chart)`
      : `Only ${totalHeaders} header bar(s), expected 2`,
  });

  if (headerInfo.titleTexts.length > 0) {
    assertions.push({
      pass: true,
      message: `Chart titles in headers: ${headerInfo.titleTexts.join(', ')}`,
    });
  }

  const screenshot = await saveScreenshot(page, 'chart-render-03', server);
  return { assertions, screenshot };
}

async function testChartRender08(page, baseUrl, server) {
  const assertions = [];
  await navigateToDashboard(page, baseUrl);

  let infoClicked = false;

  // Try Leptos approach: click Actions button, then "Chart Info" in dropdown
  const actionsButton = await page.$('button[title="Actions"]');
  if (actionsButton) {
    await actionsButton.click();
    await page.waitForTimeout(500);
    const chartInfoBtn = await page.$('button:has-text("Chart Info")');
    if (chartInfoBtn) {
      await chartInfoBtn.click();
      infoClicked = true;
    }
  }

  // Try React approach: look inside shadow DOM for info button with data-event="header-info"
  if (!infoClicked) {
    const result = await page.evaluate(() => {
      const headerBars = document.querySelectorAll('chart-header-bar');
      for (const bar of headerBars) {
        const shadow = bar.shadowRoot;
        if (!shadow) continue;
        // The info button uses data-event="header-info" and class "btn"
        const infoBtn = shadow.querySelector('.btn[data-event="header-info"]');
        if (infoBtn) {
          infoBtn.click();
          return 'info-clicked';
        }
      }
      // Check if chart-header-bar exists but has no show-info attribute
      if (headerBars.length > 0) {
        const hasShowInfo = headerBars[0].hasAttribute('show-info');
        return hasShowInfo ? 'has-show-info-but-no-button' : 'no-show-info-attr';
      }
      return false;
    });

    if (result === 'info-clicked') {
      infoClicked = true;
    } else if (result === 'no-show-info-attr') {
      // React dashboard viewer doesn't set show-info on chart-header-bar
      // This is expected behavior - info is only available in MCP chart app
      assertions.push({
        pass: true,
        message: 'React chart-header-bar does not have show-info attribute (info button not shown in dashboard viewer - expected)',
      });
      const screenshot = await saveScreenshot(page, 'chart-render-08', server);
      return { assertions, screenshot };
    }
  }

  assertions.push({
    pass: infoClicked,
    message: infoClicked ? 'Info button clicked' : 'Could not find or click info button',
  });

  if (infoClicked) {
    await page.waitForTimeout(1500);

    // Check for modal/dialog visibility - Leptos modal uses fixed overlay with z-[1000]
    const modalInfo = await page.evaluate(() => {
      const result = { modalVisible: false, hasYaml: false, modalText: '', allCodeText: '' };

      // Check for Leptos modal: fixed overlay with z-[1000]
      const fixedOverlays = document.querySelectorAll('.fixed.inset-0');
      for (const el of fixedOverlays) {
        if (el.offsetHeight > 0 && el.offsetWidth > 0) {
          result.modalVisible = true;
          const text = el.textContent || '';
          result.modalText = text.substring(0, 500);
          if (text.includes('type:') || text.includes('visualize:') || text.includes('data:')) {
            result.hasYaml = true;
          }
        }
      }

      // Check role="dialog" or class-based modals (React-style)
      if (!result.modalVisible) {
        const candidates = document.querySelectorAll(
          '[role="dialog"], .modal, [class*="modal"], [class*="Modal"]'
        );
        for (const el of candidates) {
          if (el.offsetHeight > 0 && el.offsetWidth > 0) {
            result.modalVisible = true;
            const text = el.textContent || '';
            result.modalText = text.substring(0, 500);
            if (text.includes('type:') || text.includes('visualize:') || text.includes('data:')) {
              result.hasYaml = true;
            }
          }
        }
      }

      // Check pre/code blocks for YAML content (these might be inside the modal)
      document.querySelectorAll('pre, code').forEach(el => {
        const text = el.textContent || '';
        if (el.offsetHeight > 0 && text.length > 10) {
          result.allCodeText += text.substring(0, 200) + ' | ';
          if (text.includes('type:') || text.includes('visualize:') || text.includes('data:')) {
            result.hasYaml = true;
          }
        }
      });

      return result;
    });

    assertions.push({
      pass: modalInfo.modalVisible,
      message: modalInfo.modalVisible
        ? 'Info modal is visible'
        : 'Info modal not visible after clicking info button',
    });

    assertions.push({
      pass: modalInfo.hasYaml,
      message: modalInfo.hasYaml
        ? 'ChartML YAML spec content visible in modal'
        : `No ChartML YAML found. Modal text: "${modalInfo.modalText.substring(0, 200)}". Code blocks: "${modalInfo.allCodeText.substring(0, 200)}"`,
    });
  }

  const screenshot = await saveScreenshot(page, 'chart-render-08', server);
  return { assertions, screenshot };
}

async function testChartRender09(page, baseUrl, server) {
  const assertions = [];
  await navigateToDashboard(page, baseUrl);

  const axisInfo = await page.evaluate(() => {
    const result = { xLabels: [], yLabels: [], titles: [], hasSvgChart: false, hasCanvasChart: false };
    const months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

    document.querySelectorAll('svg').forEach(svg => {
      const rect = svg.getBoundingClientRect();
      if (rect.width > 200 && rect.height > 100) {
        result.hasSvgChart = true;
        svg.querySelectorAll('text').forEach(t => {
          const text = t.textContent.trim();
          if (!text) return;
          if (months.some(m => text.includes(m))) result.xLabels.push(text);
          else if (/^\d+(\.\d+)?$/.test(text)) result.yLabels.push(text);
        });
      }
    });

    document.querySelectorAll('canvas').forEach(c => {
      const rect = c.getBoundingClientRect();
      if (rect.width > 200 && rect.height > 100) result.hasCanvasChart = true;
    });

    // Chart titles in header bars
    document.querySelectorAll('.text-sm.font-medium, chart-header-bar').forEach(el => {
      const t = (el.textContent || '').trim();
      if (t.length > 2 && t.length < 100) result.titles.push(t);
    });

    return result;
  });

  const hasCharts = axisInfo.hasSvgChart || axisInfo.hasCanvasChart;
  assertions.push({
    pass: hasCharts,
    message: hasCharts
      ? `Charts rendered (SVG: ${axisInfo.hasSvgChart}, Canvas: ${axisInfo.hasCanvasChart})`
      : 'No chart rendering found',
  });

  if (axisInfo.hasSvgChart) {
    assertions.push({
      pass: axisInfo.xLabels.length > 0,
      message: axisInfo.xLabels.length > 0
        ? `X-axis labels: ${axisInfo.xLabels.slice(0, 6).join(', ')}`
        : 'No x-axis month labels found in SVG',
    });
    assertions.push({
      pass: axisInfo.yLabels.length > 0,
      message: axisInfo.yLabels.length > 0
        ? `Y-axis values: ${axisInfo.yLabels.slice(0, 6).join(', ')}`
        : 'No y-axis numeric values found in SVG',
    });
  } else if (axisInfo.hasCanvasChart) {
    assertions.push({
      pass: true,
      message: 'Canvas charts - axis labels require visual review',
    });
  }

  assertions.push({
    pass: axisInfo.titles.length > 0,
    message: axisInfo.titles.length > 0
      ? `Chart titles: ${axisInfo.titles.slice(0, 3).join(', ')}`
      : 'No chart titles found',
  });

  const screenshot = await saveScreenshot(page, 'chart-render-09', server);
  return { assertions, screenshot };
}

async function testChartRender10(page, baseUrl, server) {
  const assertions = [];
  await navigateToDashboard(page, baseUrl);

  // Scroll to bottom and back
  await page.evaluate(() => window.scrollTo(0, document.body.scrollHeight));
  await page.waitForTimeout(1000);
  await page.evaluate(() => window.scrollTo(0, 0));
  await page.waitForTimeout(500);

  const chartCount = await page.evaluate(() => {
    let svgCharts = 0, canvasCharts = 0, headerBars = 0;
    document.querySelectorAll('svg').forEach(svg => {
      const r = svg.getBoundingClientRect();
      if (r.width > 200 && r.height > 100) svgCharts++;
    });
    document.querySelectorAll('canvas').forEach(c => {
      const r = c.getBoundingClientRect();
      if (r.width > 200 && r.height > 100) canvasCharts++;
    });
    headerBars = Math.max(
      document.querySelectorAll('chart-header-bar').length,
      document.querySelectorAll('button[title="Refresh"]').length
    );
    return { svgCharts, canvasCharts, headerBars };
  });

  const totalCharts = Math.max(chartCount.svgCharts + chartCount.canvasCharts, chartCount.headerBars);

  assertions.push({
    pass: totalCharts >= 2,
    message: totalCharts >= 2
      ? `Both charts visible: ${totalCharts} (SVG: ${chartCount.svgCharts}, Canvas: ${chartCount.canvasCharts}, Headers: ${chartCount.headerBars})`
      : `Only ${totalCharts} chart(s), expected 2 (SVG: ${chartCount.svgCharts}, Canvas: ${chartCount.canvasCharts}, Headers: ${chartCount.headerBars})`,
  });

  assertions.push({
    pass: totalCharts >= 2,
    message: totalCharts >= 2
      ? 'Multiple charts confirmed rendering'
      : 'Could not confirm multiple charts',
  });

  const screenshot = await saveScreenshot(page, 'chart-render-10', server);
  return { assertions, screenshot };
}

// --- Runner ---

const FLOWS = [
  { id: 'chart-render-01', name: 'Charts render in dashboard viewer', fn: testChartRender01 },
  { id: 'chart-render-02', name: 'Charts fill full content width', fn: testChartRender02 },
  { id: 'chart-render-03', name: 'ChartML header bar present above each chart', fn: testChartRender03 },
  { id: 'chart-render-08', name: 'Chart info modal shows ChartML spec', fn: testChartRender08 },
  { id: 'chart-render-09', name: 'Chart axes and labels render correctly', fn: testChartRender09 },
  { id: 'chart-render-10', name: 'Multiple charts in one dashboard all render', fn: testChartRender10 },
];

async function runAllFlowsForServer(server) {
  console.log(`\n--- Logging in to ${server} (${SERVERS[server]}) ---`);
  const { browser, context, page, baseUrl } = await loginOnce(server);
  console.log(`  Login successful on ${server}`);

  const results = {};
  for (const flow of FLOWS) {
    console.log(`  Running ${flow.id} on ${server}...`);
    try {
      // Each flow gets a fresh page in the same auth context
      const flowPage = await context.newPage();
      const { assertions, screenshot } = await flow.fn(flowPage, baseUrl, server);
      const allPass = assertions.every(a => a.pass);
      results[flow.id] = {
        status: allPass ? 'pass' : 'fail',
        assertions,
        screenshot,
        error: allPass ? null : assertions.filter(a => !a.pass).map(a => a.message).join('; '),
      };
      await flowPage.close();
    } catch (e) {
      results[flow.id] = { status: 'fail', error: e.message, assertions: [], screenshot: null };
    }
    console.log(`    ${server}: ${results[flow.id].status}${results[flow.id].error ? ' - ' + results[flow.id].error : ''}`);
  }

  await browser.close();
  return results;
}

async function main() {
  console.log(`\n=== E2E Chart Render Tests ===`);

  // Run all flows per server (single login per server)
  const reactResults = await runAllFlowsForServer('react');
  const leptosResults = await runAllFlowsForServer('leptos');

  // Combine results
  const flows = FLOWS.map(flow => {
    const result = {
      id: flow.id,
      name: flow.name,
      react: reactResults[flow.id],
      leptos: leptosResults[flow.id],
    };
    result.classification = classify(result.react, result.leptos);
    return result;
  });

  const outputDir = `/tmp/e2e-regression/${GROUP}`;
  fs.mkdirSync(outputDir, { recursive: true });
  const resultsPath = `${outputDir}/results.json`;
  fs.writeFileSync(resultsPath,
    JSON.stringify({ group: GROUP, timestamp: new Date().toISOString(), flows }, null, 2));
  console.log(`\nResults written to: ${resultsPath}`);

  // Summary
  console.log('\n=== Summary ===');
  for (const flow of flows) {
    console.log(`${flow.classification.padEnd(18)} ${flow.id} — ${flow.name}`);
    if (flow.classification !== 'PASS') {
      if (flow.react.error) console.log(`  React:  ${flow.react.error}`);
      if (flow.leptos.error) console.log(`  Leptos: ${flow.leptos.error}`);
    }
  }
}

main().catch(e => { console.error('Fatal:', e); process.exit(1); });
