// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Markdown ChartML Parser v1.0
 *
 * Extracts ChartML v1.0 code blocks from markdown documents.
 * All components use ```chartml blocks with type: field for discrimination.
 */

import * as yaml from 'js-yaml';
import systemDefaultsYaml from '../config/system-defaults.chartml?raw';
import { getPalette } from '../config/chartPalettes.js';

/**
 * Deep merge two objects
 * Arrays are replaced, not merged. Objects are recursively merged.
 *
 * @param {Object} target - Base object
 * @param {Object} source - Object to merge in (overrides target)
 * @returns {Object} Merged object
 */
export function deepMerge(target, source) {
  // Handle null/undefined
  if (!source) return target;
  if (!target) return source;

  // Create a new object to avoid mutating inputs
  const result = { ...target };

  for (const key in source) {
    if (source.hasOwnProperty(key)) {
      const sourceValue = source[key];
      const targetValue = target[key];

      // If both values are plain objects, recursively merge
      if (
        sourceValue &&
        typeof sourceValue === 'object' &&
        !Array.isArray(sourceValue) &&
        targetValue &&
        typeof targetValue === 'object' &&
        !Array.isArray(targetValue)
      ) {
        result[key] = deepMerge(targetValue, sourceValue);
      } else {
        // Otherwise, source overwrites target (including arrays)
        result[key] = sourceValue;
      }
    }
  }

  return result;
}

/**
 * Parse markdown document and extract sources, params, styles, config, and charts
 *
 * @param {string} markdown - Markdown content
 * @returns {Object} { sources: {}, charts: [], params: [], styles: {}, config: null }
 */
export function parseMarkdownChartML(markdown) {
  const sources = {};
  const charts = [];
  const params = []; // Array of params blocks (in order they appear)
  const styles = {}; // Named styles keyed by name
  let config = null; // Dashboard-level config (only one allowed)
  const sourceNames = []; // Track all source names including duplicates
  const styleNames = []; // Track all style names including duplicates

  // Regular expression to match ```chartml code blocks
  const codeBlockRegex = /```chartml\s*\n([\s\S]*?)```/g;

  let match;
  while ((match = codeBlockRegex.exec(markdown)) !== null) {
    const content = match[1].trim();

    try {
      const parsed = parseYAML(content);

      // Handle both single objects and arrays
      const components = Array.isArray(parsed) ? parsed : [parsed];

      components.forEach(component => {
        // Check for type field to discriminate component type
        if (!component.type) {
          return;
        }

        // Validate version field
        if (component.version !== 1) {
        }

        // Process based on type
        switch (component.type) {
          case 'source':
            // Parse source component
            if (!component.name) {
              throw new Error('Source component must have a "name" property');
            }

            const sourceName = component.name;
            sourceNames.push(sourceName); // Track for duplicate detection

            // Store source without the name and type fields
            const { name, type, version, ...sourceData } = component;
            sources[sourceName] = sourceData;
            break;

          case 'params':
            // Parse params component
            if (!component.params || !Array.isArray(component.params)) {
              throw new Error('Params component must have a "params" array');
            }

            // Validate each parameter has required fields
            component.params.forEach((param, idx) => {
              if (!param.id) {
                throw new Error(`Parameter at index ${idx} must have an "id" property`);
              }
              if (!param.type) {
                throw new Error(`Parameter "${param.id}" must have a "type" property`);
              }
            });

            params.push(component.params);
            break;

          case 'style':
            // Parse style component
            if (!component.name) {
              throw new Error('Style component must have a "name" property');
            }

            const styleName = component.name;
            styleNames.push(styleName); // Track for duplicate detection

            // Store style without the name, type, and version fields
            const { name: sName, type: sType, version: sVersion, ...styleData } = component;
            styles[styleName] = styleData;
            break;

          case 'config':
            // Parse config component
            if (config !== null) {
            }

            // Store config without type and version fields
            const { type: cType, version: cVersion, ...configData } = component;
            config = configData;
            break;

          case 'chart':
            // Parse chart component
            charts.push(component);
            break;

          default:
        }
      });
    } catch (error) {
      // Continue processing other blocks
    }
  }

  return {
    sources,
    charts,
    params,
    styles,
    config,
    _sourceNames: sourceNames,
    _styleNames: styleNames
  };
}

