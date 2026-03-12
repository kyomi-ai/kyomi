// SPDX-License-Identifier: AGPL-3.0-or-later
import { ChartHeaderBarElement } from './chart-header-bar.js';

export { ChartHeaderBarElement };

// Register the custom element (idempotent)
if (!customElements.get('chart-header-bar')) {
  customElements.define('chart-header-bar', ChartHeaderBarElement);
}
