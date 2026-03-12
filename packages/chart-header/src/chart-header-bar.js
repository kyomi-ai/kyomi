// SPDX-License-Identifier: AGPL-3.0-or-later
import { formatRelativeTime, formatRelativeTimeCompact } from './formatters.js';
import {
  ArrowPathIcon,
  PencilIcon,
  ClockIcon,
  TrashIcon,
  SquaresPlusIcon,
  InformationCircleIcon,
  ChatBubbleLeftRightIcon,
  BarChartIcon,
  LineChartIcon,
  AreaChartIcon,
  ScatterChartIcon,
  PieChartIcon,
  DoughnutChartIcon,
  TableIcon,
  MetricIcon,
  ChevronDownIcon,
} from './icons.js';
import { styles } from './styles.js';

const CHART_TYPE_OPTIONS = [
  { key: 'bar', type: 'bar', label: 'Bar', icon: BarChartIcon },
  { key: 'line', type: 'line', label: 'Line', icon: LineChartIcon },
  { key: 'area', type: 'area', label: 'Area', icon: AreaChartIcon },
  { key: 'scatter', type: 'scatter', label: 'Scatter', icon: ScatterChartIcon },
  { key: 'pie', type: 'pie', label: 'Pie', icon: PieChartIcon },
  { key: 'doughnut', type: 'doughnut', label: 'Doughnut', icon: DoughnutChartIcon },
  { key: 'table', type: 'table', label: 'Table', icon: TableIcon },
  { key: 'metric', type: 'metric', label: 'Metric', icon: MetricIcon },
];

const SWITCHABLE_TYPES = new Set(CHART_TYPE_OPTIONS.map((o) => o.type));

/**
 * <chart-header-bar> — Custom element for chart chrome header.
 *
 * Attributes:
 *   last-updated          timestamp ms (string) — omit to show nothing
 *   refreshing            boolean — spinning animation on refresh icon
 *   show-refresh          boolean — show refresh button
 *   show-edit             boolean — show edit button
 *   show-delete           boolean — show delete button
 *   show-save-to-dashboard boolean — show save-to-dashboard button
 *   show-info             boolean — show info button
 *   show-ask-about        boolean — show ask-about-this-chart button
 *   chart-type            string — current chart type (bar, line, area, scatter, pie, doughnut, table, metric)
 *   chart-orientation     string — "horizontal" for horizontal bar, omit otherwise
 *   chart-mode            string — "stacked" | "grouped" | "normalized", omit for default
 *   show-type-selector    boolean — show the chart type dropdown
 *
 * Events (CustomEvent, bubbles + composed):
 *   header-refresh, header-edit, header-delete,
 *   header-save-to-dashboard, header-info, header-ask-about,
 *   header-type-change (detail: { type })
 *   header-orientation-change (detail: { orientation })
 *   header-mode-change (detail: { mode })
 *
 * CSS custom properties:
 *   --chb-bg, --chb-border, --chb-text, --chb-text-hover,
 *   --chb-accent-hover, --chb-destructive, --chb-destructive-hover-bg,
 *   --chb-font-family
 *
 * Slots:
 *   <slot name="before"> — injected before the timestamp (e.g. drag handle)
 */
export class ChartHeaderBarElement extends HTMLElement {
  static get observedAttributes() {
    return [
      'last-updated',
      'refreshing',
      'show-refresh',
      'show-edit',
      'show-delete',
      'show-save-to-dashboard',
      'show-info',
      'show-ask-about',
      'chart-type',
      'chart-orientation',
      'chart-mode',
      'show-type-selector',
    ];
  }

  constructor() {
    super();
    this.attachShadow({ mode: 'open' });
    this._intervalId = null;
    this._tooltipEl = null;
    this._tooltipTimeout = null;
    this._typeMenuOpen = false;
    this._outsideClickHandler = null;
    this._escapeHandler = null;
  }

  connectedCallback() {
    this._render();
    this._startTimestampUpdater();
  }

