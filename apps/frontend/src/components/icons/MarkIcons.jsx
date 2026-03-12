// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Mark Type Icons (Series Icons)
 * Reusable SVG icons for different mark types
 * These match the chart type icons in style
 */

// PRESERVED: Same as VerticalBarIcon - Heroicons style
export const BarMarkIcon = ({ className = "w-4 h-4" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    <path d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" opacity="0.7"/>
  </svg>
);

// PRESERVED: Kept exactly as-is from working version
export const LineMarkIcon = ({ className = "w-4 h-4" }) => (
  <svg className={className} fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth={2}>
    <path strokeLinecap="round" strokeLinejoin="round" d="M7 12l3-3 3 3 4-4" />
  </svg>
);

// PRESERVED: Kept exactly as-is from working version
export const AreaMarkIcon = ({ className = "w-4 h-4" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    <path d="M3 20L7 12L11 16L16 8L21 13V20H3Z" opacity="0.5"/>
    <path d="M3 20L7 12L11 16L16 8L21 13" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
  </svg>
);

// PRESERVED: Kept exactly as-is from working version
export const DotMarkIcon = ({ className = "w-4 h-4" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    <circle cx="6" cy="12" r="2" />
    <circle cx="12" cy="8" r="2" />
    <circle cx="18" cy="14" r="2" />
  </svg>
);
