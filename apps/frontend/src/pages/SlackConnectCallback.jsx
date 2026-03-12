// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Slack Connect Callback Page
 *
 * Handles the callback from /kyomi connect command in Slack.
 * Route: /auth/slack-connect
 *
 * Flow:
 * 1. User types /kyomi connect in Slack
 * 2. Backend generates state and returns link to /auth/slack-connect?state={state}
 * 3. User clicks link and lands here (already authenticated with Kyomi)
 * 4. We call /api/v1/slack/connect?state={state} to link the accounts
 * 5. Redirect to profile settings on success
 */

import { useEffect, useState } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { XCircle, CheckCircle } from 'lucide-react';

export default function SlackConnectCallback() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { apiClient } = useAuth();

  const [status, setStatus] = useState('processing'); // 'processing', 'success', 'error'
  const [message, setMessage] = useState('Connecting your Slack account...');
  const [showHelpText, setShowHelpText] = useState(false);

  // Show help text after 10 seconds if redirect hasn't happened
  useEffect(() => {
    const timer = setTimeout(() => {
      setShowHelpText(true);
    }, 10000);

    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    const handleCallback = async () => {
      const state = searchParams.get('state');

      if (!state) {
        setStatus('error');
        setMessage('Missing state parameter. Please run /kyomi connect again in Slack.');
        return;
      }

      try {
        // Get OAuth URL from backend - this enriches the state with user_id
        // and returns the Slack OAuth URL to redirect to
        const response = await apiClient.get(`/api/v1/slack/connect/initiate?state=${state}`);

        if (response.data.authorization_url) {
          // Redirect to Slack OAuth
          setMessage('Redirecting to Slack...');
          window.location.href = response.data.authorization_url;
        } else {
          // Fallback for old flow or error
          setStatus('error');
          setMessage('Failed to get Slack authorization URL');
        }

      } catch (error) {
        const errorMsg = error.response?.data?.detail || error.message || 'Failed to connect Slack account';
        setStatus('error');
        setMessage(errorMsg);
      }
    };

    handleCallback();
  }, [searchParams, apiClient, navigate]);

  const StatusIcon = () => {
    if (status === 'error') {
      return <XCircle size={48} className="text-error-foreground" />;
    }
    if (status === 'success') {
      return <CheckCircle size={48} className="text-success-foreground" />;
    }
    return <img src="/kyomi_animated_logo.svg" alt="Processing" className="w-12 h-12" />;
  };

  return (
    <div className="min-h-screen bg-background flex items-center justify-center px-4">
      <div className="max-w-md w-full">
        <div className="bg-background p-8 text-center">
          {/* Icon */}
          <div className="flex justify-center">
            <StatusIcon />
          </div>

          {/* Success state */}
          {status === 'success' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                Slack Connected
              </h2>
              <p className="text-muted-foreground">
                {message}
              </p>
              <p className="text-sm text-muted-foreground mt-2">
                Redirecting to your profile settings...
              </p>
            </>
          )}

          {/* Processing state */}
          {status === 'processing' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                Connecting Slack
              </h2>
              <p className="text-muted-foreground">
                Please wait...
              </p>
            </>
          )}

          {/* Error state */}
          {status === 'error' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                Slack Connection Failed
              </h2>
              <p className="text-error-foreground mb-4">
                {message}
              </p>
              <div className="space-y-3">
                <button
                  onClick={() => navigate('/settings/profile')}
                  className="w-full px-4 py-2 bg-muted-foreground text-primary-foreground rounded-xl hover:bg-foreground transition-colors"
                >
                  Return to Profile Settings
                </button>
                <button
                  onClick={() => window.location.reload()}
                  className="w-full px-4 py-2 bg-primary text-white rounded-xl hover:bg-primary/90 transition-colors"
                >
                  Try Again
                </button>
              </div>
            </>
          )}
        </div>

        {/* Help text */}
        {showHelpText && status !== 'error' && (
          <div className="mt-6 text-center text-sm text-muted-foreground">
            <p>If this page doesn't automatically redirect, you can close it and return to your settings.</p>
          </div>
        )}
      </div>
    </div>
  );
}
