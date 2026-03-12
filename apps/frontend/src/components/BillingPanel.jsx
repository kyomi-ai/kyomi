// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useSearchParams } from 'react-router-dom';
import { useAuth } from '../context/AuthContext';
import { Button } from './ui/button';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from './ui/card';
import { Alert } from './ui/alert';
import { StatusBadge } from './ui/status-badge';
import { CreditCard, Check, FileText, ExternalLink, Users, Plus, Minus } from 'lucide-react';
import { Spinner } from './ui/spinner';
import ConfirmDialog from './ConfirmDialog';
import useConfirm from '../hooks/useConfirm';
import Modal from './Modal';

/**
 * BillingPanel - Complete billing UI for Kyomi subscriptions
 *
 * Features:
 * - Current plan display with usage stats
 * - Upgrade/downgrade options
 * - AI usage progress bar
 * - Stripe checkout integration
 * - Subscription management (cancel, etc.)
 */
export default function BillingPanel() {
  const { apiClient } = useAuth();
  const { isOpen, dialogProps, confirm } = useConfirm();
  const [searchParams, setSearchParams] = useSearchParams();
  const [loading, setLoading] = useState(true);
  const [subscriptionInfo, setSubscriptionInfo] = useState(null);
  const [checkoutLoading, setCheckoutLoading] = useState(false);
  const [error, setError] = useState(null);
  const [success, setSuccess] = useState(null);
  const [invoices, setInvoices] = useState([]);
  const [invoicesLoading, setInvoicesLoading] = useState(false);
  const [showPlansModal, setShowPlansModal] = useState(false);
  const [teamSizeLoading, setTeamSizeLoading] = useState(false);
  const [desiredTeamSize, setDesiredTeamSize] = useState(5);

  // Load subscription info and invoices on mount
  useEffect(() => {
    loadSubscriptionInfo();

    // Load invoices after a short delay to avoid blocking other requests
    setTimeout(() => {
      loadInvoices();
    }, 100);
  }, []);

  // Sync desired team size with current user limit
  useEffect(() => {
    if (subscriptionInfo?.user_limit) {
      setDesiredTeamSize(subscriptionInfo.user_limit);
    }
  }, [subscriptionInfo]);

  // Handle successful checkout - poll for updated subscription
  useEffect(() => {
    const successParam = searchParams.get('success');
    if (successParam === 'true') {
      setSuccess('Payment successful! Your subscription is being activated...');

      // Poll for updated subscription (webhook might take a few seconds)
      let pollCount = 0;
      const maxPolls = 10; // Poll for up to 10 seconds

      const pollInterval = setInterval(async () => {
        pollCount++;
        const response = await apiClient.get('/api/v1/billing/subscription-info');

        // Check if subscription tier has changed from 'free'
        if (response.data.tier !== 'free') {
          clearInterval(pollInterval);
          setSuccess('Subscription activated! Reloading...');

          // Force full page reload to get fresh JWT with updated subscription
          setTimeout(() => {
            window.location.href = '/settings/billing';
          }, 1000);
          return;
        }

        if (pollCount >= maxPolls) {
          clearInterval(pollInterval);
          setSuccess('Subscription is processing. Please refresh the page in a moment.');
        }
      }, 1000);

      // Clean up URL
      searchParams.delete('success');
      setSearchParams(searchParams, { replace: true });

      return () => clearInterval(pollInterval);
    }
  }, [searchParams, apiClient]);

  const loadSubscriptionInfo = async () => {
    try {
      setLoading(true);
      const response = await apiClient.get('/api/v1/billing/subscription-info');
      setSubscriptionInfo(response.data);
    } catch (err) {
      setError('Failed to load subscription information');
    } finally {
      setLoading(false);
    }
  };

  const loadInvoices = async () => {
    try {
      setInvoicesLoading(true);
      const response = await apiClient.get('/api/v1/billing/invoices');
      setInvoices(response.data.invoices || []);
    } catch (err) {
      // Don't show error for invoices if user hasn't had any billing yet
      if (err.response?.status !== 404) {
        setError('Failed to load invoices');
      }
    } finally {
      setInvoicesLoading(false);
    }
  };

  const handleUpgrade = async (tier, billingCycle, additionalUsers = 0) => {
    try {
      setCheckoutLoading(true);
      setError(null);

      const response = await apiClient.post('/api/v1/billing/create-checkout', {
        tier,
        billing_cycle: billingCycle,
        additional_users: additionalUsers,
      });

      // Redirect to Stripe Checkout
      window.location.href = response.data.checkout_url;
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to start checkout');
      setCheckoutLoading(false);
    }
  };

  const handleCancelSubscription = async () => {
    const confirmed = await confirm({
      title: 'Cancel Subscription?',
      message: 'Are you sure you want to cancel your subscription? You will keep access until the end of your billing period.',
      confirmText: 'Cancel Subscription',
      variant: 'destructive'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.post('/api/v1/billing/cancel-subscription');
      setSuccess('Subscription will be cancelled at the end of your billing period');
      await loadSubscriptionInfo();
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to cancel subscription');
    }
  };

  const handleReactivateSubscription = async () => {
    const confirmed = await confirm({
      title: 'Reactivate Subscription?',
      message: 'Reactivate your subscription? Your subscription will continue after the current billing period.',
      confirmText: 'Reactivate',
      variant: 'default'
    });

    if (!confirmed) {
      return;
    }

    try {
      await apiClient.post('/api/v1/billing/reactivate-subscription');
      setSuccess('Subscription has been reactivated');
      await loadSubscriptionInfo();
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to reactivate subscription');
    }
  };

  const handleUpdateTeamSize = async () => {
    if (desiredTeamSize === subscriptionInfo.user_limit) {
      setError('Team size is already set to this value');
      return;
    }

    if (desiredTeamSize < 5) {
      setError('Team tier requires a minimum of 5 users');
      return;
    }

    try {
      setTeamSizeLoading(true);
      setError(null);
      setSuccess(null);

      const response = await apiClient.post('/api/v1/billing/update-team-size', {
        total_users: desiredTeamSize,
      });

      setSuccess(response.data.message || 'Team size updated successfully');
      await loadSubscriptionInfo();
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to update team size');
    } finally {
      setTeamSizeLoading(false);
    }
  };

  const handleManageBilling = async () => {
    try {
      setCheckoutLoading(true);
      setError(null);

      const response = await apiClient.post('/api/v1/billing/create-portal-session');

      // Redirect to Stripe Customer Portal
      window.location.href = response.data.portal_url;
    } catch (err) {
      setError(err.response?.data?.detail || 'Failed to open billing portal');
      setCheckoutLoading(false);
    }
  };

  // Show loading spinner while subscription info loads (backend is fast ~100ms)
  // Invoices load independently with their own spinner (Stripe is slow ~500ms)
  if (loading) {
    return (
      <div className="flex items-center justify-center p-8">
        <Spinner size="md" className="text-primary" />
      </div>
    );
  }

  const currentTier = subscriptionInfo?.tier || 'free';

  return (
    <div className="space-y-6" style={{display: 'block'}}>
      {/* Alerts */}
      {error && (
        <Alert variant="error" className="mb-4">
          {error}
        </Alert>
      )}
      {success && (
        <Alert variant="success" className="mb-4">
          {success}
        </Alert>
      )}

      {/* Current Plan */}
      <Card>
        <CardHeader>
          <div className="flex items-start justify-between">
            <div>
              <CardTitle>Current Plan</CardTitle>
              <CardDescription className="mt-1">
                {currentTier === 'free' ? 'Free Plan' : `${currentTier.charAt(0).toUpperCase() + currentTier.slice(1)} - ${subscriptionInfo.billing_cycle === 'annual' ? 'Annual' : 'Monthly'}`}
              </CardDescription>
            </div>
            <div className="flex items-center gap-3">
              {/* Status Badge */}
              {currentTier !== 'free' && subscriptionInfo.status && (
                <StatusBadge variant={
                  subscriptionInfo.status === 'active' ? 'success' :
                  subscriptionInfo.status === 'cancelled' ? 'warning' :
                  subscriptionInfo.status === 'past_due' ? 'error' :
                  'default'
                }>
                  {subscriptionInfo.status === 'active' ? 'Active' :
                   subscriptionInfo.status === 'cancelled' ? 'Cancelled' :
                   subscriptionInfo.status === 'past_due' ? 'Past Due' :
                   subscriptionInfo.status}
                </StatusBadge>
              )}
              {/* Manage Billing Button - Only for paid tiers */}
              {currentTier !== 'free' && (
                <Button
                  variant="outline"
                  size="sm"
                  onClick={handleManageBilling}
                  disabled={checkoutLoading}
                >
                  {checkoutLoading ? (
                    <>
                      <Spinner className="mr-2" />
                      Loading...
                    </>
                  ) : (
                    <>
                      <CreditCard className="w-4 h-4 mr-2" />
                      Manage Billing
                    </>
                  )}
                </Button>
              )}
            </div>
          </div>
        </CardHeader>
        <CardContent>
          <div className="space-y-4">
            {/* Subscription Status Info */}
            {currentTier !== 'free' && (
              <div className="bg-muted/50 border border-border rounded-lg p-4 space-y-2">
                {subscriptionInfo.status === 'active' && subscriptionInfo.period_end && (
                  <div key="active-subscription-info">
                    <div className="flex justify-between text-sm">
                      <span className="text-muted-foreground">Renews on</span>
                      <span className="font-medium text-foreground">
                        {new Date(subscriptionInfo.period_end).toLocaleDateString('en-US', {
                          month: 'long',
                          day: 'numeric',
                          year: 'numeric'
                        })}
                      </span>
                    </div>
                    <div className="flex justify-between text-sm">
                      <span className="text-muted-foreground">Next charge</span>
                      <span className="font-medium text-foreground">
                        {(() => {
                          const isAnnual = subscriptionInfo.billing_cycle === 'annual';
                          let basePrice = 0;

                          // Base tier prices (v2.0 PMF pricing)
                          // Support both 'basic' (alias) and 'starter' tier names
                          if (currentTier === 'basic' || currentTier === 'starter') {
                            basePrice = isAnnual ? 180 : 20;
                          } else if (currentTier === 'pro') {
                            basePrice = isAnnual ? 348 : 39;
                          } else if (currentTier === 'team') {
                            basePrice = isAnnual ? 1188 : 129;

                            // Add additional users cost for Team tier
                            const additionalUsers = Math.max(0, subscriptionInfo.user_limit - 5);
                            if (additionalUsers > 0) {
                              const perUserCost = isAnnual ? 180 : 20; // $15/mo annual ($180/yr) or $20/mo monthly
                              basePrice += (additionalUsers * perUserCost);
                            }
                          }

                          return `$${basePrice.toFixed(2)}`;
                        })()}
                      </span>
                    </div>
                  </div>
                )}
                {subscriptionInfo.status === 'cancelled' && subscriptionInfo.period_end && (
                  <div key="cancelled-subscription-info">
                    <div className="flex justify-between text-sm">
                      <span className="text-muted-foreground">Access until</span>
                      <span className="font-medium text-foreground">
                        {new Date(subscriptionInfo.period_end).toLocaleDateString('en-US', {
                          month: 'long',
                          day: 'numeric',
                          year: 'numeric'
                        })}
                      </span>
                    </div>
                    <div className="text-sm text-muted-foreground">
                      Your subscription has been cancelled. You'll retain access to paid features until the end of your billing period.
                    </div>
                  </div>
                )}
              </div>
            )}

            {/* Action Buttons - Only for paid tiers */}
            {currentTier !== 'free' && (
              <div className="flex gap-2 mt-4">
                {subscriptionInfo.status === 'active' && (
                  <>
                    <Button
                      variant="default"
                      onClick={() => setShowPlansModal(true)}
                    >
                      Change Plan
                    </Button>
                    <Button
                      variant="outline"
                      onClick={handleCancelSubscription}
                    >
                      Cancel Subscription
                    </Button>
                  </>
                )}
                {subscriptionInfo.status === 'cancelled' && (
                  <Button
                    variant="default"
                    onClick={handleReactivateSubscription}
                    className="w-full"
                  >
                    Reactivate Subscription
                  </Button>
                )}
              </div>
            )}
          </div>
        </CardContent>
      </Card>

      {/* Team Size Management - Only for Team tier */}
      {currentTier === 'team' && subscriptionInfo.status === 'active' && (
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Users className="w-5 h-5" />
              Team Size
            </CardTitle>
            <CardDescription>
              Manage your team size. Base plan includes 5 users, additional users are{' '}
              {subscriptionInfo.billing_cycle === 'annual' ? '$15/month (billed $180/year)' : '$20/month'} each.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="space-y-4">
              {/* Current Team Info */}
              <div className="bg-muted/50 border border-border rounded-lg p-4">
                <div className="flex justify-between text-sm mb-2">
                  <span className="text-muted-foreground">Current team size</span>
                  <span className="font-medium text-foreground">
                    {subscriptionInfo.user_limit} {subscriptionInfo.user_limit === 1 ? 'user' : 'users'}
                  </span>
                </div>
                {subscriptionInfo.user_limit > 5 && (
                  <div className="flex justify-between text-sm">
                    <span className="text-muted-foreground">Additional users</span>
                    <span className="font-medium text-foreground">
                      {subscriptionInfo.user_limit - 5} × {subscriptionInfo.billing_cycle === 'annual' ? '$15/mo' : '$20/mo'}
                    </span>
                  </div>
                )}
              </div>

              {/* Team Size Adjuster */}
              <div>
                <label className="text-sm font-medium text-foreground block mb-2">
                  Adjust team size
                </label>
                <div className="flex items-center gap-3">
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setDesiredTeamSize(Math.max(5, desiredTeamSize - 1))}
                    disabled={desiredTeamSize <= 5 || teamSizeLoading}
                  >
                    <Minus className="w-4 h-4" />
                  </Button>
                  <input
                    type="number"
                    value={desiredTeamSize}
                    onChange={(e) => setDesiredTeamSize(Math.max(5, parseInt(e.target.value) || 5))}
                    min="5"
                    className="w-20 px-3 py-2 text-center border border-border rounded-md bg-background text-foreground"
                    disabled={teamSizeLoading}
                  />
                  <Button
                    variant="outline"
                    size="sm"
                    onClick={() => setDesiredTeamSize(desiredTeamSize + 1)}
                    disabled={teamSizeLoading}
                  >
                    <Plus className="w-4 h-4" />
                  </Button>
                  <span className="text-sm text-muted-foreground">users</span>
                </div>
              </div>

              {/* Cost Preview */}
              {desiredTeamSize !== subscriptionInfo.user_limit && (
                <div className="bg-primary/10 border border-primary/20 rounded-lg p-4">
                  <div className="text-sm space-y-2">
                    <div className="flex justify-between">
                      <span className="text-foreground">Base Team plan (5 users)</span>
                      <span className="font-medium text-foreground">
                        {subscriptionInfo.billing_cycle === 'annual' ? '$99/mo' : '$129/mo'}
                      </span>
                    </div>
                    {desiredTeamSize > 5 && (
                      <div className="flex justify-between">
                        <span className="text-foreground">
                          Additional users ({desiredTeamSize - 5})
                        </span>
                        <span className="font-medium text-foreground">
                          {subscriptionInfo.billing_cycle === 'annual'
                            ? `$${((desiredTeamSize - 5) * 15).toFixed(2)}/mo`
                            : `$${((desiredTeamSize - 5) * 20).toFixed(2)}/mo`}
                        </span>
                      </div>
                    )}
                    <div className="pt-2 border-t border-primary/20 flex justify-between font-semibold">
                      <span className="text-foreground">New monthly total</span>
                      <span className="text-foreground">
                        {subscriptionInfo.billing_cycle === 'annual'
                          ? `$${(99 + (desiredTeamSize - 5) * 15).toFixed(2)}/mo`
                          : `$${(129 + (desiredTeamSize - 5) * 20).toFixed(2)}/mo`}
                      </span>
                    </div>
                    <p className="text-xs text-muted-foreground pt-2">
                      {desiredTeamSize > subscriptionInfo.user_limit
                        ? 'You will be charged a prorated amount for the remainder of your billing period.'
                        : 'You will receive a prorated credit on your next invoice.'}
                    </p>
                  </div>
                </div>
              )}

              {/* Update Button */}
              <Button
                onClick={handleUpdateTeamSize}
                disabled={desiredTeamSize === subscriptionInfo.user_limit || teamSizeLoading}
                className="w-full"
              >
                {teamSizeLoading ? (
                  <>
                    <Spinner className="mr-2" />
                    Updating...
                  </>
                ) : (
                  'Update Team Size'
                )}
              </Button>
            </div>
          </CardContent>
        </Card>
      )}

      {/* Available Plans - Only show for free tier, paid users get modal */}
      {currentTier === 'free' && (
        <div style={{display: 'block'}}>
          <h3 className="text-lg font-semibold mb-4 text-foreground">Available Plans</h3>
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4" style={{display: 'grid'}}>
            {/* Starter Plan */}
            <PlanCard
              name="Starter"
              annualPrice="$15"
              annualTotal="$180/year"
              monthlyPrice="$20/month"
              features={[
                'AI chat and analysis',
                '30 days query history',
                'Unlimited dashboards',
                'Website analytics (1M events/mo)',
                'MCP support',
                '1 user',
                'Email support',
              ]}
              currentTier={currentTier}
              onSelectAnnual={() => handleUpgrade('starter', 'annual')}
              onSelectMonthly={() => handleUpgrade('starter', 'monthly')}
              loading={checkoutLoading}
            />

            {/* Pro Plan */}
            <PlanCard
              name="Pro"
              annualPrice="$29"
              annualTotal="$348/year"
              monthlyPrice="$39/month"
              features={[
                '3x more AI usage vs Starter',
                'Kyomi Watch — proactive data monitoring',
                'Website analytics (5M events/mo)',
                'Unlimited query history',
                'PDF dashboard export',
                '1 user',
                'Priority email support',
              ]}
              recommended={true}
              currentTier={currentTier}
              onSelectAnnual={() => handleUpgrade('pro', 'annual')}
              onSelectMonthly={() => handleUpgrade('pro', 'monthly')}
              loading={checkoutLoading}
            />

            {/* Team Plan */}
            <PlanCard
              name="Team"
              annualPrice="$99"
              annualTotal="$1,188/year"
              monthlyPrice="$129/month"
              features={[
                'Shared AI pool for team',
                'Kyomi Watch — proactive data monitoring',
                'Website analytics (25M events/mo)',
                'Slack integration — alerts & @kyomi mentions',
                'Up to 5 users ($15-20/mo per additional)',
                'Dashboard sharing & collaboration',
                'Priority chat support',
              ]}
              currentTier={currentTier}
              onSelectAnnual={() => handleUpgrade('team', 'annual')}
              onSelectMonthly={() => handleUpgrade('team', 'monthly')}
              loading={checkoutLoading}
            />
          </div>
        </div>
      )}

      {/* Invoices Section - Show if user has invoices OR is on paid tier */}
      {(invoices.length > 0 || currentTier !== 'free') && (
        <div>
          <h3 className="text-lg font-semibold mb-4 text-foreground">
            {currentTier === 'free' && invoices.length > 0 ? 'Billing History' : 'Invoices'}
          </h3>
          {currentTier === 'free' && invoices.length > 0 && (
            <p className="text-sm text-muted-foreground mb-4">
              Your subscription has ended. Below are your past invoices for your records.
            </p>
          )}
          {invoicesLoading ? (
            <div className="flex items-center justify-center p-8">
              <Spinner size="md" className="text-primary" />
            </div>
          ) : invoices.length === 0 ? (
            <Card>
              <CardContent className="pt-6">
                <p className="text-sm text-muted-foreground text-center">
                  No invoices yet. Invoices will appear here after your first billing cycle.
                </p>
              </CardContent>
            </Card>
          ) : (
            <Card>
              <CardContent className="pt-6">
                <div className="overflow-x-auto">
                  <table className="w-full">
                    <thead>
                      <tr className="border-b border-border">
                        <th className="text-left py-3 px-4 text-sm font-medium text-muted-foreground">Date</th>
                        <th className="text-left py-3 px-4 text-sm font-medium text-muted-foreground">Description</th>
                        <th className="text-left py-3 px-4 text-sm font-medium text-muted-foreground">Amount</th>
                        <th className="text-left py-3 px-4 text-sm font-medium text-muted-foreground">Status</th>
                        <th className="text-right py-3 px-4 text-sm font-medium text-muted-foreground">Invoice</th>
                      </tr>
                    </thead>
                    <tbody>
                      {invoices.map((invoice, index) => {
                        const description = invoice.description ||
                          invoice.lines?.data?.[0]?.description ||
                          'Subscription';

                        return (
                          <tr key={invoice.id || `invoice-${index}`} className="border-b border-border last:border-0">
                            <td className="py-3 px-4 text-sm text-foreground">
                              {new Date(invoice.created * 1000).toLocaleDateString()}
                            </td>
                            <td className="py-3 px-4 text-sm text-foreground">
                              {description}
                            </td>
                            <td className="py-3 px-4 text-sm text-foreground">
                              ${invoice.amount_paid.toFixed(2)}
                            </td>
                          <td className="py-3 px-4 text-sm">
                            <StatusBadge variant={
                              invoice.status === 'paid' ? 'success' :
                              invoice.status === 'open' ? 'warning' :
                              'error'
                            }>
                              {invoice.status === 'paid' ? 'Paid' : invoice.status === 'open' ? 'Pending' : 'Failed'}
                            </StatusBadge>
                          </td>
                          <td className="py-3 px-4 text-sm text-right">
                            {invoice.invoice_pdf && (
                              <a
                                href={invoice.invoice_pdf}
                                target="_blank"
                                rel="noopener noreferrer"
                                className="inline-flex items-center gap-1 text-primary hover:text-primary/80 transition-colors"
                              >
                                <FileText className="w-4 h-4" />
                                <span>PDF</span>
                                <ExternalLink className="w-3 h-3" />
                              </a>
                            )}
                          </td>
                        </tr>
                        );
                      })}
                    </tbody>
                  </table>
                </div>
              </CardContent>
            </Card>
          )}
        </div>
      )}

      {/* Billing Cycle Info - Only show for free tier users */}
      {currentTier === 'free' && (
        <Card className="bg-muted/50">
          <CardContent className="pt-6">
            <p className="text-sm text-muted-foreground">
              <strong>Annual billing saves 25-30%</strong> compared to monthly billing.
            </p>
          </CardContent>
        </Card>
      )}

      {/* Change Plan Modal */}
      <Modal
        show={showPlansModal}
        onClose={() => setShowPlansModal(false)}
        title="Change Plan"
        size="xl"
      >
        <p className="text-sm text-muted-foreground mb-6">
          Select a new plan to upgrade or downgrade your subscription
        </p>

        <div style={{display: 'block'}}>
          <div className="grid grid-cols-1 md:grid-cols-3 gap-4 mb-6" style={{display: 'grid'}}>
            {/* Starter Plan */}
            <PlanCard
              name="Starter"
              annualPrice="$15"
              annualTotal="$180/year"
              monthlyPrice="$20/month"
              features={[
                'AI chat and analysis',
                '30 days query history',
                'Unlimited dashboards',
                'Website analytics (1M events/mo)',
                'MCP support',
                '1 user',
                'Email support',
              ]}
              currentTier={currentTier}
              currentBillingCycle={subscriptionInfo.billing_cycle}
              onSelectAnnual={() => handleUpgrade('starter', 'annual')}
              onSelectMonthly={() => handleUpgrade('starter', 'monthly')}
              loading={checkoutLoading}
            />

            {/* Pro Plan */}
            <PlanCard
              name="Pro"
              annualPrice="$29"
              annualTotal="$348/year"
              monthlyPrice="$39/month"
              features={[
                '3x more AI usage vs Starter',
                'Kyomi Watch — proactive data monitoring',
                'Website analytics (5M events/mo)',
                'Unlimited query history',
                'PDF dashboard export',
                '1 user',
                'Priority email support',
              ]}
              recommended={true}
              currentTier={currentTier}
              currentBillingCycle={subscriptionInfo.billing_cycle}
              onSelectAnnual={() => handleUpgrade('pro', 'annual')}
              onSelectMonthly={() => handleUpgrade('pro', 'monthly')}
              loading={checkoutLoading}
            />

            {/* Team Plan */}
            <PlanCard
              name="Team"
              annualPrice="$99"
              annualTotal="$1,188/year"
              monthlyPrice="$129/month"
              features={[
                'Shared AI pool for team',
                'Kyomi Watch — proactive data monitoring',
                'Website analytics (25M events/mo)',
                'Slack integration — alerts & @kyomi mentions',
                'Up to 5 users ($15-20/mo per additional)',
                'Dashboard sharing & collaboration',
                'Priority chat support',
              ]}
              currentTier={currentTier}
              currentBillingCycle={subscriptionInfo.billing_cycle}
              onSelectAnnual={() => handleUpgrade('team', 'annual')}
              onSelectMonthly={() => handleUpgrade('team', 'monthly')}
              loading={checkoutLoading}
            />
          </div>

          {/* Billing Cycle Info in Modal */}
          <Card className="bg-muted/50">
            <CardContent className="pt-6">
              <p className="text-sm text-muted-foreground">
                <strong>Annual billing saves 25-30%</strong> compared to monthly billing.
              </p>
            </CardContent>
          </Card>
        </div>
      </Modal>

      {/* Confirm Dialog */}
      <ConfirmDialog isOpen={isOpen} {...dialogProps} />
    </div>
  );
}

