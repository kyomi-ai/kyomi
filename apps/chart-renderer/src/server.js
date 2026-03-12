// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * ChartML Renderer Service
 *
 * Node.js service that renders ChartML specifications to PNG images
 * using JSDOM (no browser required).
 *
 * Architecture:
 * 1. Python backend passes ChartML spec + inline data
 * 2. JSDOM provides DOM environment for D3/ChartML
 * 3. node-canvas provides SVG measurement APIs (getBBox, getComputedTextLength)
 * 4. ChartML renders SVG
 * 5. sharp converts SVG → PNG
 * 6. Returns base64-encoded PNG
 */

import express from 'express';
import { JSDOM } from 'jsdom';
import sharp from 'sharp';
import yaml from 'js-yaml';
import { ChartML } from '@chartml/core';
import { createPieChartRenderer } from '@chartml/chart-pie';
import { createScatterPlotRenderer } from '@chartml/chart-scatter';
import * as d3 from 'd3';
import { aggregateMiddleware, executeTransform } from './aggregateMiddleware.js';
import { execFileSync } from 'node:child_process';
import { writeFileSync, readFileSync, unlinkSync, mkdtempSync, rmdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { createRequire } from 'node:module';

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);

/**
 * Build a CSS custom property resolver from @chartml/core's stylesheet.
 *
 * JSDOM has no CSS engine and sharp/librsvg doesn't support CSS custom
 * properties. When chartml sets `style="stroke: var(--chartml-grid)"`,
 * the var() reference passes through unresolved and librsvg falls back
 * to black. We parse the :root light-mode values from chartml's own CSS
 * at startup and replace var() references in the SVG before rasterization.
 */
function buildCssVariableResolver() {
  const cssPath = require.resolve('@chartml/core/style.css');
  const css = readFileSync(cssPath, 'utf-8');

  // Extract the first :root block (light-mode defaults)
  const rootMatch = css.match(/:root\s*\{([^}]+)\}/);
  if (!rootMatch) {
    console.warn('Could not parse :root block from chartml CSS — var() references will not be resolved');
    return (svg) => svg;
  }

  // Parse --name: value pairs
  const varMap = new Map();
  for (const line of rootMatch[1].split('\n')) {
    const m = line.match(/\s*(--[\w-]+)\s*:\s*(.+?)\s*;/);
    if (m) varMap.set(m[1], m[2]);
  }

  console.log(`Loaded ${varMap.size} CSS custom properties from @chartml/core`);

  // Replace var(--name) and var(--name, fallback) with resolved values
  return (svg) => svg.replace(/var\((--[\w-]+)(?:\s*,\s*([^)]+))?\)/g, (match, name, fallback) => {
    return varMap.get(name) || fallback || match;
  });
}

const resolveCssVariables = buildCssVariableResolver();

const app = express();
app.use(express.json({ limit: '10mb' }));

const PORT = process.env.CHART_RENDERER_PORT || 3030;

/**
 * Mock SVG measurement APIs using simple approximations
 *
 * ChartML uses getBBox() and getComputedTextLength() to calculate layout.
 * JSDOM doesn't implement these (no layout engine), so we mock them
 * with approximate calculations.
 *
 * NOTE: For POC only - measurements won't be pixel-perfect.
 * If POC succeeds, we can improve accuracy with node-canvas or browser.
 */
function mockSVGMeasurements(window) {
  const { SVGElement } = window;

  if (!SVGElement) {
    throw new Error('SVGElement not found in JSDOM window');
  }

  // Mock getBBox() - returns bounding box of SVG element
  SVGElement.prototype.getBBox = function() {
    const text = this.textContent || '';
    const tagName = this.tagName?.toLowerCase();

    // Get font styles from element
    const fontSize = this.getAttribute('font-size') ||
                     this.style?.fontSize ||
                     '12px';

    // Extract numeric font size
    const fontSizeNum = parseFloat(fontSize) || 12;

    // For non-text elements, try to use actual dimensions
    if (tagName === 'rect' || tagName === 'g' || tagName === 'svg') {
      const w = parseFloat(this.getAttribute('width')) || 100;
      const h = parseFloat(this.getAttribute('height')) || 100;
      return { x: 0, y: 0, width: w, height: h };
    }

    // Approximate width: ~0.6 * font size per character (monospace-ish)
    const approxWidth = text.length * fontSizeNum * 0.6;

    return {
      x: 0,
      y: -fontSizeNum * 0.75, // Approximate baseline offset
      width: approxWidth,
      height: fontSizeNum,
      top: -fontSizeNum * 0.75,
      right: approxWidth,
      bottom: fontSizeNum * 0.25,
      left: 0
    };
  };

  // Mock getComputedTextLength() - returns width of text
  SVGElement.prototype.getComputedTextLength = function() {
    const text = this.textContent || '';
    const fontSize = this.getAttribute('font-size') ||
                     this.style?.fontSize ||
                     '12px';

    const fontSizeNum = parseFloat(fontSize) || 12;

    // Approximate width
    return text.length * fontSizeNum * 0.6;
  };

  // Mock getTotalLength() - returns length of path (used for line chart animations)
  SVGElement.prototype.getTotalLength = function() {
    // For path elements, estimate length from d attribute
    const d = this.getAttribute('d') || '';
    // Very rough approximation: count path commands and estimate
    const commands = d.match(/[MLHVCSQTAZ]/gi) || [];
    // Assume average segment length of 50px
    return commands.length * 50;
  };

  // Mock getPointAtLength() - returns point on path at given length
  SVGElement.prototype.getPointAtLength = function(length) {
    return { x: length, y: 0 };
  };
}

