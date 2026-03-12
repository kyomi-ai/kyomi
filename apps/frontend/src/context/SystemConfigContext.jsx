// SPDX-License-Identifier: AGPL-3.0-or-later
import { createContext, useContext, useState, useEffect } from 'react';

const SystemConfigContext = createContext();

// Default feature flags: all true while loading to avoid false negatives
const DEFAULT_FEATURES = {
  ai_enabled: true,
  smtp_configured: true,
  chart_renderer_configured: true,
  slack_configured: true,
  pdf_export: true,
  watch_email_alerts: true,
  watch_slack_alerts: true,
  slack_integration: true,
  website_analytics: true,
};

export function useSystemConfig() {
  const context = useContext(SystemConfigContext);
  if (!context) {
    throw new Error('useSystemConfig must be used within a SystemConfigProvider');
  }
  return context;
}

export function SystemConfigProvider({ children }) {
  const [selfHosted, setSelfHosted] = useState(false);
  const [edition, setEdition] = useState('saas');
  const [features, setFeatures] = useState(DEFAULT_FEATURES);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState(null);

  useEffect(() => {
    // Raw fetch — this endpoint requires no auth and is called before login
    fetch('/api/v1/system/config')
      .then((res) => {
        if (!res.ok) {
          throw new Error(`System config returned ${res.status}`);
        }
        return res.json();
      })
      .then((data) => {
        setSelfHosted(data.self_hosted ?? false);
        setEdition(data.edition ?? 'saas');
        setFeatures({ ...DEFAULT_FEATURES, ...data.features });
      })
      .catch((err) => {
        // On error, keep defaults (all features enabled) so the app works normally
        setError(err.message);
      })
      .finally(() => {
        setLoading(false);
      });
  }, []);

  const value = {
    selfHosted,
    edition,
    features,
    loading,
    error,
  };

  return (
    <SystemConfigContext.Provider value={value}>
      {children}
    </SystemConfigContext.Provider>
  );
}
