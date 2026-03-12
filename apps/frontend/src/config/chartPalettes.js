// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Chart color palettes for data visualization
 *
 * Define palettes once here and reference them by name throughout the app.
 * Colors are evenly distributed across the color wheel for maximum contrast.
 */

export const CHART_PALETTES = {
  // Balanced (Default) - Classic BI with varied saturation/luminosity
  balanced: [
    '#1A75C9', // Azure - Primary blue anchor (reduced saturation)
    '#B8405A', // Crimson - Alert/negative
    '#3D8A5A', // Forest - Success/positive (true green)
    '#D9952D', // Amber - Warning (warm accent)
    '#2D7A8A', // Teal - Dark cyan
    '#C9734D', // Terracotta - Warm secondary
    '#4D5A8A', // Indigo - Deep blue-purple
    '#99C94D', // Lime - Yellow-green
    '#8A5A7A', // Mauve - Purple accent
    '#D9B370', // Sand - Light warm
    '#70B8D9', // Sky - Light blue
    '#6B8A4D', // Olive - Forest tone
  ],

  // Vibrant - Higher saturation for modern dashboards
  vibrant: [
    '#1E88C7', // Ocean - Complementary to orange
    '#D92849', // Ruby - Alert/negative
    '#28C75A', // Emerald - Success
    '#E8B733', // Sunflower - Warning
    '#28C7A8', // Turquoise - Cool accent
    '#E87333', // Tangerine - Warm secondary
    '#3355D9', // Sapphire - Deep blue
    '#A8D928', // Chartreuse - Yellow-green
    '#C728A8', // Magenta - Pink-purple
    '#D97328', // Copper - Reference match
    '#28A8D9', // Cyan - Bright cyan
    '#73A828', // Moss - Forest tone
  ],

  // Accessible - Maximum luminosity range for colorblind users
  accessible: [
    '#2D5F7A', // Navy - Dark complementary
    '#A83D52', // Burgundy - Dark red
    '#3D7A52', // Forest - Dark green
    '#C9A642', // Mustard - Medium yellow
    '#3D8A8A', // Teal - Dark cyan
    '#E89970', // Peach - Light warm
    '#5C6D99', // Slate - Medium blue
    '#B8D96B', // Lime - Light green
    '#996B8A', // Mauve - Medium purple
    '#B87752', // Sienna - Earth tone
    '#85B8D9', // Sky - Light blue
    '#85996B', // Sage - Muted green
  ],

  // Accent colors for each palette
  accents: {
    balanced: '#0A7AFF',    // Electric Blue
    vibrant: '#0080FF',     // Bright Blue
    accessible: '#2563EB',  // Royal Blue
  }
};

/**
 * Get a palette by name
 * @param {string} name - Palette name (e.g., 'balanced', 'vibrant', 'accessible')
 * @returns {string[]} Array of color hex codes
 * @throws {Error} If palette name is not found
 */
export function getPalette(name) {
  if (!CHART_PALETTES[name]) {
    throw new Error(`Palette "${name}" not found. Available palettes: ${Object.keys(CHART_PALETTES).filter(k => k !== 'accents').join(', ')}`);
  }
  return CHART_PALETTES[name];
}

