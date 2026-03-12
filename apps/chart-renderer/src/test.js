// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Test script for chart renderer POC
 *
 * Tests:
 * 1. Basic D3 + JSDOM rendering
 * 2. SVG → PNG conversion
 * 3. Base64 encoding
 */

const http = require('http');
const fs = require('fs');
const path = require('path');

const PORT = 3030;
const BASE_URL = `http://localhost:${PORT}`;

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
            data
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

async function runTests() {
  console.log('🧪 Testing ChartML Renderer Service\n');

  // Test 1: Health check
  console.log('Test 1: Health check');
  try {
    const response = await makeRequest('GET', '/health');
    if (response.status === 200 && response.data.status === 'ok') {
      console.log('✅ Health check passed\n');
    } else {
      console.log('❌ Health check failed:', response, '\n');
      process.exit(1);
    }
  } catch (error) {
    console.error('❌ Health check error:', error.message);
    console.error('💡 Make sure the server is running: npm start\n');
    process.exit(1);
  }

  // Test 2: Render basic test chart
  console.log('Test 2: Render basic test chart');
  try {
    const response = await makeRequest('POST', '/render', {
      chartMLSpec: 'visualize: bar',  // Dummy spec for now
      width: 400,
      height: 300
    });

    if (response.status === 200 && response.data.image) {
      console.log(`✅ Chart rendered successfully`);
      console.log(`   Format: ${response.data.format}`);
      console.log(`   Size: ${response.data.width}x${response.data.height}`);
      console.log(`   Base64 length: ${response.data.image.length} chars`);

      // Save to file for manual inspection
      const outputPath = path.join(__dirname, '../test-output.png');
      const buffer = Buffer.from(response.data.image, 'base64');
      fs.writeFileSync(outputPath, buffer);
      console.log(`   Saved to: ${outputPath}`);
      console.log('   Open the file to verify the chart rendered correctly\n');
    } else {
      console.log('❌ Chart rendering failed:', response, '\n');
      process.exit(1);
    }
  } catch (error) {
    console.error('❌ Render error:', error.message, '\n');
    process.exit(1);
  }

  console.log('✅ All tests passed!\n');
  console.log('Next steps:');
  console.log('1. Open test-output.png to verify SVG rendering works');
  console.log('2. Integrate real ChartML library');
  console.log('3. Test with actual ChartML specs');
}

runTests().catch(err => {
  console.error('Fatal error:', err);
  process.exit(1);
});
