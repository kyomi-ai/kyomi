// SPDX-License-Identifier: AGPL-3.0-or-later
// redshift/index.js
import { schema } from './schema.js';

export default {
  type: 'redshift',
  label: 'Amazon Redshift',
  schema,
  // No custom CredentialsSection - uses generic password fields
};
