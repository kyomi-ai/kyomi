// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState } from 'react';
import { Navigate, Link } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { API_CONFIG } from '../config/api.js';
import { trackEvent } from '../utils/analytics';
import { Button } from '../components/ui/button';
import { Alert, AlertTitle, AlertDescription } from '../components/ui/alert';
import { Spinner } from '../components/ui/spinner';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '../components/ui/select';

export default function BetaSignup() {
  const [email, setEmail] = useState('');
  const [companyName, setCompanyName] = useState('');
  const [companySize, setCompanySize] = useState('');
  const [useCase, setUseCase] = useState('');
  const [preferredSignIn, setPreferredSignIn] = useState('');
  const [marketingConsent, setMarketingConsent] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [success, setSuccess] = useState(false);

  const { isAuthenticated } = useAuth();

  if (isAuthenticated) {
    return <Navigate to="/" replace />;
  }

  const handleSubmit = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError('');

    // Track beta signup attempt
    trackEvent('beta_signup_submitted', {
      company_size: companySize,
      use_case: useCase,
      preferred_signin: preferredSignIn,
      marketing_consent: marketingConsent
    });

    try {
      const response = await fetch(`${API_CONFIG.baseURL}/api/v1/subscribe`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          email,
          company_name: companyName,
          company_size: companySize,
          use_case: useCase,
          preferred_signin: preferredSignIn,
          marketing_consent: marketingConsent,
          source: 'beta_waitlist'
        })
      });

      if (response.ok) {
        setSuccess(true);
        setEmail('');
        setCompanyName('');
        setCompanySize('');
        setUseCase('');
        setPreferredSignIn('');
        setMarketingConsent(false);

        // Track successful signup
        trackEvent('beta_signup_success');
      } else {
        const errorData = await response.json();
        setError(errorData.detail || 'Failed to submit. Please try again.');

        // Track failed signup
        trackEvent('beta_signup_failed', {
          error: errorData.detail
        });
      }
    } catch (err) {
      setError('Failed to submit. Please try again.');
      trackEvent('beta_signup_error', {
        error: err.message
      });
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="min-h-screen bg-background flex force-light">
      {/* Left side - Branding */}
      <div className="hidden lg:flex lg:w-1/2 relative overflow-hidden" style={{backgroundColor: '#0f172a'}}>
        <div className="absolute inset-0" style={{background: 'radial-gradient(ellipse at center, rgba(217, 119, 6, 0.1) 0%, rgba(15, 23, 42, 0.9) 50%, #0f172a 100%), linear-gradient(135deg, #1e293b 0%, #0f172a 100%)'}}></div>
        <div className="absolute inset-0 opacity-30" style={{backgroundImage: 'radial-gradient(circle at 20% 80%, rgba(217, 119, 6, 0.15) 0%, transparent 50%), radial-gradient(circle at 80% 20%, rgba(217, 119, 6, 0.1) 0%, transparent 50%)'}}></div>
        <div className="relative z-10 flex flex-col justify-center items-start px-12 text-white max-w-lg mx-auto">
          <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-32 mb-8" />
          <h1 className="text-4xl font-bold mb-4">The Intelligence Layer for Your Data</h1>
          <p className="text-xl text-gray-300 mb-12">
            Kyomi captures the understanding that lives in your analysts' heads—which tables matter, what metrics mean, how to query your data—and makes it available to your entire team.
          </p>

          <div className="space-y-6">
            <div className="flex items-start space-x-4">
              <div className="flex-shrink-0">
                <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M8 10h.01M12 10h.01M16 10h.01M9 16H5a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v8a2 2 0 01-2 2h-5l-5 5v-5z" />
                </svg>
              </div>
              <div>
                <h3 className="text-lg font-semibold mb-1">Natural Language Queries</h3>
                <p className="text-gray-400">Ask questions in plain English, get instant answers from your data</p>
              </div>
            </div>

            <div className="flex items-start space-x-4">
              <div className="flex-shrink-0">
                <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M4 7v10c0 2.21 3.582 4 8 4s8-1.79 8-4V7M4 7c0 2.21 3.582 4 8 4s8-1.79 8-4M4 7c0-2.21 3.582-4 8-4s8 1.79 8 4" />
                </svg>
              </div>
              <div>
                <h3 className="text-lg font-semibold mb-1">Connect Any Database</h3>
                <p className="text-gray-400">BigQuery, Snowflake, PostgreSQL, ClickHouse, and more</p>
              </div>
            </div>

            <div className="flex items-start space-x-4">
              <div className="flex-shrink-0">
                <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M9 19v-6a2 2 0 00-2-2H5a2 2 0 00-2 2v6a2 2 0 002 2h2a2 2 0 002-2zm0 0V9a2 2 0 012-2h2a2 2 0 012 2v10m-6 0a2 2 0 002 2h2a2 2 0 002-2m0 0V5a2 2 0 012-2h2a2 2 0 012 2v14a2 2 0 01-2 2h-2a2 2 0 01-2-2z" />
                </svg>
              </div>
              <div>
                <h3 className="text-lg font-semibold mb-1">Auto-Generated Dashboards</h3>
                <p className="text-gray-400">Turn conversations into beautiful visualizations automatically</p>
              </div>
            </div>

            <div className="flex items-start space-x-4">
              <div className="flex-shrink-0">
                <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13 10V3L4 14h7v7l9-11h-7z" />
                </svg>
              </div>
              <div>
                <h3 className="text-lg font-semibold mb-1">Lightning Fast</h3>
                <p className="text-gray-400">Powered by AI, optimized for performance on massive datasets</p>
              </div>
            </div>
          </div>
        </div>
      </div>

      {/* Right side - Signup form */}
      <div className="w-full lg:w-1/2 flex items-center justify-center p-8">
        <div className="w-full max-w-md">
          <div className="text-center mb-8">
            <div className="lg:hidden mb-6">
              <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-12 mx-auto dark:hidden" />
              <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-12 mx-auto hidden dark:block" />
            </div>
            <h2 className="text-3xl font-bold text-foreground mb-2">
              Join the Beta Waitlist
            </h2>
            <p className="text-muted-foreground mb-4">
              Get early access to Kyomi when we launch. We'll notify you as soon as we're ready.
            </p>
          </div>

          <div className="space-y-6">
            {success ? (
              <Alert variant="success">
                <AlertTitle>You're on the list! 🎉</AlertTitle>
                <AlertDescription>
                  We'll email you at <strong>{email || 'your address'}</strong> as soon as we launch. In the meantime,
                  check out <a href="https://kyomi.ai" className="underline hover:no-underline font-medium" target="_blank" rel="noopener noreferrer">kyomi.ai</a> to learn more.
                </AlertDescription>
              </Alert>
            ) : (
              <form onSubmit={handleSubmit} className="space-y-5">
                <div>
                  <label htmlFor="email" className="block text-sm font-semibold text-foreground mb-3">
                    Work Email <span className="text-destructive">*</span>
                  </label>
                  <input
                    id="email"
                    name="email"
                    type="email"
                    autoComplete="email"
                    className="w-full px-4 py-3.5 bg-muted border border-input rounded-xl text-foreground placeholder-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent transition-all duration-200 hover:bg-card"
                    value={email}
                    onChange={(e) => setEmail(e.target.value)}
                    placeholder="you@company.com"
                    required
                  />
                </div>

                <div>
                  <label htmlFor="company-name" className="block text-sm font-semibold text-foreground mb-3">
                    Company Name
                  </label>
                  <input
                    id="company-name"
                    name="company"
                    type="text"
                    autoComplete="organization"
                    className="w-full px-4 py-3.5 bg-muted border border-input rounded-xl text-foreground placeholder-muted-foreground focus:outline-none focus:ring-2 focus:ring-ring focus:border-transparent transition-all duration-200 hover:bg-card"
                    value={companyName}
                    onChange={(e) => setCompanyName(e.target.value)}
                    placeholder="Acme Inc."
                  />
                </div>

                <div>
                  <label htmlFor="company-size" className="block text-sm font-semibold text-foreground mb-3">
                    Company Size
                  </label>
                  <Select value={companySize} onValueChange={setCompanySize}>
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Select company size" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="1-5">1-5 employees</SelectItem>
                      <SelectItem value="5-20">5-20 employees</SelectItem>
                      <SelectItem value="20-50">20-50 employees</SelectItem>
                      <SelectItem value="50-200">50-200 employees</SelectItem>
                      <SelectItem value="200+">200+ employees</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div>
                  <label htmlFor="use-case" className="block text-sm font-semibold text-foreground mb-3">
                    Primary Use Case
                  </label>
                  <Select value={useCase} onValueChange={setUseCase}>
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Select primary use case" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="business-intelligence">Business Intelligence</SelectItem>
                      <SelectItem value="data-analysis">Data Analysis</SelectItem>
                      <SelectItem value="reporting">Reporting & Dashboards</SelectItem>
                      <SelectItem value="ad-hoc-queries">Ad-hoc Queries</SelectItem>
                      <SelectItem value="product-analytics">Product Analytics</SelectItem>
                      <SelectItem value="other">Other</SelectItem>
                    </SelectContent>
                  </Select>
                </div>

                <div>
                  <label htmlFor="preferred-signin" className="block text-sm font-semibold text-foreground mb-3">
                    Preferred Sign-in Method <span className="text-destructive">*</span>
                  </label>
                  <Select value={preferredSignIn} onValueChange={setPreferredSignIn} required>
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Select preferred sign-in method" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="google">Google</SelectItem>
                      <SelectItem value="microsoft">Microsoft</SelectItem>
                      <SelectItem value="passkey">Passkey (fingerprint, face, or security key)</SelectItem>
                    </SelectContent>
                  </Select>
                  <p className="text-xs text-muted-foreground mt-2">
                    Passkey sign-in is available now. Google and Microsoft require beta tester approval.
                  </p>
                </div>

                <div className="flex items-start space-x-3">
                  <input
                    id="marketing-consent"
                    name="marketing-consent"
                    type="checkbox"
                    className="mt-1 h-4 w-4 rounded border-input text-primary focus:ring-2 focus:ring-ring focus:ring-offset-0"
                    checked={marketingConsent}
                    onChange={(e) => setMarketingConsent(e.target.checked)}
                  />
                  <label htmlFor="marketing-consent" className="text-sm text-muted-foreground">
                    I agree to receive product updates and launch announcements from Kyomi. You can unsubscribe anytime.
                  </label>
                </div>

                {error && (
                  <Alert variant="error">
                    <AlertDescription>{error}</AlertDescription>
                  </Alert>
                )}

                <Button
                  type="submit"
                  disabled={loading || !email.trim() || !preferredSignIn}
                  className="w-full py-3.5 px-4 bg-primary text-primary-foreground font-semibold rounded-xl focus:outline-none focus:ring-2 focus:ring-ring focus:ring-offset-2 transition-opacity duration-200 transform hover:scale-[0.99] active:scale-[0.97] hover:opacity-90 disabled:bg-muted disabled:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-100"
                >
                  {loading ? (
                    <div className="flex items-center justify-center space-x-2">
                      <Spinner className="text-primary-foreground" />
                      <span>Joining waitlist...</span>
                    </div>
                  ) : 'Request Beta Access'}
                </Button>
              </form>
            )}

            {!success && (
              <div className="text-center">
                <p className="text-sm text-muted-foreground">
                  Already have beta access?{' '}
                  <Link
                    to="/login"
                    className="text-primary font-medium hover:underline"
                    onClick={() => trackEvent('beta_signup_existing_user_clicked')}
                  >
                    Sign in here
                  </Link>
                </p>
              </div>
            )}
          </div>

          {/* Footer with marketing site links */}
          <div className="mt-8 pt-6 border-t border-border space-y-3">
            <div className="flex justify-center items-center space-x-1 text-sm text-muted-foreground">
              <a
                href="https://kyomi.ai/privacy"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-foreground transition-colors"
              >
                Privacy
              </a>
              <span>·</span>
              <a
                href="https://kyomi.ai/terms"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-foreground transition-colors"
              >
                Terms
              </a>
              <span>·</span>
              <a
                href="https://kyomi.ai"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-foreground transition-colors"
              >
                About
              </a>
            </div>
            <p className="text-xs text-muted-foreground text-center">
              All trademarks are property of their respective owners.
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}
