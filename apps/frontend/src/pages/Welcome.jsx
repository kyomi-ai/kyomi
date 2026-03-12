// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import apiClient from '../api/apiClient';
import { useAuth } from '../context/AuthContext';
import { Button } from '../components/ui/button';
import { Card } from '../components/ui/card';
import { Alert } from '../components/ui/alert';
import { Spinner } from '../components/ui/spinner';

export function Welcome() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const { refreshUser } = useAuth();
  const [agreed, setAgreed] = useState(false);
  const [marketingConsent, setMarketingConsent] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState(null);

  const tempToken = searchParams.get('temp_token');
  const isExistingUser = searchParams.get('existing_user') === 'true';

  useEffect(() => {
    if (!tempToken) {
      // No temp token - redirect to login
      navigate('/login');
    }
  }, [tempToken, navigate]);

  async function handleAccept() {
    if (!agreed) return;

    setLoading(true);
    setError(null);

    try {
      const response = await apiClient.post('/api/v1/auth/accept-terms', {
        temp_token: tempToken,
        accepted: true,
        marketing_consent: marketingConsent
      });

      if (response.data.success) {
        // Terms accepted! Backend has already set auth cookies
        // All new users go to datasource onboarding (datasource-agnostic)
        window.location.href = '/onboarding';
      }
    } catch (err) {
      setError(
        err.response?.data?.detail ||
        'Failed to accept terms. Please try again.'
      );
      setLoading(false);
    }
  }

  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-4">
      <Card className="max-w-2xl w-full p-8">
        <div className="text-center mb-6">
          <h1 className="text-3xl font-bold mb-2">
            {isExistingUser ? 'Welcome Back!' : 'Welcome to Kyomi!'}
          </h1>
          <p className="text-muted-foreground">
            {isExistingUser
              ? 'Please review and accept our updated terms to continue.'
              : 'Before you continue, please review and accept our terms.'}
          </p>
        </div>

        {error && (
          <Alert variant="error" className="mb-6">
            {error}
          </Alert>
        )}

        <div className="mb-6 space-y-4">
          <label className="flex items-start space-x-3 cursor-pointer">
            <input
              type="checkbox"
              checked={agreed}
              onChange={(e) => setAgreed(e.target.checked)}
              className="mt-1"
            />
            <span className="text-sm">
              I have read and agree to the{' '}
              <a href="https://kyomi.ai/terms" target="_blank" rel="noopener noreferrer" className="text-primary hover:underline">
                Terms of Service
              </a>
              {' '}and{' '}
              <a href="https://kyomi.ai/privacy" target="_blank" rel="noopener noreferrer" className="text-primary hover:underline">
                Privacy Policy
              </a>
            </span>
          </label>

          <label className="flex items-start space-x-3 cursor-pointer">
            <input
              type="checkbox"
              checked={marketingConsent}
              onChange={(e) => setMarketingConsent(e.target.checked)}
              className="mt-1"
            />
            <span className="text-sm">
              I agree to receive product updates and announcements from Kyomi. You can unsubscribe anytime.
            </span>
          </label>
        </div>

        <Button
          onClick={handleAccept}
          disabled={!agreed || loading}
          className="w-full"
          size="lg"
        >
          {loading ? (
            <span className="flex items-center justify-center gap-2">
              <Spinner size="md" />
              Please wait...
            </span>
          ) : (
            'Continue to Kyomi'
          )}
        </Button>
      </Card>
    </div>
  );
}
