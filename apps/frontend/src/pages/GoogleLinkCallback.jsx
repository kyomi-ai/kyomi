// SPDX-License-Identifier: AGPL-3.0-or-later
import { useEffect, useState } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { XCircle } from 'lucide-react';

export default function GoogleLinkCallback() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { apiClient } = useAuth();
  const [status, setStatus] = useState('processing'); // 'processing', 'success', 'error'
  const [message, setMessage] = useState('Signing in with Google');
  const [isPopup, setIsPopup] = useState(false);
  const [isLoginFlow, setIsLoginFlow] = useState(false);
  const [showHelpText, setShowHelpText] = useState(false);

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

    // Determine if this is login flow vs link flow based on URL path
    const isLogin = window.location.pathname === '/auth/google/callback';
    setIsLoginFlow(isLogin);

    const handleCallback = async () => {
      const code = searchParams.get('code');
      const state = searchParams.get('state');
      const error = searchParams.get('error');

      if (error) {
        setStatus('error');
        setMessage(`Google OAuth error: ${error}`);

        // If in popup, send error message to parent and close
        if (isInPopup) {
          setTimeout(() => {
            window.opener.postMessage({
              type: 'GOOGLE_OAUTH_ERROR',
              error: `Google OAuth error: ${error}`
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
              type: 'GOOGLE_OAUTH_ERROR',
              error: errorMsg
            }, window.location.origin);
            window.close();
          }, 2000);
        }
        return;
      }

      try {
        let response;

        if (isLogin) {
          // For login flow, call the login callback endpoint
          response = await fetch('/api/v1/auth/google/callback', {
            method: 'POST',
            headers: {
              'Content-Type': 'application/json',
            },
            credentials: 'include', // Include cookies for HTTPOnly auth
            body: JSON.stringify({ code, state })
          });

          if (!response.ok) {
            throw new Error(await response.text());
          }

          const data = await response.json();
          response = { data };
        } else {
          // For link flow, call the link callback endpoint (requires authentication)
          response = await apiClient.post('/api/v1/auth/google/link-callback', {
            code,
            state
          });
        }

        setStatus('success');

        if (isInPopup) {
          // If in popup, send success message to parent and close
          setTimeout(() => {
            window.opener.postMessage({
              type: 'GOOGLE_OAUTH_SUCCESS',
              data: response.data
            }, window.location.origin);
            window.close();
          }, 1000);
        } else {
          // If not in popup, redirect after 1.5 seconds
          setTimeout(() => {
            if (isLogin) {
              // For login flow, existing user goes straight to home
              window.location.href = '/';
            } else {
              // For link flow, redirect to settings profile tab
              navigate('/settings/profile', { replace: true });
            }
          }, 1500);
        }

      } catch (error) {
        const errorMsg = error.response?.data?.detail || error.message || (isLogin ? 'Failed to sign in with Google' : 'Failed to link Google account');
        setStatus('error');
        setMessage(errorMsg);

        // If in popup, send error message to parent and close
        if (isInPopup) {
          setTimeout(() => {
            window.opener.postMessage({
              type: 'GOOGLE_OAUTH_ERROR',
              error: errorMsg
            }, window.location.origin);
            window.close();
          }, 2000);
        }
      }
    };

    handleCallback();
  }, [searchParams, apiClient, navigate]);

  const StatusIcon = () => {
    switch (status) {
      case 'error':
        return <XCircle size={48} className="text-error-foreground" />;
      default:
        return <img src="/kyomi_animated_logo.svg" alt="Processing" className="w-12 h-12" />;
    }
  };

  return (
    <div className="min-h-screen bg-background flex items-center justify-center px-4">
      <div className="max-w-md w-full">
        <div className="bg-background p-8 text-center">
          {/* Icon - changes from animated logo to green tick */}
          <div className="flex justify-center">
            <StatusIcon />
          </div>

          {/* Error state - show message and buttons */}
          {status === 'error' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                {isLoginFlow ? 'Google Sign-In' : 'Google Account Linking'}
              </h2>
              <p className="text-error-foreground mb-4">
                {message}
              </p>
              {!isPopup && (
                <div className="space-y-3">
                  <button
                    onClick={() => navigate('/settings/profile')}
                    className="w-full px-4 py-2 bg-muted-foreground text-primary-foreground rounded-xl hover:bg-foreground transition-colors"
                  >
                    Return to Profile
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
            <p>If this page doesn't automatically redirect, you can close it and return to your profile.</p>
          </div>
        )}
      </div>
    </div>
  );
}