// Render queue — serializes chart renders to prevent global state conflicts.
// ChartML/D3 require global.document, global.window, global.d3 which can't be
// shared across concurrent renders. Each render sets up and tears down these
// globals, so concurrent requests would stomp on each other.
let renderQueue = Promise.resolve();

function enqueueRender(chartMLSpec, width, height, defaultPalette, density) {
  const result = renderQueue.then(() => renderChart(chartMLSpec, width, height, defaultPalette, density));
  // Update queue to wait for this render (whether it succeeds or fails)
  renderQueue = result.catch(() => {});
  return result;
}

/**
 * Render ChartML specification to PNG image
 */
async function renderChart(chartMLSpec, width = 800, height = 600, defaultPalette = null, density = 72) {
  // ChartML accepts either YAML string or object
  // Pass it through as-is (renderChartML will parse if needed)
  const spec = chartMLSpec;

  // Create JSDOM environment
  const dom = new JSDOM(`
    <!DOCTYPE html>
    <html>
      <head>
        <style>
          body { margin: 0; padding: 0; }
          #chart-container {
            width: ${width}px;
            height: ${height}px;
          }
        </style>
      </head>
      <body>
        <div id="chart-container"></div>
      </body>
    </html>
  `, {
    pretendToBeVisual: true,
    resources: 'usable'
  });

  const { window } = dom;

  // Set up global environment for D3/ChartML
  global.window = window;
  global.document = window.document;
  // Note: Don't set global.navigator - JSDOM's navigator is read-only

  // Mock SVG measurement APIs
  mockSVGMeasurements(window);

  // Set up global environment with D3
  global.d3 = d3;

  try {
    // Get container
    const container = window.document.getElementById('chart-container');

    // JSDOM doesn't calculate layout, so clientWidth/Height return 0
    // Patch these properties to return the actual dimensions we want
    Object.defineProperty(container, 'clientWidth', { get: () => width, configurable: true });
    Object.defineProperty(container, 'clientHeight', { get: () => height, configurable: true });
    Object.defineProperty(container, 'offsetWidth', { get: () => width, configurable: true });
    Object.defineProperty(container, 'offsetHeight', { get: () => height, configurable: true });

    // Create ChartML instance with user's default palette (if provided)
    // Disable animations for server-side rendering (charts render immediately without waiting)
    const chartMLOptions = {
      ...(defaultPalette ? { defaultPalette } : {}),
      animation: false
    };
    const chartml = new ChartML(chartMLOptions);

    // Register chart renderers that are plugins (not built into @chartml/core)
    chartml.registerChartRenderer('pie', createPieChartRenderer());
    chartml.registerChartRenderer('doughnut', createPieChartRenderer());
    chartml.registerChartRenderer('scatter', createScatterPlotRenderer());

    // Register DuckDB transform middleware for named sources with transform pipeline
    chartml.setTransformMiddleware(aggregateMiddleware);

    try {
      await chartml.render(spec, container);
    } catch (error) {
      throw new Error(`ChartML render failed: ${error.message}`);
    }

    // No animation wait needed - charts render instantly with animation: false
    d3.timerFlush();  // Flush any pending timers (defensive, shouldn't be any)

    // Extract SVG string
    const svgElement = container.querySelector('svg');
    if (!svgElement) {
      throw new Error('No SVG element generated');
    }

    // Check for title div (ChartML renders title as a div above the SVG)
    // We need to add the title to the SVG since we only export the SVG element
    const titleDiv = container.querySelector('.chart-title');
    const titleText = titleDiv?.textContent?.trim();

    if (titleText) {
      // Get current SVG dimensions from viewBox (preferred) or attributes
      const viewBox = svgElement.getAttribute('viewBox');
      let vbX = 0, vbY = 0, vbWidth, vbHeight;

      if (viewBox) {
        const parts = viewBox.split(/[\s,]+/).map(parseFloat);
        if (parts.length === 4) {
          [vbX, vbY, vbWidth, vbHeight] = parts;
        }
      }

      // Fall back to SVG element dimensions if no viewBox
      const svgWidth = vbWidth || parseFloat(svgElement.getAttribute('width')) || width;
      const svgHeight = vbHeight || parseFloat(svgElement.getAttribute('height')) || height;

      // Title styling matches ChartML: 16px font, 600 weight, 8px margin-bottom
      const titleHeight = 32; // 16px font + line-height + margin
      const titleFontSize = 16;

      // Create a new SVG text element for the title
      // Position title left-aligned to match web app styling
      const titleSvgElement = window.document.createElementNS('http://www.w3.org/2000/svg', 'text');
      titleSvgElement.setAttribute('x', vbX + 8);  // Small left margin
      titleSvgElement.setAttribute('y', vbY + titleFontSize + 8); // Baseline position with padding
      titleSvgElement.setAttribute('text-anchor', 'start');  // Left-aligned
      titleSvgElement.setAttribute('font-family', 'Liberation Sans, Arial, sans-serif');
      titleSvgElement.setAttribute('font-size', `${titleFontSize}px`);
      titleSvgElement.setAttribute('font-weight', '600');
      titleSvgElement.setAttribute('fill', '#1f2937');
      titleSvgElement.textContent = titleText;

      // Create a group to hold existing SVG content and shift it down
      const existingContent = svgElement.innerHTML;
      svgElement.innerHTML = '';

      // Add title
      svgElement.appendChild(titleSvgElement);

      // Create a group for existing content, shifted down to make room for title
      const contentGroup = window.document.createElementNS('http://www.w3.org/2000/svg', 'g');
      contentGroup.setAttribute('transform', `translate(0, ${titleHeight})`);
      contentGroup.innerHTML = existingContent;
      svgElement.appendChild(contentGroup);

      // Update SVG height to accommodate title
      const newSvgHeight = parseFloat(svgElement.getAttribute('height') || height) + titleHeight;
      svgElement.setAttribute('height', newSvgHeight);

      // Update viewBox if present
      if (viewBox) {
        const newViewBox = `${vbX} ${vbY} ${svgWidth} ${svgHeight + titleHeight}`;
        svgElement.setAttribute('viewBox', newViewBox);
      }

      console.log(`Added title to chart: "${titleText}"`);
    }

    let svgString = svgElement.outerHTML;

    // Replace system-ui font with a font that librsvg can render
    // system-ui is a CSS generic font that librsvg doesn't understand
    svgString = svgString.replace(/font-family="system-ui"/g, 'font-family="Liberation Sans, Arial, sans-serif"');
    svgString = svgString.replace(/font-family: system-ui/g, 'font-family: Liberation Sans, Arial, sans-serif');

    // Resolve var(--chartml-*) to concrete light-mode values for librsvg
    svgString = resolveCssVariables(svgString);

    // Debug: log SVG to see font-family usage
    if (process.env.DEBUG_SVG) {
      console.log('Generated SVG:', svgString.substring(0, 2000));
    }

    // Convert SVG to PNG using sharp
    // Use flatten to add white background (SVG has transparent background by default)
    // Add padding to give axis labels breathing room
    // density controls SVG rasterization DPI (default 72).
    // Higher density = more pixels for the same SVG dimensions = crisper output.
    // PDF export uses density=144 (2x) for sharp text and lines.
    const pngBuffer = await sharp(Buffer.from(svgString), { density })
      .flatten({ background: { r: 255, g: 255, b: 255 } })
      .extend({
        top: 16,
        bottom: 16,
        left: 16,
        right: 16,
        background: { r: 255, g: 255, b: 255 }
      })
      .png()
      .toBuffer();

    return pngBuffer;
  } finally {
    // CRITICAL: Clean up global state even if rendering fails
    // This prevents global variable leaks between requests
    delete global.window;
    delete global.document;
    delete global.navigator;
    delete global.d3;
  }
}

