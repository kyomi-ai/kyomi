// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Metric Card Renderer
 *
 * Renders KPI/metric cards with value, label, and optional comparison
 */

import { createFormatter } from './formatters.js';

/**
 * Render a metric card
 *
 * @param {HTMLElement} container - Container element
 * @param {Object} config - Metric configuration
 * @param {*} config.value - The metric value to display
 * @param {string} config.label - The metric label
 * @param {string} config.format - Format string for the value
 * @param {Object} config.comparison - Optional comparison data
 * @param {number} config.comparison.change - Absolute change
 * @param {number} config.comparison.percentChange - Percentage change
 * @param {string} config.comparison.trend - 'up', 'down', or 'neutral'
 */
export function renderMetricCard(container, config) {
  const { value, label, format, comparison, align = 'center', showLabel = true } = config;

  // Clear container
  container.innerHTML = '';

  // Create formatter
  const formatter = format ? createFormatter(format, 'auto') : (v => String(v));

  // Format the main value
  const formattedValue = value != null ? formatter(value) : '—';

  // Build the card HTML with container query context
  const card = document.createElement('div');
  card.className = 'metric-card';
  card.style.cssText = `
    background: white;
    padding: 20px;
    height: 100%;
    display: flex;
    flex-direction: column;
    justify-content: ${(label && showLabel) ? 'space-between' : 'center'};
    text-align: ${align};
    container-type: inline-size;
  `;

  // Label (optional - only render if provided and showLabel is true)
  if (label && showLabel) {
    const labelEl = document.createElement('div');
    labelEl.className = 'metric-label';
    labelEl.textContent = label;
    labelEl.style.cssText = `
      font-size: clamp(0.75rem, 6cqw, 0.875rem);
      font-weight: 500;
      color: #6b7280;
      margin-bottom: 8px;
      font-family: system-ui, -apple-system, sans-serif;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    `;
    card.appendChild(labelEl);
  }

  // Value - responsive font size using container query width
  const valueEl = document.createElement('div');
  valueEl.className = 'metric-value';
  valueEl.textContent = formattedValue;
  valueEl.style.cssText = `
    font-size: clamp(1.5rem, 14cqw, 2rem);
    font-weight: 600;
    color: #111827;
    margin-bottom: 8px;
    font-family: system-ui, -apple-system, sans-serif;
    line-height: 1.2;
  `;

  card.appendChild(valueEl);

  // Comparison (if available)
  if (comparison) {
    const { percentChange, direction, isGood } = comparison;

    const comparisonEl = document.createElement('div');
    comparisonEl.className = 'metric-comparison';

    // Set justify-content based on alignment
    const justifyContent = align === 'left' ? 'flex-start' : align === 'right' ? 'flex-end' : 'center';

    comparisonEl.style.cssText = `
      display: flex;
      align-items: center;
      justify-content: ${justifyContent};
      gap: 4px;
      font-size: clamp(0.75rem, 6cqw, 0.875rem);
      font-family: system-ui, -apple-system, sans-serif;
    `;

    // Determine arrow and color
    let color = '#6b7280'; // neutral gray
    let arrow = '';

    if (direction === 'up') {
      arrow = '↑';
      color = isGood ? '#10b981' : '#ef4444'; // green if good, red if bad
    } else if (direction === 'down') {
      arrow = '↓';
      color = isGood ? '#10b981' : '#ef4444'; // green if good, red if bad
    }

    comparisonEl.style.color = color;
    comparisonEl.style.fontWeight = '500';

    // Format percentage
    const percentText = Math.abs(percentChange).toFixed(1) + '%';
    comparisonEl.textContent = `${arrow} ${percentText}`;

    card.appendChild(comparisonEl);
  }

  container.appendChild(card);
}
