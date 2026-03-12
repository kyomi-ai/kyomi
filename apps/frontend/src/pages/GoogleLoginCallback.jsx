// SPDX-License-Identifier: AGPL-3.0-or-later
import { useEffect, useState, useRef } from 'react';
import { useSearchParams, useNavigate } from 'react-router-dom';
import { XCircle } from 'lucide-react';

/**
 * OAuth callback handler for Google LOGIN flow (not link flow)
 * This component does NOT require AuthProvider since it's for initial login
 */
export default function GoogleLoginCallback() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const [status, setStatus] = useState('processing');
  const [message, setMessage] = useState('Signing in with Google');
  const [isPopup, setIsPopup] = useState(false);
  const [showHelpText, setShowHelpText] = useState(false);
  const hasRun = useRef(false);

  // Show help text after 10 seconds if redirect hasn't happened
  useEffect(() => {
    const timer = setTimeout(() => {
      setShowHelpText(true);
    }, 10000);
    return () => clearTimeout(timer);
  }, []);

  useEffect(() => {
    // Prevent duplicate calls in React StrictMode
    if (hasRun.current) return;
    hasRun.current = true;
    const isInPopup = window.opener && window.opener !== window;
    setIsPopup(isInPopup);

    const handleCallback = async () => {
      const code = searchParams.get('code');
      const state = searchParams.get('state');
      const error = searchParams.get('error');

      if (error) {
        setStatus('error');
        setMessage(`Google OAuth error: ${error}`);
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
        const response = await fetch('/api/v1/auth/google/callback', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          credentials: 'include',
          body: JSON.stringify({ code, state })
        });

        if (!response.ok) {
          throw new Error(await response.text());
        }

        const data = await response.json();

        // Check if terms acceptance is required
        if (data.status === 'pending_terms') {
          // User needs to accept terms - redirect to welcome page
          setStatus('success');
          setMessage('Please accept our Terms of Service to continue');

          if (isInPopup) {
            setTimeout(() => {
              window.opener.postMessage({
                type: 'GOOGLE_OAUTH_TERMS_REQUIRED',
                data: data
              }, window.location.origin);
              window.close();
            }, 1000);
          } else {
            // Redirect to welcome page with temp token
            setTimeout(() => {
              window.location.href = data.redirect_url || '/welcome';
            }, 1500);
          }
        } else {
          // Normal login success
          setStatus('success');

          if (isInPopup) {
            setTimeout(() => {
              window.opener.postMessage({
                type: 'GOOGLE_OAUTH_SUCCESS',
                data: data
              }, window.location.origin);
              window.close();
            }, 1000);
          } else {
            // Check for MCP OAuth continuation (oauth_continue)
            if (data.oauth_continue) {
              // Continue MCP OAuth flow - redirect to authorize/continue
              setTimeout(() => {
                window.location.href = `/api/v1/oauth/authorize/continue?state=${data.oauth_continue}`;
              }, 500);
            } else {
              // Existing user with terms accepted - go straight to home
              setTimeout(() => {
                window.location.href = '/';
              }, 1500);
            }
          }
        }

      } catch (error) {
        const errorMsg = error.response?.data?.detail || error.message || 'Failed to sign in with Google';
        setStatus('error');
        setMessage(errorMsg);

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
  }, [searchParams, navigate, isPopup]);

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
          <div className="flex justify-center">
            <StatusIcon />
          </div>

          {status === 'error' && (
            <>
              <h2 className="text-xl font-semibold text-foreground mb-2 mt-4">
                Google Sign-In
              </h2>
              <p className="text-error-foreground mb-4">
                {message}
              </p>
              {!isPopup && (
                <div className="space-y-3">
                  <button
                    onClick={() => navigate('/login')}
                    className="w-full px-4 py-2 bg-muted-foreground text-primary-foreground rounded-xl hover:bg-foreground transition-colors"
                  >
                    Return to Login
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

        {showHelpText && !isPopup && status !== 'error' && (
          <div className="mt-6 text-center text-sm text-muted-foreground">
            <p>If this page doesn't automatically redirect, you can close it and return to login.</p>
          </div>
        )}
      </div>
    </div>
  );
}
