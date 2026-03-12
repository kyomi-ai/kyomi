// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * usePalettePreference Hook
 *
 * Returns the user's chart palette preference from AuthContext.
 * Returns default 'balanced' palette if no preference is set.
 *
 * @returns {Array} Array of color strings (e.g., ['#ff0000', '#00ff00', ...])
 *
 * @example
 * function MyChartComponent() {
 *   const palette = usePalettePreference();
 *   return <Chart colors={palette} />;
 * }
 */

import { useMemo } from 'react';
import { useAuth } from '../context/AuthContext';
import { CHART_PALETTES } from '../config/chartPalettes';

export function usePalettePreference() {
  const { userChartMLConfig } = useAuth();

  return useMemo(() => {
    const paletteName = userChartMLConfig?.style;
    if (paletteName && CHART_PALETTES[paletteName]) {
      return CHART_PALETTES[paletteName];
    }
    return CHART_PALETTES.balanced;
  }, [userChartMLConfig]);
}
