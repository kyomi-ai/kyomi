// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useSearchParams, Link } from 'react-router-dom';
import { API_CONFIG } from '../config/api.js';
import { Button } from '../components/ui/button';
import { Alert, AlertTitle, AlertDescription } from '../components/ui/alert';

export default function Unsubscribe() {
  const [searchParams] = useSearchParams();
  const [email, setEmail] = useState(searchParams.get('email') || '');
  const [loading, setLoading] = useState(false);
  const [success, setSuccess] = useState(false);
  const [error, setError] = useState('');

  const handleUnsubscribe = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError('');

    try {
      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/unsubscribe`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email })
      });

      if (response.ok) {
        setSuccess(true);
      } else {
        const errorData = await response.json();
        setError(errorData.detail || 'Failed to unsubscribe. Please try again.');
      }
    } catch (err) {
      setError('Failed to unsubscribe. Please try again.');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-background flex items-center justify-center p-8">
      <div className="w-full max-w-md">
        <div className="text-center mb-8">
          <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-12 mx-auto mb-6 dark:hidden" />
          <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-12 mx-auto mb-6 hidden dark:block" />
          <h1 className="text-3xl font-bold text-foreground mb-2">
            Unsubscribe from Updates
          </h1>
          <p className="text-muted-foreground">
            We're sorry to see you go
          </p>
        </div>

        <div className="space-y-6">
          {success ? (
            <Alert variant="success">
              <AlertTitle>You've been unsubscribed</AlertTitle>
              <AlertDescription>
                You won't receive any more emails from us about the Kyomi beta launch.
                <div className="mt-4">
                  <Link to="/" className="text-primary hover:underline font-medium">
                    Return to homepage
                  </Link>
                </div>
              </AlertDescription>
            </Alert>
          ) : (
            <form onSubmit={handleUnsubscribe} className="space-y-5">
              <div>
                <label htmlFor="email" className="block text-sm font-semibold text-foreground mb-3">
                  Email Address
                </label>
                <input
                  id="email"
                  name="email"
                  type="email"
                  autoComplete="email"
                  className="w-full px-4 py-3.5 bg-muted border border-border rounded-xl text-foreground placeholder-muted-foreground focus:outline-none focus:ring-2 focus:ring-primary focus:border-transparent transition-all duration-200 hover:bg-background"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder="you@company.com"
                  required
                />
              </div>

              {error && (
                <Alert variant="error">
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}

              <Button
                type="submit"
                disabled={loading || !email.trim()}
                className="w-full"
              >
                {loading ? 'Unsubscribing...' : 'Unsubscribe'}
              </Button>

              <div className="text-center">
                <Link
                  to="/"
                  className="text-sm text-muted-foreground hover:text-foreground transition-colors"
                >
                  Never mind, take me back
                </Link>
              </div>
            </form>
          )}
        </div>
      </div>
    </div>
  );
}
