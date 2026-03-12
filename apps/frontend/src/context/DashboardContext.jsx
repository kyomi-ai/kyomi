// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { createContext, useContext, useState, useCallback, useMemo } from 'react';

/**
 * DashboardContext - Centralized state management for dashboard parameters and UI
 *
 * This context solves the re-render cascade problem by:
 * 1. Providing parameter state directly to consumers (no prop drilling)
 * 2. Using separate state atoms to prevent unnecessary re-renders
 * 3. Memoizing callbacks to ensure referential stability
 *
 * Benefits:
 * - Charts only re-render when their specific parameter values change
 * - Dropdown state changes don't trigger chart re-renders
 * - No useMemo dependency issues
 * - Clean component boundaries
 */

const DashboardContext = createContext(null);

export const DashboardProvider = ({ children }) => {
  // Separate state atoms prevent re-render cascades
  const [parameterValues, setFilterValues] = useState({});
  const [openDropdown, setOpenDropdown] = useState(null);

  // Stable callback - never changes reference
  // Supports both function and object forms (like setState)
  // Pass directly to setFilterValues - React's setState handles both
  const updateParameters = useCallback((newValues) => {
    setFilterValues(newValues);
  }, []);

  // Stable callback - never changes reference
  const updateDropdown = useCallback((dropdownId) => {
    setOpenDropdown(dropdownId);
  }, []);

  // Memoize context value - only changes when state changes
  const value = useMemo(() => ({
    parameterValues,
    updateParameters,
    openDropdown,
    updateDropdown
  }), [parameterValues, openDropdown, updateParameters, updateDropdown]);

  return (
    <DashboardContext.Provider value={value}>
      {children}
    </DashboardContext.Provider>
  );
};

export const useDashboard = () => {
  const context = useContext(DashboardContext);
  if (!context) {
    // Return default values when used outside DashboardProvider (e.g., in SQL Editor)
    return {
      parameterValues: {},
      updateParameters: () => {},
      openDropdown: null,
      updateDropdown: () => {}
    };
  }
  return context;
};
