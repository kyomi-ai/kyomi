// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { Link } from 'react-router-dom';
import apiClient from '../api/apiClient';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from './ui/select';
import { DatasourceIcon } from './ui/DatasourceIcon';
import { Spinner } from './ui/spinner';
import { Settings } from 'lucide-react';

/**
 * DatasourceSelector - Dropdown to select active datasource
 *
 * Fetches available datasources from the backend and allows user to select one.
 * Used in SQL Editor to filter catalog, search, and query execution.
 *
 * Only shows datasources that the user can actually query:
 * - can_enable: true (user has valid credentials or it's shared-auth)
 * - user_enabled: true (user hasn't disabled it)
 *
 * @param {string} value - Selected datasource slug (e.g., "production-postgres")
 * @param {function} onChange - Callback when selection changes: (slug, datasource) => void
 *   - slug: The selected datasource slug (for API calls and state)
 *   - datasource: Full datasource object {id, slug, name, datasource_type, ...}
 * @param {string} className - Additional CSS classes
 * @param {boolean} renderWhenEmpty - If false, render nothing when no datasources (default: true)
 */
const DatasourceSelector = ({ value, onChange, className = '', renderWhenEmpty = true }) => {
  const [datasources, setDatasources] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  // Fetch datasources with credential status on mount
  useEffect(() => {
    const fetchDatasources = async () => {
      try {
        setLoading(true);
        setError(null);
        const response = await apiClient.get('/api/v1/datasources/credential-status');
        const allDatasources = response.data?.datasources || [];

        // Filter to only datasources the user can actually query
        // can_enable: user has valid credentials or it's shared-auth
        // user_enabled: user hasn't disabled it
        const accessibleDatasources = allDatasources.filter(
          ds => ds.can_enable && ds.user_enabled
        );

        setDatasources(accessibleDatasources);

        // Check if current value exists in the accessible list (by slug)
        const valueExists = value && accessibleDatasources.some(ds => ds.slug === value);

        if (valueExists) {
          // Value exists - update parent with full datasource object
          const selectedDatasource = accessibleDatasources.find(ds => ds.slug === value);
          onChange(value, selectedDatasource);
        } else if (accessibleDatasources.length > 0) {
          // Value doesn't exist or is null - auto-select first datasource
          onChange(accessibleDatasources[0].slug, accessibleDatasources[0]);
        }
      } catch (err) {
        setError(err.message || 'Failed to load datasources');
      } finally {
        setLoading(false);
      }
    };

    fetchDatasources();
  }, []);

  // Update parent when value changes externally (e.g., from tab switch)
  useEffect(() => {
    if (!loading && datasources.length > 0 && value) {
      const selectedDatasource = datasources.find(ds => ds.slug === value);
      if (selectedDatasource) {
        onChange(value, selectedDatasource);
      }
    }
  }, [value, datasources, loading, onChange]);

  // Handle selection change
  const handleValueChange = (newValue) => {
    const selectedDatasource = datasources.find(ds => ds.slug === newValue);
    onChange(newValue, selectedDatasource);
  };

  if (loading) {
    if (!renderWhenEmpty) {
      return null; // Don't show loading state if we're hiding when empty
    }
    return (
      <div className={`flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground ${className}`}>
        <Spinner className="text-muted-foreground" />
        <span>Loading datasources...</span>
      </div>
    );
  }

  if (error) {
    if (!renderWhenEmpty) {
      return null; // Hide errors too when renderWhenEmpty is false
    }
    return (
      <div className={`px-3 py-2 text-sm text-error-foreground ${className}`}>
        Error: {error}
      </div>
    );
  }

  if (datasources.length === 0) {
    if (!renderWhenEmpty) {
      return null;
    }
    return (
      <div className={`flex items-center gap-2 px-3 py-2 text-sm text-muted-foreground ${className}`}>
        <span>No datasources available.</span>
        <Link
          to="/settings"
          className="inline-flex items-center gap-1 text-primary hover:underline"
        >
          <Settings className="h-3 w-3" />
          <span>Connect in Settings</span>
        </Link>
      </div>
    );
  }

  return (
    <Select value={value} onValueChange={handleValueChange}>
      <SelectTrigger className={`w-[140px] sm:w-[240px] ${className}`}>
        <SelectValue placeholder="Select datasource..." />
      </SelectTrigger>
      <SelectContent>
        {datasources.map((ds) => (
          <SelectItem key={ds.slug} value={ds.slug}>
            <div className="flex items-center gap-2">
              <DatasourceIcon type={ds.datasource_type} className="h-4 w-4" opacity={0.8} />
              <span className="font-medium">{ds.name}</span>
            </div>
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
};

export default DatasourceSelector;
