// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Test font replacement in chart rendering
 *
 * Verifies that system-ui fonts are replaced with system-available fonts
 * so librsvg can render text properly.
 */

import { JSDOM } from 'jsdom';
import sharp from 'sharp';
import * as d3 from 'd3';
import { renderChart as renderChartML } from '@chartml/core';
import assert from 'assert';

/**
 * Mock SVG measurement APIs
 */
function mockSVGMeasurements(window) {
  const { SVGElement } = window;

  SVGElement.prototype.getBBox = function() {
    const text = this.textContent || '';
    const fontSize = parseFloat(this.getAttribute('font-size') || '12');
    return { x: 0, y: -fontSize * 0.75, width: text.length * fontSize * 0.6, height: fontSize };
  };

  SVGElement.prototype.getComputedTextLength = function() {
    const text = this.textContent || '';
    const fontSize = parseFloat(this.getAttribute('font-size') || '12');
    return text.length * fontSize * 0.6;
  };

  SVGElement.prototype.getTotalLength = function() {
    return 100;
  };

  SVGElement.prototype.getPointAtLength = function() {
    return { x: 0, y: 0 };
  };
}

/**
 * Render a chart and return the SVG string (before and after font replacement)
 */
async function renderChartToSVG(spec, width = 800, height = 400) {
  const dom = new JSDOM(`
    <!DOCTYPE html>
    <html>
      <body>
        <div id="chart-container"></div>
      </body>
    </html>
  `, { pretendToBeVisual: true });

  const { window } = dom;
  global.window = window;
  global.document = window.document;
  global.d3 = d3;

  mockSVGMeasurements(window);

  const container = window.document.getElementById('chart-container');
  Object.defineProperty(container, 'clientWidth', { get: () => width });
  Object.defineProperty(container, 'clientHeight', { get: () => height });
  Object.defineProperty(container, 'offsetWidth', { get: () => width });
  Object.defineProperty(container, 'offsetHeight', { get: () => height });

  try {
    await renderChartML(spec, container);
    d3.timerFlush();

    const svgElement = container.querySelector('svg');
    if (!svgElement) {
      throw new Error('No SVG element generated');
    }

    const originalSVG = svgElement.outerHTML;

    // Apply font replacement (same as server.js)
    let fixedSVG = originalSVG;
    fixedSVG = fixedSVG.replace(/font-family="system-ui"/g, 'font-family="Liberation Sans, Arial, sans-serif"');
    fixedSVG = fixedSVG.replace(/font-family: system-ui/g, 'font-family: Liberation Sans, Arial, sans-serif');

    return { originalSVG, fixedSVG };
  } finally {
    delete global.window;
    delete global.document;
    delete global.d3;
  }
}

/**
 * Test: SVG contains system-ui font before fix
 */
async function testOriginalSVGHasSystemUI() {
  console.log('Test 1: Original SVG contains system-ui font');

  const spec = {
    type: 'chart',
    version: 1,
    title: 'Test Chart',
    data: {
      provider: 'inline',
      rows: [
        { category: 'A', value: 10 },
        { category: 'B', value: 20 },
      ]
    },
    visualize: {
      type: 'bar',
      columns: 'category',
      rows: 'value'
    }
  };

  const { originalSVG } = await renderChartToSVG(spec);

  const hasSystemUI = originalSVG.includes('font-family="system-ui"') ||
                      originalSVG.includes('font-family: system-ui');

  assert.ok(hasSystemUI, 'Original SVG should contain system-ui font');
  console.log('  PASS: Original SVG contains system-ui');
}

/**
 * Test: Fixed SVG does not contain system-ui font
 */
async function testFixedSVGNoSystemUI() {
  console.log('Test 2: Fixed SVG does not contain system-ui font');

  const spec = {
    type: 'chart',
    version: 1,
    title: 'Test Chart',
    data: {
      provider: 'inline',
      rows: [
        { category: 'A', value: 10 },
        { category: 'B', value: 20 },
      ]
    },
    visualize: {
      type: 'bar',
      columns: 'category',
      rows: 'value'
    }
  };

  const { fixedSVG } = await renderChartToSVG(spec);

  const hasSystemUI = fixedSVG.includes('font-family="system-ui"') ||
                      fixedSVG.includes('font-family: system-ui');

  assert.ok(!hasSystemUI, 'Fixed SVG should NOT contain system-ui font');
  console.log('  PASS: Fixed SVG does not contain system-ui');
}