/**
 * Parse YAML content synchronously
 */
function parseYAML(content) {
  try {
    return yaml.load(content);
  } catch (error) {
    // Fallback: try to parse as JSON (YAML is superset of JSON)
    try {
      return JSON.parse(content);
    } catch {
      throw new Error(`Failed to parse YAML: ${error.message}`);
    }
  }
}

/**
 * Extract all source components from markdown
 *
 * @param {string} markdown - Markdown content
 * @returns {Object} Source definitions keyed by name
 */
export function extractSources(markdown) {
  const { sources } = parseMarkdownChartML(markdown);
  return sources;
}

/**
 * Extract all chart components from markdown
 *
 * @param {string} markdown - Markdown content
 * @returns {Array} Array of chart specifications
 */
export function extractCharts(markdown) {
  const { charts } = parseMarkdownChartML(markdown);
  return charts;
}

/**
 * Extract all params components from markdown
 *
 * @param {string} markdown - Markdown content
 * @returns {Array} Array of parameter definition arrays
 */
export function extractParams(markdown) {
  const { params } = parseMarkdownChartML(markdown);
  return params;
}

/**
 * Extract all style components from markdown
 *
 * @param {string} markdown - Markdown content
 * @returns {Object} Style definitions keyed by name
 */
export function extractStyles(markdown) {
  const { styles } = parseMarkdownChartML(markdown);
  return styles;
}

/**
 * Extract config component from markdown
 *
 * @param {string} markdown - Markdown content
 * @returns {Object|null} Config object or null if not defined
 */
export function extractConfig(markdown) {
  const { config } = parseMarkdownChartML(markdown);
  return config;
}

/**
 * Resolve a chart's style based on scope hierarchy
 *
 * Resolution order (system → workspace → user → dashboard → chart):
 * 1. Start with system config.style (from system-defaults.chartml)
 * 2. Override with workspace config.style
 * 3. Override with user config.style
 * 4. Override with dashboard config.style
 * 5. Override with explicit chart style: field reference
 * 6. Override with inline chart visualize.style
 *
 * @param {Object} chart - Chart component
 * @param {Object} dashboardStyles - Named styles from markdown (dashboard scope)
 * @param {Object|null} dashboardConfig - Config component from markdown (dashboard scope)
 * @param {Object|null} userConfig - User-level config from settings
 * @param {Object|null} workspaceConfig - Workspace-level config from settings
 * @param {Object} systemStyles - Named styles from system-defaults.chartml
 * @param {Object|null} systemConfig - Config from system-defaults.chartml
 * @returns {Object|null} Resolved style object or null
 */
