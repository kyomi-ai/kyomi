// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import apiClient from '../api/apiClient';

/**
 * useDatasources - Hook to fetch and track available datasources
 *
 * Returns loading state, datasources list, and convenience helpers.
 * Used for determining if datasources are configured (empty state detection).
 */
export default function useDatasources() {
  const [datasources, setDatasources] = useState([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  useEffect(() => {
    let isMounted = true;

    const fetchDatasources = async () => {
      try {
        setLoading(true);
        setError(null);
        const response = await apiClient.get('/api/v1/datasources');
        if (isMounted) {
          setDatasources(response.data || []);
        }
      } catch (err) {
        if (isMounted) {
          setError(err.message || 'Failed to load datasources');
        }
      } finally {
        if (isMounted) {
          setLoading(false);
        }
      }
    };

    fetchDatasources();

    return () => {
      isMounted = false;
    };
  }, []);

  return {
    datasources,
    loading,
    error,
    hasDatasources: datasources.length > 0,
    datasourceCount: datasources.length,
  };
}
