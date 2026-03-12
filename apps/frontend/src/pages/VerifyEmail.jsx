// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Mail, CheckCircle, AlertCircle, ArrowRight } from 'lucide-react';
import { Spinner } from '../components/ui/spinner';
import { API_CONFIG } from '../config/api.js';

export default function VerifyEmail() {
  const [searchParams] = useSearchParams();
  const navigate = useNavigate();
  const [status, setStatus] = useState('verifying'); // verifying, success, error
  const [message, setMessage] = useState('');
  const token = searchParams.get('token');

  useEffect(() => {
    if (!token) {
      setStatus('error');
      setMessage('No verification token provided');
      return;
    }

    verifyEmail(token);
  }, [token]);

  const verifyEmail = async (verificationToken) => {
    try {
      const response = await fetch(`${API_CONFIG.baseURL}${API_CONFIG.endpoints.auth.verify}?token=${verificationToken}`, {
        method: 'GET'
      });

      const data = await response.json();

      if (response.ok) {
        setStatus('success');
        setMessage(data.message || 'Email verified successfully!');
        
        // Redirect to login after 3 seconds
        setTimeout(() => {
          navigate('/login', { replace: true });
        }, 3000);
      } else {
        setStatus('error');
        setMessage(data.detail || 'Email verification failed');
      }
    } catch (error) {
      setStatus('error');
      setMessage('Network error occurred during verification');
    }
  };

  const getStatusIcon = () => {
    switch (status) {
      case 'verifying':
        return (
          <Spinner size="xl" className="text-primary mx-auto" />
        );
      case 'success':
        return <CheckCircle className="h-16 w-16 text-success-foreground mx-auto" />;
      case 'error':
        return <AlertCircle className="h-16 w-16 text-error-foreground mx-auto" />;
    }
  };

  const getStatusColor = () => {
    switch (status) {
      case 'verifying':
        return 'text-primary';
      case 'success':
        return 'text-success-foreground';
      case 'error':
        return 'text-error-foreground';
    }
  };

  const getBackgroundColor = () => {
    switch (status) {
      case 'verifying':
        return 'bg-info border-info-border';
      case 'success':
        return 'bg-success border-success-border';
      case 'error':
        return 'bg-error border-error-border';
    }
  };

  return (
    <div className="min-h-screen bg-gradient-to-br from-muted via-blue-50 to-purple-50 flex items-center justify-center p-4">
      <div className="w-full max-w-md">
        <div className="bg-card/80 backdrop-blur-sm rounded-2xl shadow-xl border border-border overflow-hidden">
          {/* Header */}
          <div className="p-8 text-center">
            <div className="w-20 h-20 bg-info rounded-2xl flex items-center justify-center mx-auto mb-6">
              <Mail className="text-primary" size={32} />
            </div>
            <h1 className="text-2xl font-bold text-foreground mb-2">Email Verification</h1>
            <p className="text-muted-foreground">
              {status === 'verifying' && 'Verifying your email address...'}
              {status === 'success' && 'Your email has been verified!'}
              {status === 'error' && 'Verification failed'}
            </p>
          </div>

          {/* Status Section */}
          <div className="px-8 pb-8">
            <div className={`border rounded-2xl p-6 ${getBackgroundColor()}`}>
              <div className="text-center">
                {getStatusIcon()}
                <div className={`mt-4 font-semibold ${getStatusColor()}`}>
                  {status === 'verifying' && 'Please wait...'}
                  {status === 'success' && 'Success'}
                  {status === 'error' && 'Error'}
                </div>
                <p className="text-sm text-muted-foreground mt-2">
                  {message}
                </p>
              </div>

              {status === 'success' && (
                <div className="mt-6 text-center">
                  <div className="bg-card/60 rounded-xl p-4 border border-success-border">
                    <p className="text-sm text-muted-foreground mb-3">
                      You can now sign in with your credentials.
                    </p>
                    <p className="text-xs text-muted-foreground">
                      Redirecting to login in 3 seconds...
                    </p>
                  </div>
                  <button
                    onClick={() => navigate('/login', { replace: true })}
                    className="mt-4 inline-flex items-center px-4 py-2 bg-success text-white font-semibold rounded-xl hover:bg-success/90 transition-colors duration-200"
                  >
                    Go to Login
                    <ArrowRight size={16} className="ml-2" />
                  </button>
                </div>
              )}

              {status === 'error' && (
                <div className="mt-6 text-center">
                  <div className="bg-card/60 rounded-xl p-4 border border-error-border">
                    <p className="text-sm text-muted-foreground mb-3">
                      The verification link may be expired or invalid.
                    </p>
                  </div>
                  <button
                    onClick={() => navigate('/login', { replace: true })}
                    className="mt-4 inline-flex items-center px-4 py-2 bg-muted-foreground text-primary-foreground font-semibold rounded-xl hover:bg-foreground transition-colors duration-200"
                  >
                    Back to Login
                    <ArrowRight size={16} className="ml-2" />
                  </button>
                </div>
              )}
            </div>
          </div>
        </div>

        {/* Footer */}
        <div className="text-center mt-8">
          <p className="text-sm text-muted-foreground">
            Need help? Contact{' '}
            <a href="mailto:support@kyomi.dev" className="text-primary hover:text-primary/80">
              support@kyomi.dev
            </a>
          </p>
        </div>
      </div>
    </div>
  );
}