  disconnectedCallback() {
    this._stopTimestampUpdater();
    this._hideTooltip();
    this._removeMenuListeners();
  }

  attributeChangedCallback() {
    if (this.shadowRoot) {
      this._render();
      this._startTimestampUpdater();
    }
  }

  // ── Helpers ──

  _bool(attr) {
    return this.hasAttribute(attr);
  }

  _emit(name, detail = null) {
    this.dispatchEvent(
      new CustomEvent(name, {
        bubbles: true,
        composed: true,
        ...(detail !== null && { detail }),
      })
    );
  }

  _startTimestampUpdater() {
    this._stopTimestampUpdater();
    if (this.getAttribute('last-updated')) {
      this._intervalId = setInterval(() => this._updateTimestamps(), 30_000);
    }
  }

  _stopTimestampUpdater() {
    if (this._intervalId) {
      clearInterval(this._intervalId);
      this._intervalId = null;
    }
  }

  // ── Tooltip ──

  _showTooltip(target, text) {
    clearTimeout(this._tooltipTimeout);
    this._tooltipTimeout = setTimeout(() => {
      if (!this._tooltipEl) return;
      this._tooltipEl.textContent = text;

      // Position below the target, centered horizontally
      const rect = target.getBoundingClientRect();
      let left = rect.left + rect.width / 2;
      let top = rect.bottom + 6;

      // Measure tooltip to clamp within viewport
      this._tooltipEl.classList.add('visible');
      const tipRect = this._tooltipEl.getBoundingClientRect();

      // Clamp horizontal: keep fully within viewport
      const vw = document.documentElement.clientWidth;
      const vh = document.documentElement.clientHeight;
      if (left - tipRect.width / 2 < 4) left = 4 + tipRect.width / 2;
      if (left + tipRect.width / 2 > vw - 4) left = vw - 4 - tipRect.width / 2;

      // If tooltip would overflow bottom, show above instead
      if (top + tipRect.height > vh - 4) {
        top = rect.top - tipRect.height - 6;
      }

      this._tooltipEl.style.left = `${left - tipRect.width / 2}px`;
      this._tooltipEl.style.top = `${top}px`;
    }, 400);
  }

  _hideTooltip() {
    clearTimeout(this._tooltipTimeout);
    if (this._tooltipEl) {
      this._tooltipEl.classList.remove('visible');
    }
  }

  _bindTooltips() {
    const targets = this.shadowRoot.querySelectorAll('[data-tip]');
    targets.forEach((el) => {
      el.addEventListener('mouseenter', () => {
        this._showTooltip(el, el.dataset.tip);
      });
      el.addEventListener('mouseleave', () => {
        this._hideTooltip();
      });
    });
  }

  // ── Type menu ──

  _toggleTypeMenu() {
    if (this._typeMenuOpen) {
      this._closeTypeMenu();
    } else {
      this._typeMenuOpen = true;
      this._addMenuListeners();
      this._render();
    }
  }

  _closeTypeMenu() {
    if (!this._typeMenuOpen) return;
    this._typeMenuOpen = false;
    this._removeMenuListeners();
    this._render();
  }

  _addMenuListeners() {
    this._outsideClickHandler = (e) => {
      // composedPath() crosses shadow DOM boundaries
      const path = e.composedPath();
      const menu = this.shadowRoot.querySelector('.type-selector');
      if (menu && !path.includes(menu)) {
        this._closeTypeMenu();
      }
    };
    this._escapeHandler = (e) => {
      if (e.key === 'Escape') this._closeTypeMenu();
    };
    document.addEventListener('click', this._outsideClickHandler, true);
    document.addEventListener('keydown', this._escapeHandler);
  }

  _removeMenuListeners() {
    if (this._outsideClickHandler) {
      document.removeEventListener('click', this._outsideClickHandler, true);
      this._outsideClickHandler = null;
    }
    if (this._escapeHandler) {
      document.removeEventListener('keydown', this._escapeHandler);
      this._escapeHandler = null;
    }
  }