/**
 * Test: Fixed SVG contains replacement font
 */
async function testFixedSVGHasReplacementFont() {
  console.log('Test 3: Fixed SVG contains Liberation Sans replacement');

  const spec = {
    type: 'chart',
    version: 1,
    title: 'Test Chart',
    data: {
      provider: 'inline',
      rows: [
        { category: 'A', value: 10 },
        { category: 'B', value: 20 },
      ]
    },
    visualize: {
      type: 'bar',
      columns: 'category',
      rows: 'value'
    }
  };

  const { fixedSVG } = await renderChartToSVG(spec);

  const hasReplacement = fixedSVG.includes('Liberation Sans');

  assert.ok(hasReplacement, 'Fixed SVG should contain Liberation Sans font');
  console.log('  PASS: Fixed SVG contains Liberation Sans');
}

/**
 * Test: Sharp can render the fixed SVG to PNG
 */
async function testSharpCanRenderFixedSVG() {
  console.log('Test 4: Sharp can render fixed SVG to PNG');

  const spec = {
    type: 'chart',
    version: 1,
    title: 'Test Chart',
    data: {
      provider: 'inline',
      rows: [
        { category: 'A', value: 10 },
        { category: 'B', value: 20 },
      ]
    },
    visualize: {
      type: 'bar',
      columns: 'category',
      rows: 'value'
    }
  };

  const { fixedSVG } = await renderChartToSVG(spec);

  const pngBuffer = await sharp(Buffer.from(fixedSVG))
    .flatten({ background: { r: 255, g: 255, b: 255 } })
    .png()
    .toBuffer();

  assert.ok(pngBuffer.length > 1000, 'PNG should be generated with reasonable size');
  console.log(`  PASS: Generated PNG (${pngBuffer.length} bytes)`);
}

/**
 * Test: PNG has visible content (not blank)
 */
async function testPNGHasVisibleContent() {
  console.log('Test 5: PNG has visible content (not all white)');

  const spec = {
    type: 'chart',
    version: 1,
    title: 'Test Chart',
    data: {
      provider: 'inline',
      rows: [
        { category: 'A', value: 10 },
        { category: 'B', value: 20 },
      ]
    },
    visualize: {
      type: 'bar',
      columns: 'category',
      rows: 'value'
    }
  };

  const { fixedSVG } = await renderChartToSVG(spec);

  const pngBuffer = await sharp(Buffer.from(fixedSVG))
    .flatten({ background: { r: 255, g: 255, b: 255 } })
    .png()
    .toBuffer();

  // Check that PNG has some non-white pixels by analyzing raw pixel data
  const { data, info } = await sharp(pngBuffer).raw().toBuffer({ resolveWithObject: true });

  let nonWhitePixels = 0;
  for (let i = 0; i < data.length; i += info.channels) {
    const r = data[i];
    const g = data[i + 1];
    const b = data[i + 2];
    if (r !== 255 || g !== 255 || b !== 255) {
      nonWhitePixels++;
    }
  }

  const totalPixels = info.width * info.height;
  const nonWhitePercent = (nonWhitePixels / totalPixels) * 100;

  // Chart should have at least 1% non-white pixels (bars, text, axes)
  assert.ok(nonWhitePercent > 1, `PNG should have visible content, got ${nonWhitePercent.toFixed(2)}% non-white`);
  console.log(`  PASS: PNG has ${nonWhitePercent.toFixed(2)}% non-white pixels`);
}

/**
 * Run all tests
 */
async function runTests() {
  console.log('\n========================================');
  console.log('Chart Renderer Font Tests');
  console.log('========================================\n');

  try {
    await testOriginalSVGHasSystemUI();
    await testFixedSVGNoSystemUI();
    await testFixedSVGHasReplacementFont();
    await testSharpCanRenderFixedSVG();
    await testPNGHasVisibleContent();

    console.log('\n========================================');
    console.log('All tests passed!');
    console.log('========================================\n');
    process.exit(0);
  } catch (error) {
    console.error('\n========================================');
    console.error('TEST FAILED:', error.message);
    console.error('========================================\n');
    process.exit(1);
  }
}

runTests();
