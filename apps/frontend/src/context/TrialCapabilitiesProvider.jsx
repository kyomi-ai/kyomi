// SPDX-License-Identifier: AGPL-3.0-or-later
import React from 'react';
import { CapabilitiesContext } from './CapabilitiesContext';
import { AuthContext } from './AuthContext';

/**
 * TrialCapabilitiesProvider - Provides restrictive capabilities for trial/anonymous users.
 *
 * This is used on the /try page where users aren't authenticated.
 * All capabilities are disabled/restrictive - this only controls UI visibility.
 * Actual security is enforced by the backend (trial endpoint restrictions, rate limits).
 *
 * Uses the same CapabilitiesContext and AuthContext so hooks work seamlessly.
 */

// Static trial auth - no user, no API client
const TRIAL_AUTH = {
  user: null,
  apiClient: null,
  isAuthenticated: false,
  isLoading: false,
  authState: 'unauthenticated',
  login: () => Promise.reject(new Error('Not available in trial mode')),
  logout: () => {},
  refreshAuth: () => Promise.resolve(),
};

// Static trial capabilities - everything restrictive
const TRIAL_CAPABILITIES = {
  // Raw capabilities object (used by ChartML)
  capabilities: {
    ai_chat_enabled: false,
    ai_sql_generation_enabled: false,
    ai_autocomplete_enabled: false,
    ai_chart_copilot_enabled: false,
    user_management_enabled: false,
    dashboard_sharing_enabled: false,
    export_enabled: false,
    api_access_enabled: false,
    subscription_tier: 'trial',
    credits_exhausted: false,
    credits_remaining: 0,
    credits_limit: 0,
    percentage_used: 0,
    max_dashboards: 0,
    kyomi_watch_enabled: false,
    arrow_streaming_enabled: false,
    multi_user_enabled: false,
    slack_integration_enabled: false,
    mcp_access_enabled: false,
    bigquery_mode: null,
    bigquery_access_level: 'none',
  },
  loading: false,
  can: () => false,  // All feature checks return false
  refetch: () => {},  // No-op for trial

  // Getters (match CapabilitiesContext interface)
  get aiEnabled() { return false; },
  get creditsExhausted() { return false; },
  get subscriptionTier() { return 'trial'; },
  get bigqueryMode() { return null; },
  get bigqueryAccessLevel() { return 'none'; },
  get isEnterprise() { return false; },
  get creditsRemaining() { return 0; },
  get creditsLimit() { return 0; },
  get creditsPercentageUsed() { return 0; },
  get max_dashboards() { return 0; },
  get kyomiWatchEnabled() { return false; },
  get arrowStreamingEnabled() { return false; },
  get multiUserEnabled() { return false; },
  get slackIntegrationEnabled() { return false; },
  get mcpAccessEnabled() { return false; },
};

export const TrialCapabilitiesProvider = ({ children }) => {
  return (
    <AuthContext.Provider value={TRIAL_AUTH}>
      <CapabilitiesContext.Provider value={TRIAL_CAPABILITIES}>
        {children}
      </CapabilitiesContext.Provider>
    </AuthContext.Provider>
  );
};
