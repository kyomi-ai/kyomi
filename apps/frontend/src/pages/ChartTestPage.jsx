// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useState } from 'react';
import { MarkdownRenderer } from '../components/MarkdownRenderer';

/**
 * ChartTestPage - Test page for debugging chart width issues
 * No authentication required - direct access for testing
 */
const ChartTestPage = () => {
  const [mockMarkdown] = useState(`# X-Axis Label Strategy Tests

Testing intelligent label strategies with various category counts.

## Test 1: Few Categories (5) - Should use HORIZONTAL labels

\`\`\`chartml
visualize:
  type: bar
  columns: region
  rows: sales
data:
  - region: North
    sales: 12000
  - region: South
    sales: 15000
  - region: East
    sales: 13500
  - region: West
    sales: 14200
  - region: Central
    sales: 11800
\`\`\`

## Test 2: Medium Categories (12) - Should use ROTATED labels (-45°)

\`\`\`chartml
visualize:
  type: bar
  columns: month
  rows: revenue
data:
  - month: January
    revenue: 12000
  - month: February
    revenue: 13500
  - month: March
    revenue: 15000
  - month: April
    revenue: 14200
  - month: May
    revenue: 16500
  - month: June
    revenue: 18000
  - month: July
    revenue: 17500
  - month: August
    revenue: 19000
  - month: September
    revenue: 18500
  - month: October
    revenue: 20000
  - month: November
    revenue: 21500
  - month: December
    revenue: 23000
\`\`\`

## Test 3: Many Long Categories (11) - User's Original Screenshot Data

\`\`\`chartml
visualize:
  type: bar
  columns: term
  rows: score
data:
  - term: chicago marathon results
    score: 7000
  - term: cardinals vs colts
    score: 11500
  - term: cardinals vs colts tickets
    score: 10000
  - term: farmers almanac weather
    score: 10500
  - term: farmers almanac weather 2024
    score: 9000
  - term: fort worth plane crash
    score: 9000
  - term: fort worth plane crash news
    score: 7500
  - term: fred warner
    score: 10000
  - term: fred warner stats
    score: 9800
  - term: calvin ridley
    score: 8600
  - term: calvin ridley fantasy
    score: 7300
\`\`\`

## Test 4: Very Many Categories (30) - Should use SAMPLED labels

\`\`\`chartml
visualize:
  type: bar
  columns: item
  rows: value
data:
  - {item: Item 1, value: 5234}
  - {item: Item 2, value: 6123}
  - {item: Item 3, value: 7456}
  - {item: Item 4, value: 5789}
  - {item: Item 5, value: 8234}
  - {item: Item 6, value: 6789}
  - {item: Item 7, value: 9123}
  - {item: Item 8, value: 7234}
  - {item: Item 9, value: 6456}
  - {item: Item 10, value: 8789}
  - {item: Item 11, value: 5567}
  - {item: Item 12, value: 9345}
  - {item: Item 13, value: 6234}
  - {item: Item 14, value: 7890}
  - {item: Item 15, value: 5123}
  - {item: Item 16, value: 8456}
  - {item: Item 17, value: 6789}
  - {item: Item 18, value: 7123}
  - {item: Item 19, value: 9567}
  - {item: Item 20, value: 6345}
  - {item: Item 21, value: 8234}
  - {item: Item 22, value: 5678}
  - {item: Item 23, value: 9123}
  - {item: Item 24, value: 7456}
  - {item: Item 25, value: 6234}
  - {item: Item 26, value: 8789}
  - {item: Item 27, value: 5345}
  - {item: Item 28, value: 9678}
  - {item: Item 29, value: 7123}
  - {item: Item 30, value: 6456}
\`\`\`
`);

  return (
    <div className="h-screen flex flex-col bg-gray-100">
      {/* Header */}
      <div className="bg-white border-b border-gray-200 px-6 py-4">
        <h1 className="text-2xl font-bold text-gray-900">Chart Width Test Page</h1>
        <p className="text-sm text-gray-600">Testing ChartML v2 in constrained containers</p>
      </div>

      {/* Main content - simulating DashboardEditor split view */}
      <div className="flex-1 flex overflow-hidden">
        {/* Left panel - Editor simulation (50%) */}
        <div className="w-1/2 m-2 p-6 bg-white border border-gray-300 rounded-md overflow-y-auto">
          <h2 className="text-lg font-semibold mb-4">Editor (50% width)</h2>
          <pre className="text-xs bg-gray-50 p-4 rounded overflow-auto">
            {mockMarkdown}
          </pre>
        </div>

        {/* Right panel - Preview (50%) - THIS IS WHERE THE ISSUE HAPPENS */}
        <div className="flex-1 m-2 p-6 bg-gray-50 overflow-y-auto" style={{ minWidth: 0 }}>
          <h2 className="text-lg font-semibold mb-4 text-gray-700">Preview (50% width - Charts should fit here)</h2>
          <MarkdownRenderer
            messageId="chart-test"
            sessionId="test"
            isStreaming={false}
            showAddToDashboard={false}
            isChatBubble={false}
          >
            {mockMarkdown}
          </MarkdownRenderer>
        </div>
      </div>
    </div>
  );
};

export default ChartTestPage;
