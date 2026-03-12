// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useSearchParams, Link } from 'react-router-dom';
import apiClient from '../api/apiClient';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from '../components/ui/card';
import { Button } from '../components/ui/button';
import { Alert, AlertDescription } from '../components/ui/alert';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Checkbox } from '../components/ui/checkbox';
import { Spinner } from '../components/ui/spinner';

/**
 * SignupComplete - Handles email verification and password-based account creation
 *
 * Route: /signup/complete?token=xxx
 *
 * Flow:
 * 1. User clicks email link with signup token
 * 2. User enters name, password, confirm password, accepts terms
 * 3. Click "Create Account" -> verifies token, creates account with password, logs in
 * 4. Redirect to /onboarding for datasource setup
 */
export default function SignupComplete() {
  const [searchParams] = useSearchParams();
  const token = searchParams.get('token');

  // States: 'form', 'creating', 'success', 'error'
  const [status, setStatus] = useState('form');
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [termsAccepted, setTermsAccepted] = useState(false);
  const [marketingConsent, setMarketingConsent] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    if (!token) {
      setStatus('error');
      setError('Missing signup token. Please use the link from your email.');
    }
  }, [token]);

  const handleCreateAccount = async (e) => {
    e.preventDefault();
    setError('');

    // Validate
    if (!name.trim()) {
      setError('Please enter your name.');
      return;
    }
    if (!password) {
      setError('Please enter a password.');
      return;
    }
    if (password.length < 8) {
      setError('Password must be at least 8 characters.');
      return;
    }
    if (password !== confirmPassword) {
      setError('Passwords do not match.');
      return;
    }
    if (!termsAccepted) {
      setError('Please accept the Terms of Service and Privacy Policy.');
      return;
    }

    setStatus('creating');

    try {
      const response = await apiClient.post('/api/v1/auth/signup/complete', {
        token,
        name: name.trim(),
        password,
        terms_accepted: termsAccepted,
        marketing_consent: marketingConsent
      });

      setStatus('success');

      // Redirect to onboarding (same pattern as PasskeySignupComplete)
      setTimeout(() => {
        window.location.href = '/onboarding';
      }, 1500);

    } catch (err) {
      setStatus('form'); // Allow retry

      setError(
        err.response?.data?.detail ||
        err.message ||
        'Failed to create account. Please try again.'
      );
    }
  };

  // Render error state (missing/invalid token)
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
              <p className="text-muted-foreground">Creating your account...</p>
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
            <div className="space-y-2">
              <Label htmlFor="name">Full Name</Label>
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

            {/* Password input */}
            <div className="space-y-2">
              <Label htmlFor="password">Password</Label>
              <Input
                id="password"
                type="password"
                autoComplete="new-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                placeholder="At least 8 characters"
                minLength={8}
                required
              />
            </div>

            {/* Confirm password input */}
            <div className="space-y-2">
              <Label htmlFor="confirm-password">Confirm Password</Label>
              <Input
                id="confirm-password"
                type="password"
                autoComplete="new-password"
                value={confirmPassword}
                onChange={(e) => setConfirmPassword(e.target.value)}
                placeholder="Re-enter your password"
                minLength={8}
                required
              />
            </div>

            {/* Terms and consent */}
            <div className="space-y-3">
              <div className="flex items-start space-x-3 cursor-pointer" onClick={() => setTermsAccepted(!termsAccepted)}>
                <Checkbox
                  checked={termsAccepted}
                  onCheckedChange={setTermsAccepted}
                  className="mt-0.5"
                />
                <span className="text-sm text-foreground">
                  I have read and agree to the{' '}
                  <a href="https://kyomi.ai/terms" target="_blank" rel="noopener noreferrer" className="text-primary hover:underline" onClick={(e) => e.stopPropagation()}>
                    Terms of Service
                  </a>
                  {' '}and{' '}
                  <a href="https://kyomi.ai/privacy" target="_blank" rel="noopener noreferrer" className="text-primary hover:underline" onClick={(e) => e.stopPropagation()}>
                    Privacy Policy
                  </a>
                </span>
              </div>

              <div className="flex items-start space-x-3 cursor-pointer" onClick={() => setMarketingConsent(!marketingConsent)}>
                <Checkbox
                  checked={marketingConsent}
                  onCheckedChange={setMarketingConsent}
                  className="mt-0.5"
                />
                <span className="text-sm text-muted-foreground">
                  I agree to receive product updates and announcements from Kyomi. You can unsubscribe anytime.
                </span>
              </div>
            </div>

            {error && (
              <Alert variant="destructive">
                <AlertDescription>{error}</AlertDescription>
              </Alert>
            )}

            <Button type="submit" className="w-full" size="lg">
              Create Account
            </Button>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
