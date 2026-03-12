// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Chart Type Icons
 * Reusable SVG icons for different chart types
 * Most icons preserved exactly as-is from working version
 * Only table, vertical bar, and horizontal bar modified for consistency
 */

// PRESERVED: Heroicons table - keep as-is
export const TableIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="none" viewBox="0 0 24 24" strokeWidth="1.5" stroke="currentColor">
    <path strokeLinecap="round" strokeLinejoin="round" d="M3.375 19.5h17.25m-17.25 0a1.125 1.125 0 0 1-1.125-1.125M3.375 19.5h7.5c.621 0 1.125-.504 1.125-1.125m-9.75 0V5.625m0 12.75v-1.5c0-.621.504-1.125 1.125-1.125m18.375 2.625V5.625m0 12.75c0 .621-.504 1.125-1.125 1.125m1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125m0 3.75h-7.5A1.125 1.125 0 0 1 12 18.375m9.75-12.75c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125m19.5 0v1.5c0 .621-.504 1.125-1.125 1.125M2.25 5.625v1.5c0 .621.504 1.125 1.125 1.125m0 0h17.25m-17.25 0h7.5c.621 0 1.125.504 1.125 1.125M3.375 8.25c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125m17.25-3.75h-7.5c-.621 0-1.125.504-1.125 1.125m8.625-1.125c.621 0 1.125.504 1.125 1.125v1.5c0 .621-.504 1.125-1.125 1.125m-17.25 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125M12 10.875v-1.5m0 1.5c0 .621-.504 1.125-1.125 1.125M12 10.875c0 .621.504 1.125 1.125 1.125m-2.25 0c.621 0 1.125.504 1.125 1.125M13.125 12h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125M20.625 12c.621 0 1.125.504 1.125 1.125v1.5c0 .621-.504 1.125-1.125 1.125m-17.25 0h7.5M12 14.625v-1.5m0 1.5c0 .621-.504 1.125-1.125 1.125M12 14.625c0 .621.504 1.125 1.125 1.125m-2.25 0c.621 0 1.125.504 1.125 1.125m0 1.5v-1.5m0 0c0-.621.504-1.125 1.125-1.125m0 0h7.5" />
  </svg>
);

// PRESERVED: Heroicons vertical bar - keep as-is
export const VerticalBarIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    <path d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z" opacity="0.7"/>
  </svg>
);

// PRESERVED: Heroicons horizontal bar - keep as-is (longest at top)
export const HorizontalBarIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    <path d="M2.25 5.25C2.25 4.629 2.754 4.125 3.375 4.125h15.75c.621 0 1.125.504 1.125 1.125v2.25c0 .621-.504 1.125-1.125 1.125H3.375A1.125 1.125 0 0 1 2.25 7.5V5.25ZM2.25 12c0-.621.504-1.125 1.125-1.125h11.25c.621 0 1.125.504 1.125 1.125v2.25c0 .621-.504 1.125-1.125 1.125H3.375A1.125 1.125 0 0 1 2.25 14.25V12ZM2.25 18.75c0-.621.504-1.125 1.125-1.125h6.75c.621 0 1.125.504 1.125 1.125v2.25c0 .621-.504 1.125-1.125 1.125h-6.75A1.125 1.125 0 0 1 2.25 21v-2.25Z" opacity="0.7"/>
  </svg>
);

// ADJUSTED: Three straight line segments with moderate volatility, ends at highest point
export const LineIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    <path d="M 3.75 15 L 9 7.5 L 14.5 11 L 21 4.125" stroke="currentColor" strokeWidth="2.5" fill="none" strokeLinecap="round" strokeLinejoin="round"/>
  </svg>
);

// ADJUSTED: Area follows three-segment line with moderate volatility, extends to y=23 and left to cover stroke
export const AreaIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    {/* Top line - drawn first so area can cover it */}
    <path d="M 3.75 15 L 9 7.5 L 14.5 11 L 21 4.125" stroke="currentColor" strokeWidth="2.5" fill="none" strokeLinecap="round" strokeLinejoin="round"/>
    {/* Filled area - extends to x=22, y=23, and left to x=2.5 to cover stroke */}
    <path d="M 2.5 23 L 2.5 15 L 3.75 15 L 9 7.5 L 14.5 11 L 21 4.125 L 22 4.125 L 22 23 Z" opacity="0.5"/>
  </svg>
);

// ADJUSTED: Increased radius to 10 to match doughnut
export const PieIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    {/* Pie slices with varying opacity, radius 10 */}
    <path d="M 12 12 L 12 2 A 10 10 0 0 1 20.07 6.93 Z" opacity="0.7"/>
    <path d="M 12 12 L 20.07 6.93 A 10 10 0 0 1 20.07 17.07 Z" opacity="0.5"/>
    <path d="M 12 12 L 20.07 17.07 A 10 10 0 0 1 12 22 Z" opacity="0.6"/>
    <path d="M 12 12 L 12 22 A 10 10 0 0 1 3.93 17.07 Z" opacity="0.4"/>
    <path d="M 12 12 L 3.93 17.07 A 10 10 0 0 1 3.93 6.93 Z" opacity="0.8"/>
    <path d="M 12 12 L 3.93 6.93 A 10 10 0 0 1 12 2 Z" opacity="0.3"/>
  </svg>
);

// ADJUSTED: Increased outer radius to 10, inner radius 4
export const DoughnutIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    {/* Doughnut slices with center hole, outer radius 10, inner radius 4 */}
    <path d="M 12 12 L 12 2 A 10 10 0 0 1 20.07 6.93 L 14.83 10.47 A 4 4 0 0 0 12 8 Z" opacity="0.7"/>
    <path d="M 12 12 L 20.07 6.93 A 10 10 0 0 1 20.07 17.07 L 14.83 13.53 A 4 4 0 0 0 14.83 10.47 Z" opacity="0.5"/>
    <path d="M 12 12 L 20.07 17.07 A 10 10 0 0 1 12 22 L 12 16 A 4 4 0 0 0 14.83 13.53 Z" opacity="0.6"/>
    <path d="M 12 12 L 12 22 A 10 10 0 0 1 3.93 17.07 L 9.17 13.53 A 4 4 0 0 0 12 16 Z" opacity="0.4"/>
    <path d="M 12 12 L 3.93 17.07 A 10 10 0 0 1 3.93 6.93 L 9.17 10.47 A 4 4 0 0 0 9.17 13.53 Z" opacity="0.8"/>
    <path d="M 12 12 L 3.93 6.93 A 10 10 0 0 1 12 2 L 12 8 A 4 4 0 0 0 9.17 10.47 Z" opacity="0.3"/>
    {/* Center hole */}
    <circle cx="12" cy="12" r="4" fill="white"/>
  </svg>
);

// PRESERVED: Kept exactly as-is, working well
export const ScatterIcon = ({ className = "w-6 h-6" }) => (
  <svg className={className} fill="currentColor" viewBox="0 0 24 24">
    {/* Scatter plot dots at various positions */}
    <circle cx="5" cy="18" r="1.5" />
    <circle cx="8" cy="14" r="1.5" />
    <circle cx="11" cy="16" r="1.5" />
    <circle cx="6" cy="10" r="1.5" />
    <circle cx="14" cy="12" r="1.5" />
    <circle cx="10" cy="8" r="1.5" />
    <circle cx="17" cy="15" r="1.5" />
    <circle cx="19" cy="9" r="1.5" />
    <circle cx="15" cy="6" r="1.5" />
    <circle cx="12" cy="20" r="1.5" />
  </svg>
);