/**
 * Health check endpoint
 */
app.get('/health', (req, res) => {
  res.json({
    status: 'ok',
    service: 'chart-renderer',
    version: '0.1.0'
  });
});

/**
 * Render endpoint
 *
 * POST /render
 * Body: {
 *   chartMLSpec: string | object,
 *   width?: number,
 *   height?: number
 * }
 *
 * Returns: {
 *   image: string (base64-encoded PNG),
 *   format: 'png',
 *   width: number,
 *   height: number
 * }
 */
app.post('/render', async (req, res) => {
  const { chartMLSpec, width = 800, height = 600, defaultPalette = null, density = 72 } = req.body;

  if (!chartMLSpec) {
    return res.status(400).json({
      error: 'chartMLSpec is required'
    });
  }

  try {
    console.log(`Rendering chart: ${width}x${height}`);
    if (defaultPalette) {
      console.log(`Using custom palette with ${defaultPalette.length} colors`);
    }

    const pngBuffer = await enqueueRender(chartMLSpec, width, height, defaultPalette, density);
    const base64Image = pngBuffer.toString('base64');

    res.json({
      image: base64Image,
      format: 'png',
      width,
      height
    });

  } catch (error) {
    console.error('Render error:', error);
    res.status(500).json({
      error: error.message,
      stack: process.env.NODE_ENV === 'development' ? error.stack : undefined
    });
  }
});