  _selectType(key) {
    this._closeTypeMenu();
    const option = CHART_TYPE_OPTIONS.find((o) => o.key === key);
    if (option) {
      this._emit('header-type-change', { type: option.type });
    }
  }

  _toggleOrientation() {
    const current = this.getAttribute('chart-orientation');
    const newOrientation = current === 'horizontal' ? null : 'horizontal';
    this._emit('header-orientation-change', { orientation: newOrientation });
  }

  _toggleMode() {
    const chartType = this.getAttribute('chart-type');
    const current = this.getAttribute('chart-mode');

    if (chartType === 'bar') {
      // Toggle between stacked (default) and grouped
      const newMode = current === 'grouped' ? null : 'grouped';
      this._emit('header-mode-change', { mode: newMode });
    } else if (chartType === 'area') {
      // Toggle between stacked (default) and normalized
      const newMode = current === 'normalized' ? null : 'normalized';
      this._emit('header-mode-change', { mode: newMode });
    }
  }

  // ── Timestamp ──

  _updateTimestamps() {
    const ts = this.getAttribute('last-updated');
    if (!ts) return;

    const num = Number(ts);
    const fullText = `Last refreshed ${formatRelativeTime(num)}`;
    const fullEl = this.shadowRoot.querySelector('.timestamp-full');
    const compactEl = this.shadowRoot.querySelector('.timestamp-compact');

    if (fullEl) fullEl.textContent = fullText;
    if (compactEl) {
      compactEl.dataset.tip = fullText;
      const span = compactEl.querySelector('span');
      if (span) span.textContent = formatRelativeTimeCompact(num);
    }
  }

  // ── Render ──

