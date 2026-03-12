// SPDX-License-Identifier: AGPL-3.0-or-later
// sqlserver/index.js
import { schema } from './schema.js';

export default {
  type: 'sqlserver',
  label: 'SQL Server',
  schema,
  // No custom CredentialsSection - uses generic password fields
};
