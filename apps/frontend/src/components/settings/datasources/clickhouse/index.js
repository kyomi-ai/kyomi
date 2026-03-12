// SPDX-License-Identifier: AGPL-3.0-or-later
// clickhouse/index.js
import { schema } from './schema.js';

export default {
  type: 'clickhouse',
  label: 'ClickHouse',
  schema,
  // No custom CredentialsSection - uses generic password fields
};