export function resolveChartStyle(
  chart,
  dashboardStyles = {},
  dashboardConfig = null,
  userConfig = null,
  workspaceConfig = null,
  systemStyles = SYSTEM_STYLES,
  systemConfig = SYSTEM_CONFIG
) {
  let baseStyle = null;

  // Combine all named styles (later scopes can override earlier ones)
  const allStyles = { ...systemStyles, ...dashboardStyles };

  // Step 1: Start with system config.style as the absolute base
  if (systemConfig && systemConfig.style) {
    if (typeof systemConfig.style === 'string') {
      // System config references a named style (should exist in systemStyles)
      const styleName = systemConfig.style;
      baseStyle = allStyles[styleName] || null;

      if (!baseStyle) {
      }
    } else {
      // System config has inline style definition
      baseStyle = systemConfig.style;
    }
  }

  // Step 2: Override with workspace config.style
  if (workspaceConfig && workspaceConfig.style) {
    if (typeof workspaceConfig.style === 'string') {
      // Named style reference → completely replace base
      const configStyle = allStyles[workspaceConfig.style];
      if (!configStyle) {
      } else {
        baseStyle = configStyle;
      }
    } else {
      // Inline style object → merge with base
      baseStyle = baseStyle ? deepMerge(baseStyle, workspaceConfig.style) : workspaceConfig.style;
    }
  }

  // Step 3: Override with user config.style
  if (userConfig && userConfig.style) {
    if (typeof userConfig.style === 'string') {
      // Named style reference → completely replace base
      const configStyle = allStyles[userConfig.style];
      if (configStyle) {
      }
      if (!configStyle) {
      } else {
        baseStyle = configStyle;
      }
    } else {
      // Inline style object → merge with base
      baseStyle = baseStyle ? deepMerge(baseStyle, userConfig.style) : userConfig.style;
    }
  }

  // Step 4: Override with dashboard config.style
  if (dashboardConfig && dashboardConfig.style) {
    if (typeof dashboardConfig.style === 'string') {
      // Named style reference → completely replace base
      const configStyle = allStyles[dashboardConfig.style];
      if (!configStyle) {
      } else {
        baseStyle = configStyle;
      }
    } else {
      // Inline style object → merge with base
      baseStyle = baseStyle ? deepMerge(baseStyle, dashboardConfig.style) : dashboardConfig.style;
    }
  }

  // Step 5: Override with explicit chart style: field reference
  if (chart.style && typeof chart.style === 'string') {
    const chartStyle = allStyles[chart.style];

    if (!chartStyle) {
    } else {
      baseStyle = baseStyle ? deepMerge(baseStyle, chartStyle) : chartStyle;
    }
  }

  // Step 6: Override with inline chart visualize.style
  if (chart.visualize && chart.visualize.style) {
    baseStyle = baseStyle
      ? deepMerge(baseStyle, chart.visualize.style)
      : chart.visualize.style;
  }

  // Step 7: Resolve palette reference to actual colors
  // If the resolved style has a 'palette' field, look up colors from chartPalettes.js
  if (baseStyle && baseStyle.palette) {
    try {
      const paletteColors = getPalette(baseStyle.palette);

      // Replace palette field with actual colors array
      const { palette, ...styleWithoutPalette } = baseStyle;
      baseStyle = {
        ...styleWithoutPalette,
        colors: paletteColors
      };
    } catch (error) {
      // Don't set colors - let it fail downstream with a clear error
    }
  }


  return baseStyle;
}

/**
 * Get markdown context for a specific chart
 * Returns the markdown section surrounding a chart block
 *
 * @param {string} markdown - Full markdown content
 * @param {number} chartIndex - Index of chart (0-based)
 * @returns {Object} { title, description, chart }
 */
export function getChartContext(markdown, chartIndex) {
  const charts = extractCharts(markdown);

  if (chartIndex >= charts.length) {
    return null;
  }

  // Find the chart block in markdown
  const codeBlockRegex = /```chartml\s*\n([\s\S]*?)```/g;
  const matches = [...markdown.matchAll(codeBlockRegex)];

  if (chartIndex >= matches.length) {
    return null;
  }

  const match = matches[chartIndex];
  const blockStart = match.index;

  // Extract markdown before the chart block (up to previous heading or start)
  const beforeBlock = markdown.substring(0, blockStart);

  // Find the last heading before this chart
  const headingRegex = /^#{1,6}\s+(.+)$/gm;
  const headings = [...beforeBlock.matchAll(headingRegex)];
  const lastHeading = headings.length > 0 ? headings[headings.length - 1] : null;

  // Extract description (text between heading and chart block)
  let description = '';
  if (lastHeading) {
    const headingEnd = lastHeading.index + lastHeading[0].length;
    description = beforeBlock.substring(headingEnd, blockStart).trim();
  }

  return {
    title: lastHeading ? lastHeading[1].trim() : charts[chartIndex].title || 'Untitled',
    description: description,
    chart: charts[chartIndex]
  };
}

/**
 * Validate markdown ChartML document
 *
 * @param {string} markdown - Markdown content
 * @returns {Object} { valid: boolean, errors: [], warnings: [] }
 */