  _render() {
    const ts = this.getAttribute('last-updated');
    const tsNum = ts ? Number(ts) : null;
    const refreshing = this._bool('refreshing');

    const fullText = tsNum ? `Last refreshed ${formatRelativeTime(tsNum)}` : '';
    const compactText = tsNum ? formatRelativeTimeCompact(tsNum) : '';

    // Build action buttons
    const buttons = [];

    if (this._bool('show-refresh')) {
      buttons.push({
        event: 'header-refresh',
        icon: ArrowPathIcon(),
        tooltip: 'Refresh data',
        disabled: refreshing,
        cls: refreshing ? 'spin' : '',
      });
    }

    if (this._bool('show-edit')) {
      buttons.push({
        event: 'header-edit',
        icon: PencilIcon(),
        tooltip: 'Edit chart',
      });
    }

    if (this._bool('show-save-to-dashboard')) {
      buttons.push({
        event: 'header-save-to-dashboard',
        icon: SquaresPlusIcon(),
        tooltip: 'Save to dashboard',
      });
    }

    if (this._bool('show-ask-about')) {
      buttons.push({
        event: 'header-ask-about',
        icon: ChatBubbleLeftRightIcon(),
        tooltip: 'Ask about this chart',
      });
    }

    if (this._bool('show-info')) {
      buttons.push({
        event: 'header-info',
        icon: InformationCircleIcon(),
        tooltip: 'Chart info',
      });
    }

    if (this._bool('show-delete')) {
      buttons.push({
        event: 'header-delete',
        icon: TrashIcon(),
        tooltip: 'Delete chart',
        cls: 'destructive',
      });
    }

    const buttonsHtml = buttons
      .map(
        (b) =>
          `<button class="btn ${b.cls || ''}" data-event="${b.event}" data-tip="${b.tooltip}" aria-label="${b.tooltip}"${b.disabled ? ' disabled' : ''}>${b.icon}</button>`
      )
      .join('');

    // Timestamp section
    let timestampHtml;
    if (tsNum) {
      timestampHtml = `
        <span class="timestamp-full">${fullText}</span>
        <span class="timestamp-compact" data-tip="${fullText}">
          ${ClockIcon()}
          <span>${compactText}</span>
        </span>`;
    } else {
      timestampHtml = '<span class="nbsp"></span>';
    }

    // Build type selector HTML and modifier chips
    let typeSelectorHtml = '';
    let chipsHtml = '';
    const chartType = this.getAttribute('chart-type');
    const chartOrientation = this.getAttribute('chart-orientation');
    const chartMode = this.getAttribute('chart-mode');
    if (this._bool('show-type-selector') && chartType && SWITCHABLE_TYPES.has(chartType)) {
      const currentOption = CHART_TYPE_OPTIONS.find((o) => o.type === chartType);
      const menuItemsHtml = CHART_TYPE_OPTIONS.map(
        (o) =>
          `<button class="type-menu-item${o.key === currentOption?.key ? ' active' : ''}" data-key="${o.key}">${o.icon(14)}<span>${o.label}</span></button>`
      ).join('');

      typeSelectorHtml = `
        <div class="type-selector">
          <button class="type-selector-trigger" data-tip="Change chart type" aria-label="Change chart type">
            ${currentOption ? currentOption.icon(14) : ''}${ChevronDownIcon()}
          </button>
          ${this._typeMenuOpen ? `<div class="type-menu">${menuItemsHtml}</div>` : ''}
        </div>`;

      // Build contextual modifier chips
      if (chartType === 'bar') {
        const horizActive = chartOrientation === 'horizontal';
        const groupedActive = chartMode === 'grouped';
        chipsHtml = `
          <button class="modifier-chip ${horizActive ? 'active' : ''}"
                  data-chip="orientation" data-tip="Toggle horizontal orientation"><span class="chip-full">Horizontal</span><span class="chip-abbr">Horiz</span></button>
          <button class="modifier-chip ${groupedActive ? 'active' : ''}"
                  data-chip="mode" data-tip="Toggle grouped mode"><span class="chip-full">Grouped</span><span class="chip-abbr">Grpd</span></button>`;
      } else if (chartType === 'area') {
        const normActive = chartMode === 'normalized';
        chipsHtml = `
          <button class="modifier-chip ${normActive ? 'active' : ''}"
                  data-chip="mode" data-tip="Toggle 100% stacked"><span class="chip-full">Normalized</span><span class="chip-abbr">Norm</span></button>`;
      }
    }

    this.shadowRoot.innerHTML = `
      <style>${styles}</style>
      <div class="bar">
        <div class="left"><slot name="before"></slot>${timestampHtml}</div>
        <div class="actions"><slot name="actions-before"></slot>${typeSelectorHtml}${chipsHtml}${buttonsHtml}</div>
      </div>
      <div class="tooltip"></div>
    `;

    this._tooltipEl = this.shadowRoot.querySelector('.tooltip');

    // Bind click handlers
    this.shadowRoot.querySelectorAll('.btn[data-event]').forEach((btn) => {
      btn.addEventListener('click', (e) => {
        e.stopPropagation();
        this._emit(btn.dataset.event);
      });
    });

    // Bind type selector
    const trigger = this.shadowRoot.querySelector('.type-selector-trigger');
    if (trigger) {
      trigger.addEventListener('click', (e) => {
        e.stopPropagation();
        this._toggleTypeMenu();
      });
    }
    this.shadowRoot.querySelectorAll('.type-menu-item').forEach((item) => {
      item.addEventListener('click', (e) => {
        e.stopPropagation();
        this._selectType(item.dataset.key);
      });
    });

    // Bind modifier chip click handlers
    this.shadowRoot.querySelectorAll('.modifier-chip').forEach((chip) => {
      chip.addEventListener('click', (e) => {
        e.stopPropagation();
        if (chip.dataset.chip === 'orientation') {
          this._toggleOrientation();
        } else if (chip.dataset.chip === 'mode') {
          this._toggleMode();
        }
      });
    });

    // Bind tooltip hover handlers
    this._bindTooltips();
  }
}
