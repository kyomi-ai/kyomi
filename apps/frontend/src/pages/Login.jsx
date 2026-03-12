// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect, useRef } from 'react';
import { useAuth } from '../context/AuthContext';
import { Navigate, useLocation, useNavigate, Link } from 'react-router-dom';
import PasskeyManager from '../utils/passkeys';
import { API_CONFIG } from '../config/api.js';
import { trackEvent } from '../utils/analytics';
import apiClient from '../api/apiClient';
import { Alert, AlertTitle, AlertDescription } from '../components/ui/alert';
import { Button } from '../components/ui/button';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Spinner } from '../components/ui/spinner';

export default function Login() {
  const location = useLocation();
  const navigate = useNavigate();
  const successMessageFromNav = location.state?.message;

  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [totpCode, setTotpCode] = useState('');
  const [loginStep, setLoginStep] = useState('credentials'); // 'credentials' or '2fa'
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState('');
  const [showRegister, setShowRegister] = useState(false);
  const [signupEmail, setSignupEmail] = useState('');
  const [signupName, setSignupName] = useState('');
  const [signupPassword, setSignupPassword] = useState('');
  const [registrationSuccess, setRegistrationSuccess] = useState(successMessageFromNav || '');
  const [signupStep, setSignupStep] = useState('email'); // 'email', 'check_email'
  const [authConfig, setAuthConfig] = useState(null);
  const [passkeySupported, setPasskeySupported] = useState(false);
  const [passkeyLoginLoading, setPasskeyLoginLoading] = useState(false);
  const [googleLoginLoading, setGoogleLoginLoading] = useState(false);
  const [verificationNeeded, setVerificationNeeded] = useState(false);
  const [verificationEmail, setVerificationEmail] = useState('');
  const [resendLoading, setResendLoading] = useState(false);
  const [resendSuccess, setResendSuccess] = useState(false);
  const [signupLoading, setSignupLoading] = useState(false);
  const {
    login,
    logout,
    loginWithPasskey,
    isAuthenticated,
    isChallengePending,
    challenge
  } = useAuth();

  // Use ref to persist error across re-renders
  const persistentError = useRef('');

  // Guard to prevent duplicate OAuth redirects (race condition fix)
  const oauthRedirectStarted = useRef(false);

  useEffect(() => {
    
    // Restore login step from sessionStorage if it exists
    const savedLoginStep = sessionStorage.getItem('login_step');
    if (savedLoginStep) {
      setLoginStep(savedLoginStep);
    }
    
    // Restore error from sessionStorage if it exists (but not 2FA validation errors)
    const savedError = sessionStorage.getItem('login_error');
    if (savedError) {
      // Don't restore 2FA validation errors - they should be fresh on each attempt
      if (!savedError.includes('Invalid 2FA verification code')) {
        setError(savedError);
        persistentError.current = savedError;

        // Check if the saved error indicates 2FA requirement
        if (savedError.includes('2FA verification code is required')) {
          setLoginStep('2fa');
          sessionStorage.setItem('login_step', '2fa');
          // Clear the error for 2FA step - start clean
          setError('');
          persistentError.current = '';
          sessionStorage.removeItem('login_error');
        }
      } else {
        sessionStorage.removeItem('login_error');
      }
    }

    return () => {
      // Cleanup
    };
  }, []);

  useEffect(() => {
    // Check if passkeys are supported
    const checkPasskeySupport = async () => {
      try {
        if (typeof window !== 'undefined' && window.navigator && PasskeyManager) {
          const supported = await PasskeyManager.isAvailable();
          setPasskeySupported(supported);
        }
      } catch (error) {
        setPasskeySupported(false);
      }
    };
    
    // Add a small delay to ensure all modules are loaded
    setTimeout(checkPasskeySupport, 100);
  }, []);

  // Fetch auth configuration from backend (public endpoint, no auth required).
  // Uses raw fetch() instead of apiClient because this runs before authentication —
  // apiClient's token refresh interceptors would fail on an unauthenticated request.
  useEffect(() => {
    fetch('/api/v1/auth/config')
      .then(res => {
        if (!res.ok) {
          console.error(`Auth config fetch failed: ${res.status}`);
          throw new Error(`HTTP ${res.status}`);
        }
        return res.json();
      })
      .then(data => setAuthConfig(data))
      .catch(() => setAuthConfig({ google_oauth: false, passkeys: false, password: true }));
  }, []);

  if (isAuthenticated) {
    // Check for OAuth continuation flow (MCP/third-party OAuth)
    const searchParams = new URLSearchParams(location.search);
    const oauthContinue = searchParams.get('oauth_continue');

    if (oauthContinue && !oauthRedirectStarted.current) {
      // Guard against duplicate redirects (race condition with re-renders)
      oauthRedirectStarted.current = true;
      // Redirect to backend OAuth authorize endpoint to continue the flow
      window.location.href = `${API_CONFIG.baseURL}/api/v1/oauth/authorize/continue?state=${oauthContinue}`;
      return null; // Prevent render during redirect
    } else if (oauthContinue) {
      // Redirect already in progress, just return null to prevent render
      return null;
    }

    // Normal redirect to the page they were trying to access, or home if none
    // Preserve both pathname and search params (e.g., ?state=...)
    const from = location.state?.from;
    const redirectTo = from ? `${from.pathname}${from.search || ''}` : '/';
    return <Navigate to={redirectTo} replace />;
  }

  // Coming Soon mode - disable login for public launch
  const isComingSoon = import.meta.env.VITE_COMING_SOON === 'true';

  const handleSubmit = async (e) => {
    e.preventDefault();
    setLoading(true);
    setError('');
    setVerificationNeeded(false);
    setResendSuccess(false);

    try {
      // Use the new dual-token login method
      const result = await login(email, password, totpCode);

      if (!result.success) {
        // Check for email verification required
        if (result.verificationRequired) {
          setVerificationNeeded(true);
          setVerificationEmail(email);
          return;
        }

        const errorMsg = result.error || 'Login failed. Please try again.';

        // Check if this is a 2FA requirement (initial 2FA challenge)
        if (errorMsg.includes('2FA verification code is required')) {
          setLoginStep('2fa');
          sessionStorage.setItem('login_step', '2fa');

          // Clear error - successful password validation, just need 2FA
          setError('');
          persistentError.current = '';
          sessionStorage.removeItem('login_error');
          return;
        }

        // Check if this is an invalid 2FA code (correction scenario)
        if (errorMsg.includes('Invalid 2FA verification code') && loginStep === '2fa') {
          // Stay on 2FA step, show error, but don't pollute session storage
          setError(errorMsg);
          persistentError.current = errorMsg;
          // Don't persist 2FA validation errors in session storage
          return;
        }

        // Check if we need to reset the login state due to session issues
        if (result.shouldResetLogin) {
          setLoginStep('credentials');
          setTotpCode('');
          setEmail('');
          setPassword('');
          sessionStorage.removeItem('login_step');
          sessionStorage.removeItem('login_error');
        }

        // For other errors (credentials, server errors), persist to survive unmounting
        if (!errorMsg.includes('Invalid 2FA verification code')) {
          sessionStorage.setItem('login_error', errorMsg);
        }

        setError(errorMsg);
        persistentError.current = errorMsg;
      } else {
        // Clear errors and reset to credentials step on success
        sessionStorage.removeItem('login_error');
        sessionStorage.removeItem('login_step');
        setError('');
        persistentError.current = '';
        setLoginStep('credentials');
        setTotpCode(''); // Clear TOTP code on success
      }
      // Success case is handled by the AuthContext (user gets redirected)
    } catch (err) {
      const errorMsg = err.message || 'Login failed. Please try again.';

      // Check if this is a 2FA requirement
      if (errorMsg.includes('2FA verification code is required')) {
        setLoginStep('2fa');
        // Clear error - successful password validation, just need 2FA
        setError('');
        persistentError.current = '';
        sessionStorage.removeItem('login_error');
        return;
      }
      setError(errorMsg);
    } finally {
      setLoading(false);
    }
  };


  const handleGoogleLogin = async () => {
    setGoogleLoginLoading(true);
    setError('');

    // Track "Sign in with Google" button click
    trackEvent('google_signin_clicked');

    try {
      // Pass oauth_continue if present (for MCP OAuth flow continuation)
      const searchParams = new URLSearchParams(location.search);
      const oauthContinue = searchParams.get('oauth_continue');
      const loginUrl = oauthContinue
        ? `${API_CONFIG.baseURL}/api/v1/auth/google/login?oauth_continue=${oauthContinue}`
        : `${API_CONFIG.baseURL}/api/v1/auth/google/login`;

      const response = await fetch(loginUrl, {
        method: 'GET',
      });

      if (response.ok) {
        const data = await response.json();
        if (data.authorization_url) {
          // Redirect to Google OAuth - keep button disabled during redirect
          window.location.href = data.authorization_url;
          // Don't reset loading - page will redirect
        } else {
          setError('Failed to get Google authorization URL');
          setGoogleLoginLoading(false);
        }
      } else {
        const errorData = await response.json();
        setError(errorData.detail || 'Google login failed');
        setGoogleLoginLoading(false);
      }
    } catch {
      setError('Google login failed. Please try again.');
      setGoogleLoginLoading(false);
    }
  };

  const handlePasskeyLogin = async () => {
    setPasskeyLoginLoading(true);
    setError('');
    setVerificationNeeded(false);

    // Track passkey sign-in attempt
    trackEvent('passkey_signin_clicked');

    try {
      // Call PasskeyManager.authenticate() - this triggers the browser's WebAuthn prompt
      const result = await PasskeyManager.authenticate();

      if (result && result.success) {
        // Use AuthContext's loginWithPasskey to complete the login
        const loginResult = await loginWithPasskey(result);
        if (!loginResult.success) {
          setError(loginResult.error || 'Failed to complete passkey login');
        }
        // On success, the AuthContext will redirect via isAuthenticated check
      } else {
        setError(result?.error || 'Passkey authentication failed');
      }
    } catch (err) {

      // Handle email verification required
      if (err.verificationRequired) {
        setVerificationNeeded(true);
        setVerificationEmail(err.email || '');
        return;
      }

      setError(err.message || 'Passkey authentication failed. Please try again.');
    } finally {
      setPasskeyLoginLoading(false);
    }
  };

  // Self-hosted without SMTP: one-step signup (email + name + password in one form)
  const isSelfHostedNoSmtp = authConfig?.self_hosted && !authConfig?.smtp_configured;

  const handleSignup = async (e) => {
    e.preventDefault();

    if (!signupEmail.trim()) {
      setError('Please enter your email address.');
      return;
    }

    if (isSelfHostedNoSmtp) {
      if (!signupName.trim()) {
        setError('Please enter your name.');
        return;
      }
      if (!signupPassword || signupPassword.length < 8) {
        setError('Password must be at least 8 characters.');
        return;
      }
    }

    setSignupLoading(true);
    setError('');

    try {
      const payload = { email: signupEmail };
      if (isSelfHostedNoSmtp) {
        payload.name = signupName;
        payload.password = signupPassword;
      }

      const response = await apiClient.post('/api/v1/auth/signup/start', payload);
      const data = response.data;

      if (data.status === 'account_created') {
        // One-step signup complete — cookies are set by the backend,
        // full page reload to initialize auth state from cookies
        window.location.href = data.redirect || '/';
      } else if (data.status === 'verification_required') {
        setSignupStep('check_email');
        setRegistrationSuccess(data.message || 'Please check your email to complete signup.');
      } else if (data.status === 'pending_signup') {
        navigate(`/signup/complete?token=${data.token}`);
      }
    } catch (err) {
      setError(err.response?.data?.detail || err.message || 'Signup failed. Please try again.');
    } finally {
      setSignupLoading(false);
    }
  };

  const handleResendVerification = async () => {
    setResendLoading(true);
    setResendSuccess(false);
    setError('');

    try {
      await apiClient.post('/api/v1/auth/resend-verification', {
        email: verificationEmail
      });
      setResendSuccess(true);
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to resend verification email. Please try again.');
    } finally {
      setResendLoading(false);
    }
  };

  // Compute which auth sections are visible for conditional divider rendering
  const showPasskeySection = passkeySupported && authConfig?.passkeys !== false;
  const showGoogleSection = authConfig?.google_oauth === true;

  return (
    <div className="min-h-screen bg-background flex force-light">
      {/* Left side - Branding */}
      <div className="hidden lg:flex lg:w-1/2 relative overflow-hidden" style={{backgroundColor: 'var(--color-foreground)'}}>
        <div className="absolute inset-0" style={{background: 'radial-gradient(ellipse at center, color-mix(in srgb, var(--color-primary) 10%, transparent) 0%, color-mix(in srgb, var(--color-foreground) 90%, transparent) 50%, var(--color-foreground) 100%), linear-gradient(135deg, color-mix(in srgb, var(--color-foreground) 80%, white) 0%, var(--color-foreground) 100%)'}}></div>
        <div className="absolute inset-0 opacity-30" style={{backgroundImage: 'radial-gradient(circle at 20% 80%, color-mix(in srgb, var(--color-primary) 15%, transparent) 0%, transparent 50%), radial-gradient(circle at 80% 20%, color-mix(in srgb, var(--color-primary) 10%, transparent) 0%, transparent 50%)'}}></div>
        <div className="relative z-10 flex flex-col justify-center items-center px-12 text-white">
          <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-32 mb-0" />
          <p className="text-2xl font-semibold text-white text-right w-full max-w-xs -mt-6">Data Intelligence Platform</p>
        </div>
      </div>
      
      {/* Right side - Login form */}
      <div className="w-full lg:w-1/2 flex items-center justify-center p-8">
        <div className="w-full max-w-md">
          <div className="text-center mb-8">
            <div className="lg:hidden mb-6">
              <img src="/kyomi_full_logo.svg" alt="Kyomi" className="h-12 mx-auto dark:hidden" />
              <img src="/kyomi_full_logo_white.svg" alt="Kyomi" className="h-12 mx-auto hidden dark:block" />
            </div>
            <h2 className="text-3xl font-semibold text-foreground mb-2">
              {showRegister ? 'Create your account' : 'Welcome back'}
            </h2>
            <p className="text-muted-foreground mb-4">
              {showRegister ? 'Get started with Kyomi' : 'Sign in to your account to continue'}
            </p>
          </div>

          <div className="space-y-6">
            {!showRegister ? (
              <>
                {/* Coming Soon Banner */}
                {isComingSoon && loginStep === 'credentials' && (
                  <div className="bg-gradient-to-br from-primary/10 to-primary/5 border-2 border-primary/20 rounded-2xl p-8 text-center space-y-4">
                    <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mb-2">
                      <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M12 8v4l3 3m6-3a9 9 0 11-18 0 9 9 0 0118 0z" />
                      </svg>
                    </div>
                    <div>
                      <h3 className="text-2xl font-bold text-foreground mb-2">Coming Soon</h3>
                      <p className="text-foreground font-medium mb-1">
                        Kyomi is launching soon!
                      </p>
                      <p className="text-sm text-muted-foreground max-w-sm mx-auto">
                        We're putting the final touches on our AI-powered analytics platform. Sign-in will be available shortly.
                      </p>
                    </div>
                    <div className="pt-4">
                      <a
                        href="https://kyomi.ai"
                        className="inline-flex items-center justify-center px-6 py-3 bg-primary text-white font-semibold rounded-xl hover:opacity-90 transition-opacity"
                      >
                        Learn More
                      </a>
                    </div>
                  </div>
                )}

                {/* Main Sign In Options */}
                {!isComingSoon && loginStep === 'credentials' && (
                  <>
                    {/* Passkey Sign In - Open to Everyone */}
                    {showPasskeySection && (
                      <div className="space-y-3">
                        <button
                          type="button"
                          onClick={handlePasskeyLogin}
                          disabled={passkeyLoginLoading || googleLoginLoading}
                          className="w-full py-3.5 px-4 bg-primary text-white font-semibold rounded-xl shadow-lg hover:shadow-xl focus:outline-none focus:ring-2 focus:ring-primary focus:ring-offset-2 transition-all duration-200 hover:opacity-90 disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                          {passkeyLoginLoading ? (
                            <div className="flex items-center justify-center space-x-2">
                              <Spinner className="text-white" />
                              <span>Authenticating...</span>
                            </div>
                          ) : (
                            <div className="flex items-center justify-center space-x-2">
                              <span className="text-lg">🔑</span>
                              <span>Sign in with Passkey</span>
                            </div>
                          )}
                        </button>
                      </div>
                    )}

                    {/* Divider between passkey and Google */}
                    {showPasskeySection && showGoogleSection && (
                      <div className="relative my-6">
                        <div className="absolute inset-0 flex items-center">
                          <div className="w-full border-t border-border"></div>
                        </div>
                        <div className="relative flex justify-center text-sm">
                          <span className="px-4 bg-background text-muted-foreground">or</span>
                        </div>
                      </div>
                    )}

                    {/* Google Sign In */}
                    {showGoogleSection && (
                      <div className="space-y-3">
                        <button
                          type="button"
                          onClick={handleGoogleLogin}
                          disabled={googleLoginLoading}
                          className="w-full flex justify-center items-center px-4 py-3.5 border border-input rounded-xl shadow-sm bg-card text-foreground font-medium hover:bg-accent focus:outline-none focus:ring-2 focus:ring-offset-2 focus:ring-ring transition-all duration-200 disabled:cursor-not-allowed disabled:opacity-50"
                        >
                          {googleLoginLoading ? (
                            <div className="flex items-center justify-center space-x-2">
                              <Spinner className="text-muted-foreground" />
                              <span>Connecting to Google...</span>
                            </div>
                          ) : (
                            <>
                              <svg className="w-5 h-5 mr-2" viewBox="0 0 24 24">
                                <path fill="#4285F4" d="M22.56 12.25c0-.78-.07-1.53-.2-2.25H12v4.26h5.92c-.26 1.37-1.04 2.53-2.21 3.31v2.77h3.57c2.08-1.92 3.28-4.74 3.28-8.09z"/>
                                <path fill="#34A853" d="M12 23c2.97 0 5.46-.98 7.28-2.66l-3.57-2.77c-.98.66-2.23 1.06-3.71 1.06-2.86 0-5.29-1.93-6.16-4.53H2.18v2.84C3.99 20.53 7.7 23 12 23z"/>
                                <path fill="#FBBC05" d="M5.84 14.09c-.22-.66-.35-1.36-.35-2.09s.13-1.43.35-2.09V7.07H2.18C1.43 8.55 1 10.22 1 12s.43 3.45 1.18 4.93l2.85-2.22.81-.62z"/>
                                <path fill="#EA4335" d="M12 5.38c1.62 0 3.06.56 4.21 1.64l3.15-3.15C17.45 2.09 14.97 1 12 1 7.7 1 3.99 3.47 2.18 7.07l3.66 2.84c.87-2.6 3.3-4.53 6.16-4.53z"/>
                              </svg>
                              Sign in with Google
                            </>
                          )}
                        </button>
                      </div>
                    )}

                    {/* Divider before email form - show if any auth option above is visible */}
                    {(showPasskeySection || showGoogleSection) && (
                      <div className="relative my-6">
                        <div className="absolute inset-0 flex items-center">
                          <div className="w-full border-t border-border"></div>
                        </div>
                        <div className="relative flex justify-center text-sm">
                          <span className="px-4 bg-background text-muted-foreground">or sign in with email</span>
                        </div>
                      </div>
                    )}

                    {/* Email + Password Login */}
                    <form onSubmit={handleSubmit} className="space-y-4">
                      <div className="space-y-2">
                        <Label htmlFor="login-email">Email</Label>
                        <Input
                          id="login-email"
                          name="email"
                          type="email"
                          autoComplete="email"
                          value={email}
                          onChange={(e) => setEmail(e.target.value)}
                          placeholder="name@company.com"
                          className="h-11"
                          required
                        />
                      </div>
                      <div className="space-y-2">
                        <Label htmlFor="login-password">Password</Label>
                        <Input
                          id="login-password"
                          name="password"
                          type="password"
                          autoComplete="current-password"
                          value={password}
                          onChange={(e) => setPassword(e.target.value)}
                          placeholder="Enter your password"
                          className="h-11"
                          required
                        />
                      </div>
                      <Button
                        type="submit"
                        disabled={loading || !email.trim() || !password}
                        className="w-full"
                        size="lg"
                      >
                        {loading ? (
                          <div className="flex items-center justify-center space-x-2">
                            <Spinner className="text-white" />
                            <span>Signing in...</span>
                          </div>
                        ) : 'Sign In'}
                      </Button>
                      <p className="text-xs text-muted-foreground text-center mt-3">
                        New to Kyomi?{' '}
                        <button
                          type="button"
                          onClick={() => {
                            setShowRegister(true);
                            setSignupStep('email');
                            setError('');
                          }}
                          className="text-primary hover:underline"
                        >
                          Create an account
                        </button>
                        {' · '}
                        <Link
                          to="/account/recover"
                          className="text-primary hover:underline"
                        >
                          Can't sign in?
                        </Link>
                      </p>
                    </form>

                    {/* Error and Success Messages */}
                    {registrationSuccess && (
                      <Alert variant="success">
                        <AlertDescription>{registrationSuccess}</AlertDescription>
                      </Alert>
                    )}

                    {error && !verificationNeeded && (
                      <Alert variant="error">
                        <AlertDescription>{error}</AlertDescription>
                      </Alert>
                    )}

                    {verificationNeeded && (
                      <Alert variant="warning">
                        <AlertTitle>Email Verification Required</AlertTitle>
                        <AlertDescription>
                          <p className="mb-3">
                            Please verify your email before signing in.
                            Check your inbox for the verification link.
                          </p>
                          <Button
                            variant="outline"
                            size="sm"
                            onClick={handleResendVerification}
                            disabled={resendLoading}
                          >
                            {resendLoading ? 'Sending...' : 'Resend Verification Email'}
                          </Button>
                          {resendSuccess && (
                            <p className="text-sm text-success-foreground mt-2">
                              Verification email sent! Check your inbox.
                            </p>
                          )}
                          {error && (
                            <p className="text-sm text-error-foreground mt-2">
                              {error}
                            </p>
                          )}
                        </AlertDescription>
                      </Alert>
                    )}
                  </>
                )}

                {/* Step 2: 2FA Verification */}
                {(isChallengePending || loginStep === '2fa') && (
                  <div className="text-center space-y-6">
                    <div>
                      <h3 className="text-lg font-semibold text-foreground mb-2">Two-Factor Authentication</h3>
                      <p className="text-sm text-muted-foreground">Enter the 6-digit code from your authenticator app to complete sign in</p>
                      <p className="text-xs text-muted-foreground mt-1">Signing in as: <span className="font-medium">{challenge?.user_info?.email || email}</span></p>
                    </div>
                    
                    {/* Error message for 2FA step */}
                    {error && (
                      <Alert variant="error">
                        <AlertDescription>{error}</AlertDescription>
                      </Alert>
                    )}

                    <form onSubmit={handleSubmit} className="space-y-5">
                      <div className="space-y-2">
                        <Label htmlFor="totp-code">Verification Code</Label>
                        <Input
                          id="totp-code"
                          name="totp-code"
                          type="text"
                          autoComplete="one-time-code"
                          value={totpCode}
                          onChange={(e) => setTotpCode(e.target.value.replace(/\D/g, ''))}
                          placeholder="000000"
                          maxLength="6"
                          autoFocus
                          required
                          className="h-12 text-center text-2xl tracking-widest font-mono"
                        />
                      </div>
                      <Button
                        type="submit"
                        disabled={loading || totpCode.length !== 6}
                        className="w-full"
                        size="lg"
                      >
                        {loading ? (
                          <div className="flex items-center justify-center space-x-2">
                            <Spinner className="text-white" />
                            <span>Verifying...</span>
                          </div>
                        ) : 'Verify & Sign In'}
                      </Button>
                    </form>

                    <Button
                      variant="link"
                      onClick={() => {
                        setLoginStep('credentials');
                        setError('');
                        setTotpCode('');
                        sessionStorage.removeItem('login_error');
                        sessionStorage.removeItem('login_step');
                        // CRITICAL: Clear the AuthContext challenge state to allow user to return to login
                        logout(); // This clears challenge state and resets auth to unauthenticated
                      }}
                      className="text-muted-foreground"
                    >
                      Back to login
                    </Button>
                  </div>
                )}
              </>
            ) : (
              <div className="space-y-5">
                {/* Step 1: Email Input */}
                {signupStep === 'email' && (
                  <form onSubmit={handleSignup} className="space-y-5">
                    <div className="space-y-2">
                      <Label htmlFor="signup-email">Email address</Label>
                      <Input
                        id="signup-email"
                        name="email"
                        type="email"
                        autoComplete="email"
                        value={signupEmail}
                        onChange={(e) => setSignupEmail(e.target.value)}
                        placeholder="name@company.com"
                        className="h-11"
                        required
                      />
                    </div>

                    {isSelfHostedNoSmtp && (
                      <>
                        <div className="space-y-2">
                          <Label htmlFor="signup-name">Name</Label>
                          <Input
                            id="signup-name"
                            name="name"
                            type="text"
                            autoComplete="name"
                            value={signupName}
                            onChange={(e) => setSignupName(e.target.value)}
                            placeholder="Your name"
                            className="h-11"
                            required
                          />
                        </div>
                        <div className="space-y-2">
                          <Label htmlFor="signup-password">Password</Label>
                          <Input
                            id="signup-password"
                            name="password"
                            type="password"
                            autoComplete="new-password"
                            value={signupPassword}
                            onChange={(e) => setSignupPassword(e.target.value)}
                            placeholder="At least 8 characters"
                            className="h-11"
                            required
                            minLength={8}
                          />
                        </div>
                      </>
                    )}

                    {error && (
                      <Alert variant="error">
                        <AlertDescription>{error}</AlertDescription>
                      </Alert>
                    )}

                    <Button
                      type="submit"
                      disabled={signupLoading || !signupEmail.trim() || (isSelfHostedNoSmtp && (!signupName.trim() || signupPassword.length < 8))}
                      className="w-full"
                      size="lg"
                    >
                      {signupLoading ? (
                        <div className="flex items-center justify-center space-x-2">
                          <Spinner className="text-white" />
                          <span>{isSelfHostedNoSmtp ? 'Creating account...' : 'Sending verification...'}</span>
                        </div>
                      ) : (
                        isSelfHostedNoSmtp ? 'Create Account' : 'Sign up with Email'
                      )}
                    </Button>

                    {!isSelfHostedNoSmtp && (
                      <p className="text-xs text-muted-foreground text-center">
                        We'll send you an email to verify your address, then you'll set up your password.
                      </p>
                    )}

                    <p className="text-xs text-muted-foreground text-center mt-4">
                      Already have an account?{' '}
                      <button
                        type="button"
                        onClick={() => {
                          setShowRegister(false);
                          setError('');
                          setSignupEmail('');
                        }}
                        className="text-primary hover:underline"
                      >
                        Sign in
                      </button>
                    </p>
                  </form>
                )}

                {/* Step 2: Check Email */}
                {signupStep === 'check_email' && (
                  <div className="text-center space-y-4">
                    <div className="inline-flex items-center justify-center w-16 h-16 rounded-full bg-primary/10 mb-2">
                      <svg className="w-8 h-8 text-primary" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M3 8l7.89 5.26a2 2 0 002.22 0L21 8M5 19h14a2 2 0 002-2V7a2 2 0 00-2-2H5a2 2 0 00-2 2v10a2 2 0 002 2z" />
                      </svg>
                    </div>
                    <h3 className="text-xl font-semibold text-foreground">Check Your Email</h3>
                    <p className="text-muted-foreground">
                      We sent a verification link to <strong>{signupEmail}</strong>
                    </p>
                    <p className="text-muted-foreground">
                      Click the link in the email to complete your signup and set up your account.
                    </p>
                    <p className="text-sm text-muted-foreground">
                      The link expires in 1 hour.
                    </p>
                    <div className="pt-4">
                      <Button
                        variant="link"
                        onClick={() => {
                          setSignupStep('email');
                          setError('');
                          setRegistrationSuccess('');
                          setSignupEmail('');
                          setShowRegister(false);
                        }}
                        className="text-muted-foreground"
                      >
                        Back to login
                      </Button>
                    </div>
                  </div>
                )}
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
              <span>·</span>
              <a
                href="https://status.kyomi.ai"
                target="_blank"
                rel="noopener noreferrer"
                className="hover:text-foreground transition-colors"
              >
                Status
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