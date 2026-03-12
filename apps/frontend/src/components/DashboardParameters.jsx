// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useRef, useEffect, useCallback } from 'react';
import { useDashboard } from '../context/DashboardContext';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';

/**
 * MultiSelectDropdown - Extracted as separate component to avoid closure issues
 */
const MultiSelectDropdown = ({ filter, scope, currentValue, onChange }) => {
  const { openDropdown, updateDropdown } = useDashboard();
  const key = scope ? `${scope}.${filter.id}` : filter.id;
  const selectedCount = currentValue.length;
  const dropdownRef = useRef(null);
  // Use the scoped key for dropdown ID to prevent conflicts across charts
  const isOpen = openDropdown === key;

  // Track when currentValue prop changes
  useEffect(() => {
  }, [currentValue]);

  // Close dropdown when clicking outside
  useEffect(() => {
    const handleClickOutside = (event) => {
      if (dropdownRef.current && !dropdownRef.current.contains(event.target)) {
        updateDropdown(null);
      }
    };

    if (isOpen) {
      document.addEventListener('mousedown', handleClickOutside);
      return () => document.removeEventListener('mousedown', handleClickOutside);
    }
  }, [isOpen, updateDropdown]);

  const handleCheckboxChange = (option) => {
    const newValueForThisFilter = currentValue.includes(option)
      ? currentValue.filter(v => v !== option)
      : [...currentValue, option];

    // Call onChange with the key and new value
    onChange(key, newValueForThisFilter);

  };

  return (
    <div ref={dropdownRef} className="relative">
      <button
        onClick={() => updateDropdown(isOpen ? null : key)}
        className="w-full px-3 py-2 text-left bg-background border border-border rounded-md text-sm hover:border-ring/50 focus:outline-none focus:ring-2 focus:ring-ring flex items-center justify-between"
      >
        <span className="truncate">
          {selectedCount === 0 ? 'Select...' : `${selectedCount} selected`}
        </span>
        <svg className="w-4 h-4 ml-2 text-muted-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
          <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M19 9l-7 7-7-7" />
        </svg>
      </button>

      {isOpen && (
        <div className="absolute z-10 mt-1 w-full bg-popover border border-border rounded-md shadow-lg max-h-60 overflow-auto">
          {filter.options.map(option => (
            <label
              key={option}
              className="flex items-center px-3 py-2 hover:bg-accent cursor-pointer"
            >
              <input
                type="checkbox"
                checked={currentValue.includes(option)}
                onChange={(e) => {
                  e.stopPropagation();
                  handleCheckboxChange(option);
                }}
                className="w-4 h-4 rounded border-input text-primary focus:ring-ring mr-2"
              />
              <span className="text-sm text-foreground">{option}</span>
            </label>
          ))}
        </div>
      )}
    </div>
  );
};

/**
 * DashboardParameters - Renders interactive parameter controls for dashboards
 *
 * Design inspired by modern BI tools (Tableau, Looker, Power BI):
 * - Compact horizontal layout
 * - Dropdowns for all filter types (saves space)
 * - Multi-select shows selected count as badge
 * - Everything inline in one row when possible
 *
 * Supports scoped parameters for chart-level params (e.g., "chart_0_0.selected_regions")
 */
