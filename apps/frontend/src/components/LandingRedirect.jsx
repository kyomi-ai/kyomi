// SPDX-License-Identifier: AGPL-3.0-or-later
import { Navigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { useAuth } from '../context/AuthContext';
import apiClient from '../api/apiClient';
import Chat from '../pages/Chat';

/**
 * Resolves the user's landing page preference and either renders Chat
 * directly (default) or redirects to the appropriate page in a single hop.
 */
export default function LandingRedirect() {
  const { user } = useAuth();
  const landingPage = user?.extra_metadata?.landing_page || 'chat';

  // Fetch workspace default dashboard (shared cache with Sidebar)
  const { data: workspaceDefault, isLoading: isLoadingDefault } = useQuery({
    queryKey: ['default-dashboard'],
    queryFn: async () => {
      const response = await apiClient.get('/api/v1/workspaces/default-dashboard');
      return response.data;
    },
    staleTime: 60000,
  });

  // For chat (default), render directly — no redirect, no URL change
  if (landingPage === 'chat') {
    return <Chat />;
  }

  // For non-dashboard pages, single redirect
  if (landingPage === 'watches') {
    return <Navigate to="/watches" replace />;
  }
  if (landingPage === 'sql_editor') {
    return <Navigate to="/sql-editor" replace />;
  }

  // For dashboards, resolve the default dashboard chain: user > workspace > list
  if (landingPage === 'dashboards') {
    const userDefault = user?.extra_metadata?.default_dashboard_id;

    // If user has their own default, use it immediately
    if (userDefault) {
      return <Navigate to={`/dashboard/${userDefault}`} replace />;
    }

    // Wait for workspace default to load before deciding
    if (isLoadingDefault) {
      return null;
    }

    const workspaceDefaultId = workspaceDefault?.default_dashboard_id;
    if (workspaceDefaultId) {
      return <Navigate to={`/dashboard/${workspaceDefaultId}`} replace />;
    }
    return <Navigate to="/dashboards" replace />;
  }

  // Fallback: render Chat
  return <Chat />;
}
