// SPDX-License-Identifier: AGPL-3.0-or-later
// mysql/index.js
import { schema } from './schema.js';

export default {
  type: 'mysql',
  label: 'MySQL',
  schema,
  // No custom CredentialsSection - uses generic password fields
};
