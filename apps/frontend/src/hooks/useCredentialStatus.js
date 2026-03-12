// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useCallback } from 'react';
import apiClient from '../api/apiClient';

/**
 * useCredentialStatus - Hook to check credential status for all workspace datasources
 *
 * Used by the onboarding flow to determine which datasources need credentials
 * from an invited user joining an existing workspace.
 *
 * Returns:
 * - datasources: List of datasources with their credential status
 * - summary: { total, ready, needs_credentials, needs_oauth, needs_password }
 * - loading: Whether the fetch is in progress
 * - error: Error message if fetch failed
 * - refetch: Function to manually refresh the status
 */
export default function useCredentialStatus() {
  const [datasources, setDatasources] = useState([]);
  const [summary, setSummary] = useState({
    total: 0,
    ready: 0,
    needs_credentials: 0,
    needs_oauth: 0,
    needs_password: 0,
  });
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  const fetchCredentialStatus = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      const response = await apiClient.get('/api/v1/datasources/credential-status');
      setDatasources(response.data?.datasources || []);
      setSummary(response.data?.summary || {
        total: 0,
        ready: 0,
        needs_credentials: 0,
        needs_oauth: 0,
        needs_password: 0,
      });
    } catch (err) {
      setError(err.message || 'Failed to load credential status');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    fetchCredentialStatus();
  }, [fetchCredentialStatus]);

  return {
    datasources,
    summary,
    loading,
    error,
    refetch: fetchCredentialStatus,
    // Convenience helpers
    allReady: summary.ready === summary.total && summary.total > 0,
    noneReady: summary.ready === 0 && summary.total > 0,
    hasDatasources: summary.total > 0,
  };
}