/**
 * Transform endpoint
 *
 * POST /transform
 * Body: {
 *   data: { sourceName: { rows: [...] }, ... },
 *   transform: { sql?, aggregate?, forecast? }
 * }
 *
 * Returns: {
 *   data: { provider: "inline", rows: [...] },
 *   metadata: { ... }
 * }
 *
 * Runs the DuckDB transform pipeline on named source data and returns
 * resolved inline data. Used by the Python backend to pre-resolve transforms
 * for clients that don't have DuckDB (e.g. MCP App in Claude.ai).
 */
app.post('/transform', async (req, res) => {
  const { data, transform } = req.body;

  if (!data || typeof data !== 'object') {
    return res.status(400).json({
      error: 'data is required and must be an object of named sources'
    });
  }

  if (!transform || typeof transform !== 'object') {
    return res.status(400).json({
      error: 'transform is required and must be an object with sql, aggregate, or forecast stages'
    });
  }

  try {
    console.log(`Transform: ${Object.keys(data).length} source(s), stages: ${Object.keys(transform).join(', ')}`);

    const result = await executeTransform(data, transform);

    res.json({
      data: {
        provider: 'inline',
        rows: result.rows,
      },
      metadata: result.metadata,
    });

  } catch (error) {
    console.error('Transform error:', error);
    res.status(500).json({
      error: error.message,
      stack: process.env.NODE_ENV === 'development' ? error.stack : undefined
    });
  }
});

/**
 * HTML-to-PDF endpoint
 *
 * POST /html-to-pdf
 * Body: { html: string }
 *
 * Converts an HTML document to PDF using WeasyPrint (Python).
 * Returns: { pdf: string (base64-encoded PDF) }
 */
app.post('/html-to-pdf', (req, res) => {
  const { html } = req.body;

  if (!html) {
    return res.status(400).json({ error: 'html is required' });
  }

  const dir = mkdtempSync(join(tmpdir(), 'pdf-'));
  const htmlPath = join(dir, 'input.html');
  const pdfPath = join(dir, 'output.pdf');

  try {
    writeFileSync(htmlPath, html, 'utf-8');

    console.log('Converting HTML to PDF via WeasyPrint');
    execFileSync('python3', [join(__dirname, 'pdf_converter.py'), htmlPath, pdfPath], {
      timeout: 60_000,
    });

    const pdfBuffer = readFileSync(pdfPath);
    const pdfBase64 = pdfBuffer.toString('base64');

    console.log(`PDF generated: ${pdfBuffer.length} bytes`);
    res.json({ pdf: pdfBase64 });
  } catch (error) {
    console.error('HTML-to-PDF error:', error.message);
    res.status(500).json({
      error: `PDF conversion failed: ${error.message}`,
    });
  } finally {
    try { unlinkSync(htmlPath); } catch {}
    try { unlinkSync(pdfPath); } catch {}
    try { rmdirSync(dir); } catch {}
  }
});

/**
 * Start server
 */
app.listen(PORT, () => {
  console.log(`🎨 ChartML Renderer Service`);
  console.log(`📡 Listening on http://localhost:${PORT}`);
  console.log(`🔍 Health check: http://localhost:${PORT}/health`);
  console.log(`✅ Ready to render charts`);
});
