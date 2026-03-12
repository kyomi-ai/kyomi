// SPDX-License-Identifier: AGPL-3.0-or-later
import { useState, useEffect } from 'react';
import { useAuth } from '../context/AuthContext';
import { Card, CardHeader, CardTitle, CardDescription, CardContent } from './ui/card';
import { Alert, AlertTitle, AlertDescription } from './ui/alert';
import { AlertTriangle } from 'lucide-react';
import { Spinner } from './ui/spinner';

/**
 * UsagePanel - AI usage tracking for workspace and team members
 *
 * Features:
 * - Workspace-level AI usage progress bar
 * - Per-user breakdown showing fair share allocation
 * - Feature breakdown (chat, dashboard copilot, chart builder copilot)
 * - Privacy-preserving (percentages only, no message counts)
 */
export default function UsagePanel() {
  const { apiClient, user } = useAuth();
  const [loading, setLoading] = useState(true);
  const [usageData, setUsageData] = useState(null);
  const [error, setError] = useState(null);

  useEffect(() => {
    loadUsageData();
  }, []);

  const loadUsageData = async () => {
    try {
      setLoading(true);
      const response = await apiClient.get('/api/v1/billing/ai-usage-status');
      setUsageData(response.data);
    } catch (err) {
      setError('Failed to load usage information');
    } finally {
      setLoading(false);
    }
  };

  if (loading) {
    return (
      <div className="flex items-center justify-center p-8">
        <Spinner size="md" className="text-primary" />
      </div>
    );
  }

  const workspacePercentage = usageData?.percentage_used || 0;
  const isExhausted = usageData?.blocked || false;
  const userBreakdown = usageData?.by_user || [];
  const featureBreakdown = usageData?.by_feature || {};
  const currentTier = user?.subscription_tier || 'free';

  return (
    <div className="space-y-6" style={{display: 'block'}}>
      {error && (
        <Alert variant="error">
          <AlertTitle>Error</AlertTitle>
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}

      {/* Workspace AI Usage */}
      <Card>
        <CardHeader>
          <CardTitle>Workspace AI Usage</CardTitle>
          <CardDescription>
            Track your AI usage
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div>
            <div className="flex justify-between mb-2">
              <span className="text-sm font-medium text-foreground">
                AI Usage This Month
              </span>
              <span className="text-sm font-medium text-foreground">
                {workspacePercentage.toFixed(1)}% used
              </span>
            </div>
            <div className="w-full bg-muted rounded-full h-2">
              <div
                className={`h-2 rounded-full transition-all ${
                  isExhausted || workspacePercentage >= 100
                    ? 'bg-error-foreground' // Bold red: 100%+ exhausted
                    : workspacePercentage >= 90
                    ? 'bg-error-foreground' // Bold red: 90-99% critical
                    : workspacePercentage >= 80
                    ? 'bg-warning-foreground' // Bold orange: 80-89% warning
                    : 'bg-success-foreground' // Bold green: 0-79% healthy
                }`}
                style={{ width: `${Math.min(100, workspacePercentage)}%` }}
              />
            </div>
            {isExhausted && (
              <p className="text-sm text-error-foreground mt-2">
                AI budget exhausted. Upgrade to continue using AI features.
              </p>
            )}
            {usageData?.ai_reset_date && (
              <p className="text-xs text-muted-foreground mt-1">
                Resets {new Date(usageData.ai_reset_date).toLocaleDateString('en-US', {
                  month: 'short',
                  day: 'numeric',
                  year: 'numeric'
                })}
              </p>
            )}
          </div>
        </CardContent>
      </Card>

      {/* Team Usage Breakdown - Only show if there are multiple users */}
      {userBreakdown.length > 1 && (
        <Card>
          <CardHeader>
            <CardTitle>Team Usage Breakdown</CardTitle>
            <CardDescription>
              Each team member's usage as a percentage of their fair share allocation.
              Values over 100% indicate usage above fair share.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <div className="space-y-3" style={{display: 'block'}}>
              {userBreakdown.map((userData, index) => {
                const percentage = userData.percentage_of_allocation || 0;
                const isCurrentUser = userData.user_id === user?.user_id;
                const isOverAllocation = userData.is_over_allocation || false;

                // Determine bar color based on usage - use bold foreground colors
                let barColor = 'bg-success-foreground'; // Bold green for 0-80%
                if (percentage > 100) {
                  barColor = 'bg-error-foreground'; // Bold red for >100%
                } else if (percentage >= 80) {
                  barColor = 'bg-warning-foreground'; // Bold orange for 80-100%
                }

                return (
                  <div key={userData.user_id} className="space-y-1">
                    <div className="flex justify-between items-center">
                      <span className="text-sm font-medium text-foreground">
                        {userData.name || userData.email}
                        {isCurrentUser && <span className="text-muted-foreground ml-1">(you)</span>}
                      </span>
                      <div className="flex items-center gap-2">
                        <span className={`text-sm font-medium ${isOverAllocation ? 'text-error-foreground' : 'text-foreground'}`}>
                          {percentage.toFixed(0)}%
                        </span>
                        {isOverAllocation && (
                          <AlertTriangle className="w-4 h-4 text-error" />
                        )}
                      </div>
                    </div>
                    <div className="w-full bg-muted rounded-full h-2">
                      <div
                        className={`h-2 rounded-full transition-all ${barColor}`}
                        style={{ width: `${Math.min(100, percentage)}%` }}
                      />
                    </div>
                  </div>
                );
              })}
            </div>

            {/* Explanation for over-allocation */}
            {userBreakdown.some(u => u.is_over_allocation) && (
              <Alert variant="info" className="mt-4">
                <AlertDescription className="text-sm">
                  <strong>Note:</strong> Team members can use more than their fair share allocation
                  as long as the total workspace usage stays within the plan limit. This provides
                  flexibility for varying workloads across the team.
                </AlertDescription>
              </Alert>
            )}
          </CardContent>
        </Card>
      )}

      {/* Feature Breakdown - Always show if we have usage data */}
      {usageData && (
        <Card>
          <CardHeader>
            <CardTitle>Usage by Feature</CardTitle>
            <CardDescription>
              Distribution of AI usage across different features
            </CardDescription>
          </CardHeader>
          <CardContent>
            {/* Stacked horizontal bar showing distribution */}
            {/* Using first 4 colors from balanced palette (chartPalettes.js) */}
            <div className="w-full h-8 bg-muted rounded-lg overflow-hidden flex mb-4">
              {(featureBreakdown.chat || 0) > 0 && (
                <div
                  className="transition-all flex items-center justify-center"
                  style={{ width: `${featureBreakdown.chat}%`, backgroundColor: '#1A75C9' }}
                  title={`Chat: ${featureBreakdown.chat.toFixed(0)}%`}
                />
              )}
              {(featureBreakdown.kyomi_watch || 0) > 0 && (
                <div
                  className="transition-all flex items-center justify-center"
                  style={{ width: `${featureBreakdown.kyomi_watch}%`, backgroundColor: '#8B5CF6' }}
                  title={`Watch: ${featureBreakdown.kyomi_watch.toFixed(0)}%`}
                />
              )}
              {(featureBreakdown.dashboard_copilot || 0) > 0 && (
                <div
                  className="transition-all flex items-center justify-center"
                  style={{ width: `${featureBreakdown.dashboard_copilot}%`, backgroundColor: '#B8405A' }}
                  title={`Dashboard Copilot: ${featureBreakdown.dashboard_copilot.toFixed(0)}%`}
                />
              )}
              {(featureBreakdown.chart_builder_copilot || 0) > 0 && (
                <div
                  className="transition-all flex items-center justify-center"
                  style={{ width: `${featureBreakdown.chart_builder_copilot}%`, backgroundColor: '#3D8A5A' }}
                  title={`Chart Builder Copilot: ${featureBreakdown.chart_builder_copilot.toFixed(0)}%`}
                />
              )}
            </div>

            {/* Legend - always show all features */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <div className="w-3 h-3 rounded-sm" style={{ backgroundColor: '#1A75C9' }}></div>
                  <span className="text-sm text-foreground">Chat Interface</span>
                </div>
                <span className="text-sm font-medium text-foreground">
                  {(featureBreakdown.chat || 0).toFixed(0)}%
                </span>
              </div>
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <div className="w-3 h-3 rounded-sm" style={{ backgroundColor: '#8B5CF6' }}></div>
                  <span className="text-sm text-foreground">Watch</span>
                </div>
                <span className="text-sm font-medium text-foreground">
                  {(featureBreakdown.kyomi_watch || 0).toFixed(0)}%
                </span>
              </div>
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <div className="w-3 h-3 rounded-sm" style={{ backgroundColor: '#B8405A' }}></div>
                  <span className="text-sm text-foreground">Dashboard Copilot</span>
                </div>
                <span className="text-sm font-medium text-foreground">
                  {(featureBreakdown.dashboard_copilot || 0).toFixed(0)}%
                </span>
              </div>
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-2">
                  <div className="w-3 h-3 rounded-sm" style={{ backgroundColor: '#3D8A5A' }}></div>
                  <span className="text-sm text-foreground">Chart Builder Copilot</span>
                </div>
                <span className="text-sm font-medium text-foreground">
                  {(featureBreakdown.chart_builder_copilot || 0).toFixed(0)}%
                </span>
              </div>
            </div>
          </CardContent>
        </Card>
      )}
    </div>
  );
}
