// SPDX-License-Identifier: AGPL-3.0-or-later
// postgres/index.js
import { schema } from './schema.js';

export default {
  type: 'postgres',
  label: 'PostgreSQL',
  schema,
  // No custom CredentialsSection - uses generic password fields
};