/**
 * PlanCard - Individual plan display with pricing and features
 */
function PlanCard({
  name,
  annualPrice,
  annualTotal,
  monthlyPrice,
  features,
  recommended = false,
  currentTier,
  currentBillingCycle,
  onSelectAnnual,
  onSelectMonthly,
  loading,
}) {
  const planTier = name.toLowerCase();
  const isCurrentAnnual = currentTier === planTier && currentBillingCycle === 'annual';
  const isCurrentMonthly = currentTier === planTier && currentBillingCycle === 'monthly';

  return (
    <Card className={`relative ${recommended ? 'border-primary border-2' : ''}`}>
      {recommended && (
        <div className="absolute -top-3 left-1/2 -translate-x-1/2">
          <span className="bg-primary text-primary-foreground px-3 py-1 rounded-full text-xs font-semibold">
            Best Value
          </span>
        </div>
      )}
      <CardHeader>
        <CardTitle className="text-xl">{name}</CardTitle>
        <CardDescription>
          <div className="mt-2">
            <div className="text-2xl font-bold text-foreground">
              {annualPrice}<span className="text-sm font-normal text-muted-foreground">/month*</span>
            </div>
            <div className="text-xs text-muted-foreground">{annualTotal}</div>
            <div className="text-sm text-muted-foreground mt-1">
              or {monthlyPrice}
            </div>
          </div>
        </CardDescription>
      </CardHeader>
      <CardContent>
        <ul className="space-y-2 mb-6">
          {features.map((feature) => (
            <li key={feature} className="flex items-start gap-2 text-sm">
              <Check className="w-4 h-4 text-primary mt-0.5 flex-shrink-0" />
              <span className="text-foreground">{feature}</span>
            </li>
          ))}
        </ul>

        <div className="space-y-2">
          <Button
            onClick={onSelectAnnual}
            disabled={loading || isCurrentAnnual}
            variant={isCurrentAnnual ? "outline" : "default"}
            className="w-full"
          >
            {isCurrentAnnual ? (
              'Current Plan'
            ) : loading ? (
              <>
                <Spinner className="mr-2" />
                Choose Annual
              </>
            ) : (
              <>
                <CreditCard className="w-4 h-4 mr-2" />
                Choose Annual
              </>
            )}
          </Button>
          <Button
            onClick={onSelectMonthly}
            disabled={loading || isCurrentMonthly}
            variant={isCurrentMonthly ? "default" : "outline"}
            className="w-full"
          >
            {isCurrentMonthly ? 'Current Plan' : 'Choose Monthly'}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
