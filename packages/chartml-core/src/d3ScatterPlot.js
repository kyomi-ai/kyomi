// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Pure D3 Scatter Plot Implementation
 */

import * as d3 from 'd3';

/**
 * Render a scatter plot using pure d3
 *
 * @param {HTMLElement} container - DOM element to render into
 * @param {Array} data - Array of data objects
 * @param {Object} config - Chart configuration (colors MUST be provided)
 * @returns {void}
 */
export function renderD3ScatterPlot(container, data, config) {
  const {
    xField = 'x',
    yField = 'y',
    sizeField = null, // Optional: field for bubble size
    colorField = null, // Optional: field for color
    width = 600,
    height = 400,
    marginTop = 20,
    marginRight = 30,
    marginBottom = colorField ? 60 : 30,
    marginLeft = 70,
    xAxisLabel = '',
    yAxisLabel = '',
    colors, // REQUIRED - no default
    defaultRadius = 5,
    radiusRange = [3, 20] // Range for bubble sizes if sizeField is used
  } = config;

  // Validate that colors are provided
  if (!colors || !Array.isArray(colors)) {
    throw new Error('Scatter plot config missing colors array. Ensure style resolution includes palette colors.');
  }

  // Clear previous content
  container.innerHTML = '';

  // Calculate chart dimensions
  const chartWidth = width - marginLeft - marginRight;
  const chartHeight = height - marginTop - marginBottom;

  // Create SVG
  const svg = d3.select(container)
    .append('svg')
    .attr('width', '100%')
    .attr('height', height)
    .attr('viewBox', [0, 0, width, height])
    .style('max-width', '100%')
    .style('display', 'block')
    .style('overflow', 'hidden');

  // Create chart group with margins
  const g = svg.append('g')
    .attr('transform', `translate(${marginLeft},${marginTop})`);

  // Create scales
  const x = d3.scaleLinear()
    .domain([0, d3.max(data, d => d[xField])])
    .nice()
    .range([0, chartWidth]);

  const y = d3.scaleLinear()
    .domain([0, d3.max(data, d => d[yField])])
    .nice()
    .range([chartHeight, 0]);

  // Size scale if sizeField is provided
  const sizeScale = sizeField
    ? d3.scaleSqrt()
        .domain([0, d3.max(data, d => d[sizeField])])
        .range(radiusRange)
    : () => defaultRadius;

  // Color scale if colorField is provided
  const colorScale = colorField
    ? d3.scaleOrdinal()
        .domain([...new Set(data.map(d => d[colorField]))])
        .range(colors)
    : () => colors[0];

  // Add X axis
  const xAxis = g.append('g')
    .attr('transform', `translate(0,${chartHeight})`)
    .call(d3.axisBottom(x).ticks(5));

  xAxis.selectAll('text')
    .style('font-size', '12px')
    .style('font-family', 'system-ui');

  // Add X axis label
  if (xAxisLabel) {
    g.append('text')
      .attr('x', chartWidth / 2)
      .attr('y', chartHeight + marginBottom - 5)
      .attr('text-anchor', 'middle')
      .style('font-size', '14px')
      .style('font-family', 'system-ui')
      .style('fill', '#374151')
      .text(xAxisLabel);
  }

  // Add Y axis
  const yAxis = g.append('g')
    .call(d3.axisLeft(y).ticks(5));

  yAxis.selectAll('text')
    .style('font-size', '12px')
    .style('font-family', 'system-ui');

  // Add Y axis label
  if (yAxisLabel) {
    g.append('text')
      .attr('transform', 'rotate(-90)')
      .attr('x', -chartHeight / 2)
      .attr('y', -marginLeft + 15)
      .attr('text-anchor', 'middle')
      .style('font-size', '14px')
      .style('font-family', 'system-ui')
      .style('fill', '#374151')
      .text(yAxisLabel);
  }

  // Add grid lines
  g.append('g')
    .attr('class', 'grid-x')
    .attr('opacity', 0.1)
    .call(d3.axisBottom(x)
      .tickSize(chartHeight)
      .tickFormat('')
    );

  g.append('g')
    .attr('class', 'grid-y')
    .attr('opacity', 0.1)
    .call(d3.axisLeft(y)
      .tickSize(-chartWidth)
      .tickFormat('')
    );

  // Create tooltip - uses .chart-tooltip class from index.css
  const tooltip = d3.select(container)
    .append('div')
    .attr('class', 'chart-tooltip')
    .style('transition', 'opacity 0.2s');

  // Add dots
  g.selectAll('.dot')
    .data(data)
    .join('circle')
    .attr('class', 'dot')
    .attr('cx', d => x(d[xField]))
    .attr('cy', d => y(d[yField]))
    .attr('r', d => sizeField ? sizeScale(d[sizeField]) : defaultRadius)
    .attr('fill', d => colorField ? colorScale(d[colorField]) : colors[0])
    .attr('stroke', 'white')
    .attr('stroke-width', 1.5)
    .attr('opacity', 0.8)
    .style('cursor', 'pointer')
    .on('mouseenter', function(event, d) {
      // Highlight dot
      d3.select(this)
        .transition()
        .duration(200)
        .attr('r', (sizeField ? sizeScale(d[sizeField]) : defaultRadius) * 1.3)
        .attr('opacity', 1)
        .attr('stroke-width', 2);

      // Build tooltip content
      let tooltipHtml = `<strong>${xField}: ${d[xField].toLocaleString()}</strong>`;
      tooltipHtml += `<br/>${yField}: ${d[yField].toLocaleString()}`;
      if (sizeField) {
        tooltipHtml += `<br/>${sizeField}: ${d[sizeField].toLocaleString()}`;
      }
      if (colorField) {
        tooltipHtml += `<br/>${colorField}: ${d[colorField]}`;
      }

      // Show tooltip
      tooltip
        .style('opacity', 1)
        .html(tooltipHtml);
    })
    .on('mousemove', function(event) {
      const containerRect = container.getBoundingClientRect();
      tooltip
        .style('left', (event.pageX - containerRect.left + 10) + 'px')
        .style('top', (event.pageY - containerRect.top - 10) + 'px');
    })
    .on('mouseleave', function(event, d) {
      // Remove highlight
      d3.select(this)
        .transition()
        .duration(200)
        .attr('r', sizeField ? sizeScale(d[sizeField]) : defaultRadius)
        .attr('opacity', 0.8)
        .attr('stroke-width', 1.5);

      // Hide tooltip
      tooltip.style('opacity', 0);
    });

  // Add entrance animation
  g.selectAll('.dot')
    .attr('r', 0)
    .transition()
    .delay((d, i) => i * 20)
    .duration(600)
    .attr('r', d => sizeField ? sizeScale(d[sizeField]) : defaultRadius);

  // Add legend if colorField is provided - positioned below the chart
  if (colorField) {
    const categories = [...new Set(data.map(d => d[colorField]))];
    const legend = svg.append('g')
      .attr('transform', `translate(${marginLeft}, ${height - marginBottom + 35})`);

    // Calculate legend width to center it - responsive to chart width
    const idealLegendItemWidth = 100;
    const minLegendItemWidth = 60;
    const maxTotalWidth = chartWidth - 20; // Leave 10px margin on each side
    let legendItemWidth = idealLegendItemWidth;

    // If legend would be too wide, reduce item width
    if (categories.length * idealLegendItemWidth > maxTotalWidth) {
      legendItemWidth = Math.max(minLegendItemWidth, maxTotalWidth / categories.length);
    }

    const totalLegendWidth = categories.length * legendItemWidth;
    const legendStartX = Math.max(0, (chartWidth - totalLegendWidth) / 2);

    categories.forEach((category, index) => {
      const legendRow = legend.append('g')
        .attr('transform', `translate(${legendStartX + index * legendItemWidth}, 0)`);

      legendRow.append('circle')
        .attr('cx', 7)
        .attr('cy', 7)
        .attr('r', 5)
        .attr('fill', colorScale(category))
        .attr('stroke', 'white')
        .attr('stroke-width', 1.5);

      // Truncate label text if needed to fit in available width
      const labelText = String(category).length > 15 ? String(category).substring(0, 12) + '...' : category;

      legendRow.append('text')
        .attr('x', 18)
        .attr('y', 11)
        .style('font-size', '11px')
        .style('font-family', 'system-ui')
        .style('fill', '#374151')
        .text(labelText);
    });
  }
}
