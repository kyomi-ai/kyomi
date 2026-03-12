// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Generic OAuth Callback Page
 *
 * Handles OAuth callbacks for any provider via route parameter.
 * Route: /auth/oauth/:provider/callback
 *
 * Supports both popup and full-page flows.
 * For popup: Posts message to opener and closes
 * For full-page: Redirects to appropriate destination
 */

import { useEffect, useState } from 'react';
import { useParams, useSearchParams, useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { XCircle } from 'lucide-react';

export default function OAuthCallback() {
  const { provider } = useParams();
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { apiClient } = useAuth();

  const [status, setStatus] = useState('processing'); // 'processing', 'success', 'error'
  const [message, setMessage] = useState(`Connecting ${provider}...`);
  const [isPopup, setIsPopup] = useState(false);
  const [showHelpText, setShowHelpText] = useState(false);

  // Capitalize provider name for display
  const providerDisplayName = provider ? provider.charAt(0).toUpperCase() + provider.slice(1) : 'OAuth';

  // Normalize provider name for message type (replace hyphens with underscores for consistency)
  const providerMessageKey = provider ? provider.toUpperCase().replace(/-/g, '_') : 'OAUTH';

  // Show help text after 10 seconds if redirect hasn't happened
  useEffect(() => {
    const timer = setTimeout(() => {
      setShowHelpText(true);
    }, 10000);

    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    // Check if we're running in a popup window
    const isInPopup = window.opener && window.opener !== window;
    setIsPopup(isInPopup);

    const handleCallback = async () => {
      const code = searchParams.get('code');
      const state = searchParams.get('state');
      const error = searchParams.get('error');

      if (error) {
        setStatus('error');
        setMessage(`${providerDisplayName} OAuth error: ${error}`);

        // If in popup, send error message to parent and close
        if (isInPopup) {
          setTimeout(() => {
            window.opener.postMessage({
              type: `${providerMessageKey}_OAUTH_ERROR`,
              provider,
              error: `${providerDisplayName} OAuth error: ${error}`
            }, window.location.origin);
            window.close();
          }, 2000);
        }
        return;
      }

      if (!code || !state) {
        const errorMsg = 'Missing authorization code or state parameter';
        setStatus('error');
        setMessage(errorMsg);

        // If in popup, send error message to parent and close
        if (isInPopup) {
          setTimeout(() => {
            window.opener.postMessage({
              type: `${providerMessageKey}_OAUTH_ERROR`,
              provider,
              error: errorMsg
            }, window.location.origin);
            window.close();
          }, 2000);
        }
        return;
      }

      try {
        // Call the generic OAuth callback endpoint
        const response = await apiClient.post(`/api/v1/auth/oauth/${provider}/callback`, {
          code,
          state
        });

        setStatus('success');
        setMessage(`${providerDisplayName} account linked successfully`);

        if (isInPopup) {
          // If in popup, send success message to parent and close
          setTimeout(() => {
            window.opener.postMessage({
              type: `${providerMessageKey}_OAUTH_SUCCESS`,
              provider,
              data: response.data
            }, window.location.origin);
            window.close();
          }, 1000);
        } else {
          // If not in popup, redirect to settings after 1.5 seconds
          setTimeout(() => {
            navigate('/settings/datasources', { replace: true });
          }, 1500);
        }

      } catch (error) {
        const errorMsg = error.response?.data?.detail || error.message || `Failed to link ${providerDisplayName} account`;
        setStatus('error');
        setMessage(errorMsg);

        // If in popup, send error message to parent and close
        if (isInPopup) {
          setTimeout(() => {
            window.opener.postMessage({
              type: `${providerMessageKey}_OAUTH_ERROR`,
              provider,
              error: errorMsg
            }, window.location.origin);
            window.close();
          }, 2000);
        }
      }
    };

    handleCallback();
  }, [provider, providerMessageKey, searchParams, apiClient, navigate, providerDisplayName]);

  const StatusIcon = () => {
    if (status === 'error') {
      return <XCircle size={48} className="text-error-foreground" />;
    }
    return <img src="/kyomi_animated_logo.svg" alt="Processing" className="w-12 h-12" />;
  };

  return (
    <div className="min-h-screen bg-background flex items-center justify-center px-4">
      <div className="max-w-md w-full">
        <div className="bg-background p-8 text-center">
          {/* Icon - changes from animated logo to error icon */}
          <div className="flex justify-center">
            <StatusIcon />
          </div>

          {/* Success state */}
          {status === 'success' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                {providerDisplayName} Connected
              </h2>
              <p className="text-muted-foreground">
                {message}
              </p>
            </>
          )}

          {/* Processing state */}
          {status === 'processing' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                Connecting {providerDisplayName}
              </h2>
              <p className="text-muted-foreground">
                Please wait...
              </p>
            </>
          )}

          {/* Error state - show message and buttons */}
          {status === 'error' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                {providerDisplayName} Connection Failed
              </h2>
              <p className="text-error-foreground mb-4">
                {message}
              </p>
              {!isPopup && (
                <div className="space-y-3">
                  <button
                    onClick={() => navigate('/settings/datasources')}
                    className="w-full px-4 py-2 bg-muted-foreground text-primary-foreground rounded-xl hover:bg-foreground transition-colors"
                  >
                    Return to Datasources
                  </button>
                  <button
                    onClick={() => window.location.reload()}
                    className="w-full px-4 py-2 bg-primary text-white rounded-xl hover:bg-primary/90 transition-colors"
                  >
                    Try Again
                  </button>
                </div>
              )}
            </>
          )}
        </div>

        {/* Help text - only appears after delay */}
        {showHelpText && !isPopup && status !== 'error' && (
          <div className="mt-6 text-center text-sm text-muted-foreground">
            <p>If this page doesn't automatically redirect, you can close it and return to your settings.</p>
          </div>
        )}
      </div>
    </div>
  );
}
