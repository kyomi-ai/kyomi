// SPDX-License-Identifier: AGPL-3.0-or-later
import React, { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import TrialChat from '../components/TrialChat';
import { trackEvent } from '../utils/analytics';
import { TrialCapabilitiesProvider } from '../context/TrialCapabilitiesProvider';

/**
 * Try Page - Anonymous trial experience
 *
 * Allows users to try Kyomi without signing up by querying a sample dataset.
 * Limited to 5 queries per session, 10 per IP per day.
 */
export default function Try() {
  const navigate = useNavigate();

  useEffect(() => {
    // Track page view
    trackEvent('trial_page_viewed');
  }, []);

  return (
    <TrialCapabilitiesProvider>
      <div className="h-screen flex flex-col bg-muted">
        {/* Full-height trial chat experience */}
        <TrialChat />
      </div>
    </TrialCapabilitiesProvider>
  );
}