export function validateMarkdownChartML(markdown) {
  const errors = [];
  const warnings = [];

  try {
    const { sources, charts, styles, config, _sourceNames, _styleNames } = parseMarkdownChartML(markdown);

    // Check for charts
    if (charts.length === 0) {
      warnings.push('No chart components found in markdown');
    }

    // Validate each chart
    charts.forEach((chart, index) => {
      // Validate chart has type and version
      if (chart.type !== 'chart') {
        errors.push(`Chart ${index + 1} has invalid type "${chart.type}"`);
      }

      if (chart.version !== 1) {
        warnings.push(`Chart ${index + 1} has version ${chart.version}, expected version 1`);
      }

      // Check if chart references a source (data can be string reference)
      if (typeof chart.data === 'string') {
        const sourceName = chart.data;

        // Check if source exists locally
        if (!sources[sourceName]) {
          warnings.push(`Chart ${index + 1} references source "${sourceName}" which is not defined in this document`);
        }
      }

      // Check if chart references a style (style can be string reference)
      if (typeof chart.style === 'string') {
        const styleName = chart.style;

        // Check if style exists locally
        if (!styles[styleName]) {
          warnings.push(`Chart ${index + 1} references style "${styleName}" which is not defined in this document`);
        }
      }

      // Basic chart validation
      if (!chart.visualize) {
        errors.push(`Chart ${index + 1} missing required "visualize" section`);
      }

      if (!chart.data) {
        errors.push(`Chart ${index + 1} missing required "data" section`);
      }
    });

    // Check for duplicate source names using the array we tracked during parsing
    const seenSourceNames = new Set();
    const duplicateSources = [];

    _sourceNames.forEach(name => {
      if (seenSourceNames.has(name)) {
        if (!duplicateSources.includes(name)) {
          duplicateSources.push(name);
        }
      }
      seenSourceNames.add(name);
    });

    if (duplicateSources.length > 0) {
      errors.push(`Duplicate source names found: ${duplicateSources.join(', ')}`);
    }

    // Check for duplicate style names
    const seenStyleNames = new Set();
    const duplicateStyles = [];

    _styleNames.forEach(name => {
      if (seenStyleNames.has(name)) {
        if (!duplicateStyles.includes(name)) {
          duplicateStyles.push(name);
        }
      }
      seenStyleNames.add(name);
    });

    if (duplicateStyles.length > 0) {
      errors.push(`Duplicate style names found: ${duplicateStyles.join(', ')}`);
    }

  } catch (error) {
    errors.push(`Failed to parse markdown: ${error.message}`);
  }

  return {
    valid: errors.length === 0,
    errors,
    warnings
  };
}

/**
 * Parse .chartml file (raw YAML, no markdown)
 * Supports two formats:
 * 1. Array of components: [{ type: style, ... }, { type: config, ... }]
 * 2. Multi-document YAML with --- separators
 */
function parseChartmlFile(yamlContent) {
  const styles = {};
  let config = null;

  try {
    // Try parsing as multi-document YAML first (with --- separators)
    const documents = yaml.loadAll(yamlContent);

    // Flatten: if any document is an array, spread it out
    const allComponents = [];
    documents.forEach(doc => {
      if (Array.isArray(doc)) {
        allComponents.push(...doc);
      } else if (doc && typeof doc === 'object') {
        allComponents.push(doc);
      }
    });

    // Process each component
    allComponents.forEach((component, index) => {
      if (!component || typeof component !== 'object') {
        return;
      }

      // Process based on type
      switch (component.type) {
        case 'style':
          if (!component.name) {
            return;
          }
          const { name, type, version, ...styleData } = component;
          styles[name] = styleData;
          break;

        case 'config':
          if (config !== null) {
          }
          const { type: cType, version: cVersion, ...configData } = component;
          config = configData;
          break;
      }
    });
  } catch (error) {
  }

  return { styles, config };
}

/**
 * System-level defaults
 * Parsed from system-defaults.chartml at module load time
 */
let SYSTEM_STYLES = {};
let SYSTEM_CONFIG = null;

try {
  const SYSTEM_DEFAULTS = parseChartmlFile(systemDefaultsYaml);
  SYSTEM_STYLES = SYSTEM_DEFAULTS.styles;
  SYSTEM_CONFIG = SYSTEM_DEFAULTS.config;
} catch (error) {
  // Fallback to empty defaults
  SYSTEM_STYLES = {};
  SYSTEM_CONFIG = null;
}

export { SYSTEM_STYLES, SYSTEM_CONFIG, parseChartmlFile };
