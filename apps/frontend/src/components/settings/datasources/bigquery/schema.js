// SPDX-License-Identifier: AGPL-3.0-or-later
// bigquery/schema.js
import {
  billingProjectField,
  defaultProjectField,
  globalOAuthAuthMode,
  enterpriseOAuthAuthMode,
  serviceAccountAuthMode,
} from '../shared/schemas';

export const schema = {
  type: 'bigquery',
  label: 'BigQuery',

  // No connection fields - BigQuery uses OAuth/service account
  connectionFields: [],

  // No discovery fields in connection tab - projects handled in credentials section
  discoveryFields: [],

  // BigQuery supports 3 authentication modes
  authModes: [
    {
      ...globalOAuthAuthMode('Google'),
      value: 'kyomi_oauth',
      isDefault: true,
      // After OAuth connect, show project dropdowns
      credentialFields: [billingProjectField(), defaultProjectField()],
    },
    {
      ...enterpriseOAuthAuthMode('BigQuery'),
      value: 'enterprise_oauth',
      callbackPath: '/auth/oauth/bigquery-enterprise/callback',
      credentialFields: [billingProjectField(), defaultProjectField()],
    },
    {
      ...serviceAccountAuthMode(),
      credentialFields: [billingProjectField(), defaultProjectField()],
    },
  ],

  catalogConfig: {
    key: 'catalog_projects',
    label: 'Projects to Index',
    discoveryKey: 'projects',
    helpText: 'Select which projects to include in the catalog.',
  },

  sshTunnelSupported: false,
};