const DashboardParameters = ({ parameterDefinitions, scope }) => {
  const { parameterValues, updateParameters } = useDashboard();

  // Initialize parameter values with defaults on mount
  // Only set values that don't already exist in context
  useEffect(() => {
    const initialValues = {};
    let hasNewValues = false;

    parameterDefinitions.forEach(param => {
      const key = scope ? `${scope}.${param.id}` : param.id;

      // Only initialize if this parameter doesn't already have a value
      if (!(key in parameterValues) && param.default !== undefined) {
        initialValues[key] = param.default;
        hasNewValues = true;
      }
    });

    if (hasNewValues) {
      updateParameters(prevValues => ({
        ...prevValues,
        ...initialValues
      }));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [parameterDefinitions, scope, updateParameters]);

  const handleFilterChange = useCallback((key, newValue) => {
    // key is already scoped (e.g., "chart_0_0.selected_regions" or just "filterId")
    updateParameters((prevValues) => ({
      ...prevValues,
      [key]: newValue
    }));
  }, [updateParameters]);

  // Get column span class for a parameter (from layout.colSpan or auto-calculated)
  // Must use explicit class names for Tailwind JIT compiler
  // Responsive: mobile = full width (col-span-12), desktop = calculated span
  const getColSpanClass = (filter) => {
    // If user specified layout.colSpan, use that for desktop
    if (filter.layout?.colSpan) {
      const classMap = {
        1: 'col-span-12 md:col-span-1',
        2: 'col-span-12 md:col-span-2',
        3: 'col-span-12 md:col-span-3',
        4: 'col-span-12 md:col-span-4',
        5: 'col-span-12 md:col-span-5',
        6: 'col-span-12 md:col-span-6',
        7: 'col-span-12 md:col-span-7',
        8: 'col-span-12 md:col-span-8',
        9: 'col-span-12 md:col-span-9',
        10: 'col-span-12 md:col-span-10',
        11: 'col-span-12 md:col-span-11',
        12: 'col-span-12',
      };
      return classMap[filter.layout.colSpan] || 'col-span-12 md:col-span-3';
    }

    // Auto-calculate based on total number of parameters
    // 1 param = 12 cols (full width), 2 = 6 cols each, 3 = 4 cols each, 4+ = 3 cols each
    const totalParams = parameterDefinitions.length;
    const autoColSpan = totalParams === 1 ? 12 : totalParams === 2 ? 6 : totalParams === 3 ? 4 : 3;

    const classMap = {
      3: 'col-span-12 md:col-span-3',
      4: 'col-span-12 md:col-span-4',
      6: 'col-span-12 md:col-span-6',
      12: 'col-span-12',
    };

    return classMap[autoColSpan] || 'col-span-12 md:col-span-3';
  };

  const renderFilter = (filter) => {
    const key = scope ? `${scope}.${filter.id}` : filter.id;
    // Use ?? instead of || to avoid treating empty arrays as falsy
    const currentValue = parameterValues[key] ?? filter.default;
    const gridClass = getColSpanClass(filter);

    switch (filter.type) {
      case 'select':
        return (
          <div key={filter.id} className={gridClass}>
            <label className="block text-xs font-medium text-muted-foreground mb-1">
              {filter.label}
            </label>
            <Select
              value={currentValue || ''}
              onValueChange={(value) => handleFilterChange(key, value)}
            >
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Select..." />
              </SelectTrigger>
              <SelectContent>
                {filter.options.map(option => (
                  <SelectItem key={option} value={option}>{option}</SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        );

      case 'multiselect':
        return (
          <div key={filter.id} className={gridClass}>
            <label className="block text-xs font-medium text-muted-foreground mb-1">
              {filter.label}
            </label>
            <MultiSelectDropdown
              filter={filter}
              scope={scope}
              currentValue={currentValue}
              onChange={handleFilterChange}
            />
          </div>
        );

      case 'daterange':
        return (
          <div key={filter.id} className={gridClass}>
            <label className="block text-xs font-medium text-muted-foreground mb-1">
              {filter.label}
            </label>
            <div className="flex items-center gap-2">
              <input
                type="date"
                value={currentValue?.start || ''}
                onChange={(e) => handleFilterChange(filter.id, {
                  ...(currentValue || {}),
                  start: e.target.value
                })}
                className="flex-1 px-3 py-2 bg-background border border-input rounded-md text-sm hover:border-ring/50 focus:outline-none focus:ring-2 focus:ring-ring"
              />
              <span className="text-xs text-muted-foreground">to</span>
              <input
                type="date"
                value={currentValue?.end || ''}
                onChange={(e) => handleFilterChange(filter.id, {
                  ...(currentValue || {}),
                  end: e.target.value
                })}
                className="flex-1 px-3 py-2 bg-background border border-input rounded-md text-sm hover:border-ring/50 focus:outline-none focus:ring-2 focus:ring-ring"
              />
            </div>
          </div>
        );

      case 'number':
        return (
          <div key={filter.id} className={gridClass}>
            <label className="block text-xs font-medium text-muted-foreground mb-1">
              {filter.label}
            </label>
            <input
              type="number"
              value={currentValue || ''}
              onChange={(e) => handleFilterChange(filter.id, parseFloat(e.target.value))}
              className="w-full px-3 py-2 bg-background border border-input rounded-md text-sm hover:border-ring/50 focus:outline-none focus:ring-2 focus:ring-ring"
            />
          </div>
        );

      case 'text':
        return (
          <div key={filter.id} className={gridClass}>
            <label className="block text-xs font-medium text-muted-foreground mb-1">
              {filter.label}
            </label>
            <input
              type="text"
              value={currentValue || ''}
              onChange={(e) => handleFilterChange(filter.id, e.target.value)}
              className="w-full px-3 py-2 bg-background border border-input rounded-md text-sm hover:border-ring/50 focus:outline-none focus:ring-2 focus:ring-ring"
              placeholder={filter.placeholder}
            />
          </div>
        );

      default:
        return (
          <div key={filter.id} className={gridClass}>
            <p className="text-xs text-error-foreground">Unknown: {filter.type}</p>
          </div>
        );
    }
  };

  return (
    <div className="dashboard-filters bg-card border border-border rounded-lg p-4 mb-6">
      <div className="grid grid-cols-12 gap-4">
        {parameterDefinitions.map(renderFilter)}
      </div>
    </div>
  );
};

export default DashboardParameters;
