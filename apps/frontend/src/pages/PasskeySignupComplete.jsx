// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useSearchParams, Link } from 'react-router-dom';
import { startRegistration } from '@simplewebauthn/browser';
import apiClient from '../api/apiClient';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Alert, AlertDescription } from '../components/ui/alert';
import { Input } from '../components/ui/input';
import { Spinner } from '../components/ui/spinner';

/**
 * PasskeySignupComplete - Handles email verification and account creation
 *
 * Route: /auth/passkey-signup?token=xxx
 *
 * Unified flow (single page, single button):
 * 1. User clicks email link with signup token
 * 2. User enters name, accepts terms (single form)
 * 3. Click "Create Account" → verifies token, creates passkey, logs in
 * 4. Redirect to /onboarding for datasource setup
 */
export default function PasskeySignupComplete() {
  const [searchParams] = useSearchParams();
  const token = searchParams.get('token');

  // States: 'form', 'creating', 'success', 'error'
  const [status, setStatus] = useState('form');
  const [name, setName] = useState('');
  const [termsAccepted, setTermsAccepted] = useState(false);
  const [marketingConsent, setMarketingConsent] = useState(false);
  const [error, setError] = useState('');
  const [statusMessage, setStatusMessage] = useState('');

  useEffect(() => {
    if (!token) {
      setStatus('error');
      setError('Missing signup token. Please use the link from your email.');
    }
  }, [token]);

  // Get a user-friendly device name
  const getDeviceName = () => {
    const ua = navigator.userAgent;
    const platform = navigator.platform || 'Unknown';

    const getBrowser = () => {
      if (/Chrome/.test(ua) && !/Edge/.test(ua)) return 'Chrome';
      if (/Firefox/.test(ua)) return 'Firefox';
      if (/Safari/.test(ua) && !/Chrome/.test(ua)) return 'Safari';
      if (/Edge/.test(ua)) return 'Edge';
      return 'Browser';
    };

    if (/iPhone|iPad|iPod/.test(ua)) return `iPhone/iPad (${getBrowser()})`;
    if (/Android/.test(ua)) return `Android (${getBrowser()})`;
    if (/Mac/.test(platform)) return `Mac (${getBrowser()})`;
    if (/Win/.test(platform)) return `Windows (${getBrowser()})`;
    if (/Linux/.test(platform)) return `Linux (${getBrowser()})`;

    return `${platform} (${getBrowser()})`;
  };

  const handleCreateAccount = async (e) => {
    e.preventDefault();
    setError('');

    // Validate
    if (!name.trim()) {
      setError('Please enter your name.');
      return;
    }
    if (!termsAccepted) {
      setError('Please accept the Terms of Service and Privacy Policy.');
      return;
    }

    setStatus('creating');
    setStatusMessage('Verifying your email...');

    try {
      // Step 1: Verify token, update name, set terms, get WebAuthn challenge
      const response = await apiClient.post('/api/v1/auth/passkeys/signup/complete', {
        token,
        name: name.trim(),
        terms_accepted: true,
        marketing_consent: marketingConsent
      });

      const data = response.data;

      if (data.status !== 'ready_for_passkey') {
        setStatus('error');
        setError('Unexpected response from server. Please try again.');
        return;
      }

      // Step 2: Create passkey via WebAuthn
      setStatusMessage('Creating your passkey...');

      const registrationResponse = await startRegistration({
        optionsJSON: data.options.publicKey || data.options
      });

      // Step 3: Complete registration on server
      setStatusMessage('Finalizing your account...');

      const completeResponse = await apiClient.post('/api/v1/auth/passkeys/register/complete', {
        challenge_id: data.challenge_id,
        credential: registrationResponse,
        device_name: getDeviceName()
      });

      setStatus('success');

      // Step 4: All new users go to datasource onboarding (datasource-agnostic)
      setTimeout(() => {
        window.location.href = '/onboarding';
      }, 1500);

    } catch (err) {
      setStatus('form'); // Allow retry

      // Handle specific WebAuthn errors
      if (err.name === 'InvalidStateError') {
        setError('A passkey already exists for this device. Please try with a different device.');
      } else if (err.name === 'NotAllowedError') {
        setError('Passkey creation was cancelled or timed out. Please try again.');
      } else if (err.name === 'AbortError') {
        setError('Passkey creation was cancelled. Please try again.');
      } else if (err.name === 'NotSupportedError') {
        setError('Your device does not support passkeys. Please try a different authentication method.');
      } else {
        setError(
          err.response?.data?.detail ||
          err.message ||
          'Failed to create account. Please try again.'
        );
      }
    }
  };

  // Render error state
  if (status === 'error') {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-error/10 mx-auto mb-4">
              <svg className="w-8 h-8 text-error-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 9v2m0 4h.01m-6.938 4h13.856c1.54 0 2.502-1.667 1.732-3L13.732 4c-.77-1.333-2.694-1.333-3.464 0L3.34 16c-.77 1.333.192 3 1.732 3z" />
              </svg>
            </div>
            <CardTitle className="text-xl">Signup Link Invalid</CardTitle>
            <CardDescription className="text-error-foreground">
              {error}
            </CardDescription>
          </CardHeader>
          <CardContent className="space-y-4">
            <Link to="/login">
              <Button variant="outline" className="w-full">
                Back to Login
              </Button>
            </Link>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render success state
  if (status === 'success') {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardHeader className="text-center">
            <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-success/10 mx-auto mb-4">
              <svg className="w-8 h-8 text-success-foreground" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
              </svg>
            </div>
            <CardTitle className="text-xl">Account Created!</CardTitle>
            <CardDescription>
              Welcome to Kyomi! Setting up your workspace...
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Spinner size="lg" className="text-primary mx-auto" />
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render creating state
  if (status === 'creating') {
    return (
      <div className="min-h-screen bg-background flex items-center justify-center p-4">
        <Card className="w-full max-w-md">
          <CardContent className="pt-6">
            <div className="text-center space-y-4">
              <Spinner size="xl" className="text-primary mx-auto" />
              <p className="text-muted-foreground">{statusMessage}</p>
            </div>
          </CardContent>
        </Card>
      </div>
    );
  }

  // Render form state (default)
  return (
    <div className="min-h-screen bg-background flex items-center justify-center p-4">
      <Card className="w-full max-w-md">
        <CardHeader className="text-center">
          <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mx-auto mb-4">
            <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M5 13l4 4L19 7" />
            </svg>
          </div>
          <CardTitle className="text-xl">Email Verified!</CardTitle>
          <CardDescription>
            Complete your account setup below.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <form onSubmit={handleCreateAccount} className="space-y-6">
            {/* Name input */}
            <div>
              <label htmlFor="name" className="block text-sm font-medium text-foreground mb-2">
                Full Name
              </label>
              <Input
                id="name"
                type="text"
                autoComplete="name"
                autoFocus
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="John Doe"
                required
              />
            </div>

            {/* Terms and consent */}
            <div className="space-y-3">
              <label className="flex items-start space-x-3 cursor-pointer">
                <input
                  type="checkbox"
                  checked={termsAccepted}
                  onChange={(e) => setTermsAccepted(e.target.checked)}
                  className="mt-1"
                />
                <span className="text-sm text-foreground">
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
                <span className="text-sm text-muted-foreground">
                  I agree to receive product updates and announcements from Kyomi. You can unsubscribe anytime.
                </span>
              </label>
            </div>

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <Button type="submit" className="w-full" size="lg">
              Create Account
            </Button>

            <p className="text-xs text-center text-muted-foreground">
              You will be prompted to create a passkey using your fingerprint, face, or security key.
            </p>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
