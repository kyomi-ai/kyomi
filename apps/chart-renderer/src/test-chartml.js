// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Test ChartML rendering with real specs
 *
 * Generates sample charts that can be compared to kyomi.ai frontend
 */

import http from 'http';
import fs from 'fs';
import path from 'path';
import { fileURLToPath } from 'url';
import { dirname } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const PORT = 3030;

async function makeRequest(method, path, body = null) {
  return new Promise((resolve, reject) => {
    const options = {
      hostname: 'localhost',
      port: PORT,
      path,
      method,
      headers: {
        'Content-Type': 'application/json'
      }
    };

    const req = http.request(options, (res) => {
      let data = '';
      res.on('data', chunk => data += chunk);
      res.on('end', () => {
        try {
          resolve({
            status: res.statusCode,
            data: JSON.parse(data)
          });
        } catch (e) {
          resolve({
            status: res.statusCode,
            data,
            error: e.message
          });
        }
      });
    });

    req.on('error', reject);

    if (body) {
      req.write(JSON.stringify(body));
    }

    req.end();
  });
}

const TEST_SPECS = {
  'bar-chart': {
    name: 'Bar Chart',
    spec: `type: chart
version: 1
title: "Monthly Sales"

data:
  provider: inline
  rows:
    - category: January
      value: 45
    - category: February
      value: 62
    - category: March
      value: 38
    - category: April
      value: 71
    - category: May
      value: 55

visualize:
  type: bar
  columns: category
  rows: value
  axes:
    left:
      label: "Value"
  style:
    height: 400
    colors:
      - "#4A90E2"
`,
  },

  'line-chart': {
    name: 'Line Chart',
    spec: `type: chart
version: 1
title: "Revenue Trends"

data:
  provider: inline
  rows:
    - month: Jan
      revenue: 42000
    - month: Feb
      revenue: 48000
    - month: Mar
      revenue: 45000
    - month: Apr
      revenue: 53000
    - month: May
      revenue: 61000

visualize:
  type: line
  columns: month
  rows: revenue
  axes:
    left:
      label: "Revenue ($)"
      format: "$,.0f"
  style:
    height: 400
    colors:
      - "#2ECC71"
`,
  },

  'metric-card': {
    name: 'Metric Card',
    spec: `type: chart
version: 1
title: "User Metrics"

data:
  provider: inline
  rows:
    - value: 1247

visualize:
  type: metric
  value: value
  label: "Total Users"
  format: ",.0f"
`,
  },

  'grouped-bar': {
    name: 'Grouped Bar Chart',
    spec: `type: chart
version: 1
title: "Revenue vs Costs"

data:
  provider: inline
  rows:
    - month: Q1
      revenue: 45000
      costs: 32000
    - month: Q2
      revenue: 52000
      costs: 35000
    - month: Q3
      revenue: 48000
      costs: 33000
    - month: Q4
      revenue: 61000
      costs: 38000

visualize:
  type: bar
  mode: grouped
  columns: month
  rows:
    - revenue
    - costs
  axes:
    left:
      label: "Amount ($)"
      format: "$,.0f"
  style:
    height: 400
    colors:
      - "#3498DB"
      - "#E74C3C"
`,
  },
};

async function runTests() {
  console.log('🧪 Testing ChartML Rendering\n');
  console.log('Generating sample charts to compare with kyomi.ai frontend...\n');

  // Test health check
  console.log('Test 1: Health check');
  try {
    const response = await makeRequest('GET', '/health');
    if (response.status === 200 && response.data.status === 'ok') {
      console.log('✅ Service is running\n');
    } else {
      console.log('❌ Health check failed:', response, '\n');
      process.exit(1);
    }
  } catch (error) {
    console.error('❌ Health check error:', error.message);
    console.error('💡 Make sure the server is running: npm start\n');
    process.exit(1);
  }

  // Render each test spec
  const outputDir = path.join(__dirname, '../test-charts');
  if (!fs.existsSync(outputDir)) {
    fs.mkdirSync(outputDir, { recursive: true });
  }

  console.log('Test 2: Rendering ChartML specs\n');

  for (const [key, test] of Object.entries(TEST_SPECS)) {
    console.log(`  Rendering: ${test.name}`);

    try {
      const response = await makeRequest('POST', '/render', {
        chartMLSpec: test.spec,
        width: 800,
        height: 600
      });

      if (response.status === 200 && response.data.image) {
        const outputPath = path.join(outputDir, `${key}.png`);
        const buffer = Buffer.from(response.data.image, 'base64');
        fs.writeFileSync(outputPath, buffer);
        console.log(`  ✅ Saved to: ${outputPath}`);
      } else {
        console.log(`  ❌ Failed:`, response.error || response.data);
      }
    } catch (error) {
      console.log(`  ❌ Error:`, error.message);
    }
  }

  console.log('\n✅ Chart generation complete!');
  console.log(`\nGenerated charts saved to: ${outputDir}`);
  console.log('\nTo compare with kyomi.ai:');
  console.log('1. Open each PNG file');
  console.log('2. Go to http://localhost:5173 and paste the ChartML spec');
  console.log('3. Compare the rendered charts visually');
  console.log('\nChartML specs used:\n');

  for (const [key, test] of Object.entries(TEST_SPECS)) {
    console.log(`\n--- ${test.name} ---`);
    console.log(test.spec);
  }
}

runTests().catch(err => {
  console.error('Fatal error:', err);
  process.exit(1);
});
