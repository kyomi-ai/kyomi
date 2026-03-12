// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * D3 Pie/Doughnut Chart Renderer
 *
 * Renders pie and doughnut charts with interactive tooltips and legends.
 */

import * as d3 from 'd3';

/**
 * Render a pie or doughnut chart
 *
 * @param {HTMLElement} container - DOM element to render into
 * @param {Object} config - Chart configuration
 * @param {Array} data - Chart data
 */
export function renderPieChart(container, config, data) {
  const { categoryField, valueField, height, colors, type, width } = config;

  // Clear container
  container.innerHTML = '';

  // Colors MUST be present from style resolution
  if (!colors || !Array.isArray(colors)) {
    throw new Error('Pie chart config missing colors array. Ensure style resolution includes palette colors.');
  }

  const pieColors = colors;

  // Calculate dimensions
  const radius = Math.min(width, height) / 2 - 40;
  const innerRadius = type === 'doughnut' ? radius * 0.6 : 0;
  const cx = width / 2;
  const cy = height / 2 + 20;

  // Use d3.pie to calculate angles
  const pie = d3.pie()
    .value(d => d[valueField])
    .sort(null); // Maintain data order

  // Use d3.arc to generate path strings
  const arc = d3.arc()
    .innerRadius(innerRadius)
    .outerRadius(radius);

  // Arc for hover effect (slightly larger)
  const arcHover = d3.arc()
    .innerRadius(innerRadius)
    .outerRadius(radius + 5);

  // Generate pie slices
  const arcs = pie(data);
  const total = d3.sum(data, d => d[valueField]);

  // Create SVG using d3 for proper event handling
  const svg = d3.create('svg')
    .attr('width', '100%')
    .attr('height', height)
    .attr('viewBox', [0, 0, width, height])
    .attr('preserveAspectRatio', 'xMidYMid meet')
    .style('font-family', 'system-ui')
    .style('max-width', '100%');

  // Create tooltip div - styled to match Observable Plot tooltips
  const tooltip = d3.select(container)
    .append('div')
    .style('position', 'absolute')
    .style('background', 'white')
    .style('color', 'black')
    .style('padding', '8px 12px')
    .style('border-radius', '4px')
    .style('font-size', '12px')
    .style('font-family', 'system-ui')
    .style('pointer-events', 'none')
    .style('opacity', 0)
    .style('z-index', 1000)
    .style('box-shadow', '0 3px 4px rgba(0,0,0,0.2)')
    .style('transition', 'opacity 0.2s')
    .style('border', '1px solid rgba(0,0,0,0.1)');

  // Create pie slices group
  const g = svg.append('g')
    .attr('transform', `translate(${cx}, ${cy})`);

  // Add slices with hover effects
  g.selectAll('path')
    .data(arcs)
    .join('path')
    .attr('d', arc)
    .attr('fill', (d, i) => pieColors[i % pieColors.length])
    .attr('stroke', 'white')
    .attr('stroke-width', 2)
    .style('opacity', 0.9)
    .style('cursor', 'pointer')
    .on('mouseenter', function(event, d) {
      const category = d.data[categoryField];
      const value = d.data[valueField];
      const percentage = ((value / total) * 100).toFixed(1);

      // Enlarge slice
      d3.select(this)
        .transition()
        .duration(200)
        .attr('d', arcHover)
        .style('opacity', 1);

      // Show tooltip
      tooltip
        .style('opacity', 1)
        .html(`<strong>${category}</strong><br/>${value.toLocaleString()} (${percentage}%)`);
    })
    .on('mousemove', function(event) {
      // Position tooltip near cursor
      const containerRect = container.getBoundingClientRect();
      tooltip
        .style('left', (event.pageX - containerRect.left + 10) + 'px')
        .style('top', (event.pageY - containerRect.top - 10) + 'px');
    })
    .on('mouseleave', function() {
      // Reset slice
      d3.select(this)
        .transition()
        .duration(200)
        .attr('d', arc)
        .style('opacity', 0.9);

      // Hide tooltip
      tooltip.style('opacity', 0);
    });

  // Create legend
  const legend = svg.append('g')
    .attr('transform', `translate(${width - 150}, 20)`);

  legend.selectAll('g')
    .data(arcs)
    .join('g')
    .attr('transform', (d, i) => `translate(0, ${i * 25})`)
    .each(function(d, i) {
      const g = d3.select(this);
      const category = d.data[categoryField];
      const percentage = ((d.data[valueField] / total) * 100).toFixed(1);
      const color = pieColors[i % pieColors.length];

      g.append('rect')
        .attr('width', 18)
        .attr('height', 18)
        .attr('rx', 3)
        .attr('fill', color);

      g.append('text')
        .attr('x', 25)
        .attr('y', 13)
        .attr('font-size', 12)
        .attr('fill', '#374151')
        .text(category);

      g.append('text')
        .attr('x', 25)
        .attr('y', 25)
        .attr('font-size', 10)
        .attr('fill', '#6b7280')
        .text(`${percentage}%`);
    });

  // Append SVG to container
  container.appendChild(svg.node());
}
