function y(t) {
  const o = typeof t == "string" ? new Date(t).getTime() : t, e = new Date(o), n = Date.now(), a = n - o, s = Math.floor(a / 1e3), i = Math.floor(s / 60), l = Math.floor(i / 60), h = Math.floor(l / 24);
  if (s < 10) return "just now";
  if (s < 60) return `${s} seconds ago`;
  if (i === 1) return "1 minute ago";
  if (i < 60) return `${i} minutes ago`;
  if (l === 1) return "1 hour ago";
  if (l < 24) return `${l} hours ago`;
  const u = new Date(n);
  return u.setDate(u.getDate() - 1), e.toDateString() === u.toDateString() ? `yesterday ${e.toLocaleTimeString(void 0, {
    hour: "numeric",
    minute: "2-digit"
  })}` : h < 7 ? e.toLocaleDateString(void 0, {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit"
  }) : e.toLocaleDateString(void 0, {
    month: "short",
    day: "numeric"
  });
}
function x(t) {
  const o = typeof t == "string" ? new Date(t).getTime() : t, e = Date.now() - o, n = Math.floor(e / 1e3), a = Math.floor(n / 60), s = Math.floor(a / 60), i = Math.floor(s / 24);
  return n < 60 ? "now" : a < 60 ? `${a}m` : s < 24 ? `${s}h` : i < 7 ? `${i}d` : new Date(o).toLocaleDateString(void 0, { month: "short", day: "numeric" });
}
const c = (t) => `xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" width="${t}" height="${t}"`, w = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.992 0 3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 13.803-3.7l3.181 3.182M21.015 4.356v4.992"/></svg>`, _ = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="m16.862 4.487 1.687-1.688a1.875 1.875 0 1 1 2.652 2.652L10.582 16.07a4.5 4.5 0 0 1-1.897 1.13L6 18l.8-2.685a4.5 4.5 0 0 1 1.13-1.897l8.932-8.931Zm0 0L19.5 7.125M18 14v4.75A2.25 2.25 0 0 1 15.75 21H5.25A2.25 2.25 0 0 1 3 18.75V8.25A2.25 2.25 0 0 1 5.25 6H10"/></svg>`, M = (t = 14) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M12 6v6h4.5m4.5 0a9 9 0 1 1-18 0 9 9 0 0 1 18 0Z"/></svg>`, T = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="m14.74 9-.346 9m-4.788 0L9.26 9m9.968-3.21c.342.052.682.107 1.022.166m-1.022-.165L18.16 19.673a2.25 2.25 0 0 1-2.244 2.077H8.084a2.25 2.25 0 0 1-2.244-2.077L4.772 5.79m14.456 0a48.108 48.108 0 0 0-3.478-.397m-12 .562c.34-.059.68-.114 1.022-.165m0 0a48.11 48.11 0 0 1 3.478-.397m7.5 0v-.916c0-1.18-.91-2.164-2.09-2.201a51.964 51.964 0 0 0-3.32 0c-1.18.037-2.09 1.022-2.09 2.201v.916m7.5 0a48.667 48.667 0 0 0-7.5 0"/></svg>`, $ = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M13.5 16.875h3.375m0 0h3.375m-3.375 0V13.5m0 3.375v3.375M6 10.5h2.25a2.25 2.25 0 0 0 2.25-2.25V6a2.25 2.25 0 0 0-2.25-2.25H6A2.25 2.25 0 0 0 3.75 6v2.25A2.25 2.25 0 0 0 6 10.5Zm0 9.75h2.25A2.25 2.25 0 0 0 10.5 18v-2.25a2.25 2.25 0 0 0-2.25-2.25H6a2.25 2.25 0 0 0-2.25 2.25V18A2.25 2.25 0 0 0 6 20.25Zm9.75-9.75H18a2.25 2.25 0 0 0 2.25-2.25V6A2.25 2.25 0 0 0 18 3.75h-2.25A2.25 2.25 0 0 0 13.5 6v2.25a2.25 2.25 0 0 0 2.25 2.25Z"/></svg>`, A = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="m11.25 11.25.041-.02a.75.75 0 0 1 1.063.852l-.708 2.836a.75.75 0 0 0 1.063.853l.041-.021M21 12a9 9 0 1 1-18 0 9 9 0 0 1 18 0Zm-9-3.75h.008v.008H12V8.25Z"/></svg>`, E = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M20.25 8.511c.884.284 1.5 1.128 1.5 2.097v4.286c0 1.136-.847 2.1-1.98 2.193-.34.027-.68.052-1.02.072v3.091l-3-3c-1.354 0-2.694-.055-4.02-.163a2.115 2.115 0 0 1-.825-.242m9.345-8.334a2.126 2.126 0 0 0-.476-.095 48.64 48.64 0 0 0-8.048 0c-1.131.094-1.976 1.057-1.976 2.192v4.286c0 .837.46 1.58 1.155 1.951m9.345-8.334V6.637c0-1.621-1.152-3.026-2.76-3.235A48.455 48.455 0 0 0 11.25 3c-2.115 0-4.198.137-6.24.402-1.608.209-2.76 1.614-2.76 3.235v6.226c0 1.621 1.152 3.026 2.76 3.235.577.075 1.157.14 1.74.194V21l4.155-4.155"/></svg>`, C = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M3 13.125C3 12.504 3.504 12 4.125 12h2.25c.621 0 1.125.504 1.125 1.125v6.75C7.5 20.496 6.996 21 6.375 21h-2.25A1.125 1.125 0 0 1 3 19.875v-6.75ZM9.75 8.625c0-.621.504-1.125 1.125-1.125h2.25c.621 0 1.125.504 1.125 1.125v11.25c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V8.625ZM16.5 4.125c0-.621.504-1.125 1.125-1.125h2.25C20.496 3 21 3.504 21 4.125v15.75c0 .621-.504 1.125-1.125 1.125h-2.25a1.125 1.125 0 0 1-1.125-1.125V4.125Z"/></svg>`, H = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M2.25 18 9 11.25l4.306 4.306a11.95 11.95 0 0 1 5.814-5.518l2.74-1.22m0 0-5.94-2.281m5.94 2.28-2.28 5.941"/></svg>`, L = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M3 20l4-8 4 4 4-10 4 6v8H3Z"/><path stroke-linecap="round" stroke-linejoin="round" d="M3 20l4-8 4 4 4-10 4 6"/></svg>`, S = (t = 16) => `<svg ${c(t)}><circle cx="5" cy="17" r="1.5"/><circle cx="8" cy="10" r="1.5"/><circle cx="12" cy="14" r="1.5"/><circle cx="14" cy="7" r="1.5"/><circle cx="17" cy="12" r="1.5"/><circle cx="20" cy="5" r="1.5"/></svg>`, I = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M12 3a9 9 0 1 0 9 9h-9V3Z"/><path stroke-linecap="round" stroke-linejoin="round" d="M14 2.05A9 9 0 0 1 21.95 10H14V2.05Z"/></svg>`, j = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M12 3a9 9 0 1 0 9 9h-9V3Z"/><path stroke-linecap="round" stroke-linejoin="round" d="M14 2.05A9 9 0 0 1 21.95 10H14V2.05Z"/><circle cx="12" cy="12" r="4" fill="var(--chb-bg, #f4f4f5)" stroke="currentColor" stroke-width="1.5"/></svg>`, R = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M3.375 19.5h17.25m-17.25 0a1.125 1.125 0 0 1-1.125-1.125M3.375 19.5h7.5c.621 0 1.125-.504 1.125-1.125m-9.75 0V5.625m0 12.75v-1.5c0-.621.504-1.125 1.125-1.125m18.375 2.625V5.625m0 12.75c0 .621-.504 1.125-1.125 1.125m1.125-1.125v-1.5c0-.621-.504-1.125-1.125-1.125m0 3.75h-7.5A1.125 1.125 0 0 1 12 18.375m9.75-12.75c0-.621-.504-1.125-1.125-1.125H3.375c-.621 0-1.125.504-1.125 1.125m19.5 0v1.5c0 .621-.504 1.125-1.125 1.125M2.25 5.625v1.5c0 .621.504 1.125 1.125 1.125m0 0h17.25m-17.25 0h7.5c.621 0 1.125.504 1.125 1.125M3.375 8.25c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125m17.25-3.75h-7.5c-.621 0-1.125.504-1.125 1.125m8.625-1.125c.621 0 1.125.504 1.125 1.125v1.5c0 .621-.504 1.125-1.125 1.125m-17.25 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125M12 10.875v-1.5m0 1.5c0 .621-.504 1.125-1.125 1.125M12 10.875c0 .621.504 1.125 1.125 1.125m-2.25 0c.621 0 1.125.504 1.125 1.125M10.875 12h-7.5m8.625 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125M20.625 12c.621 0 1.125.504 1.125 1.125v1.5c0 .621-.504 1.125-1.125 1.125m-17.25 0h7.5m-7.5 0c-.621 0-1.125.504-1.125 1.125v1.5c0 .621.504 1.125 1.125 1.125M12 13.875v-1.5m0 1.5c0 .621-.504 1.125-1.125 1.125M12 13.875c0 .621.504 1.125 1.125 1.125m-2.25 0c.621 0 1.125.504 1.125 1.125M10.875 15h-7.5"/></svg>`, z = (t = 16) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="M5.25 8.25h15m-16.5 7.5h15m-1.8-13.5-3.9 19.5m-2.1-19.5-3.9 19.5"/></svg>`, D = (t = 12) => `<svg ${c(t)}><path stroke-linecap="round" stroke-linejoin="round" d="m19.5 8.25-7.5 7.5-7.5-7.5"/></svg>`, V = `
:host {
  display: block;
  container-type: inline-size;
  font-family: var(--chb-font-family, system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, 'Helvetica Neue', Arial, sans-serif);
}

.bar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 4px 8px;
  background: var(--chb-bg, #f4f4f5);
  border: 1px solid var(--chb-border, #e4e4e7);
  border-radius: 8px 8px 0 0;
  min-width: 0;
}

@container (min-width: 480px) {
  .bar {
    padding: 4px 16px;
  }
}

/* Left side — timestamp area */
.left {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.timestamp-full {
  display: none;
  font-size: 12px;
  line-height: 16px;
  color: var(--chb-text, #71717a);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

@container (min-width: 480px) {
  .timestamp-full {
    display: block;
  }
  .timestamp-compact {
    display: none !important;
  }
}

.timestamp-compact {
  display: flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  line-height: 16px;
  color: var(--chb-text, #71717a);
  cursor: default;
}

/* Right side — action buttons */
.actions {
  display: flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
}

@container (min-width: 480px) {
  .actions {
    gap: 8px;
  }
}

/* Individual action button */
.btn {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 4px;
  border: none;
  background: transparent;
  color: var(--chb-text, #71717a);
  border-radius: 6px;
  cursor: pointer;
  transition: color 150ms, background-color 150ms;
  line-height: 0;
}

@container (min-width: 480px) {
  .btn {
    padding: 6px;
  }
}

.btn:hover {
  color: var(--chb-text-hover, #18181b);
  background: var(--chb-accent-hover, rgba(0,0,0,0.06));
}

.btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.btn.destructive:hover {
  color: var(--chb-destructive, #dc2626);
  background: var(--chb-destructive-hover-bg, rgba(220,38,38,0.1));
}

/* JS-positioned tooltip (single shared element) */
.tooltip {
  position: fixed;
  background: var(--chb-tooltip-bg, white);
  color: var(--chb-tooltip-text, rgb(55, 65, 81));
  padding: 6px 10px;
  border-radius: 4px;
  font-size: 11px;
  font-family: system-ui;
  pointer-events: none;
  opacity: 0;
  z-index: 10000;
  box-shadow: rgba(0, 0, 0, 0.15) 0px 2px 8px;
  border: 1px solid var(--chb-tooltip-border, rgb(229, 231, 235));
  max-width: 300px;
  white-space: pre-wrap;
  transition: opacity 100ms;
}

.tooltip.visible {
  opacity: 1;
}

/* Spin animation for refresh */
@keyframes spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.spin svg {
  animation: spin 1s linear infinite;
}

/* Non-breaking space placeholder */
.nbsp::before {
  content: "\\00A0";
}

/* ── Type selector dropdown ── */

.type-selector {
  position: relative;
}

.type-selector-trigger {
  display: flex;
  align-items: center;
  gap: 2px;
  padding: 4px;
  border: none;
  background: transparent;
  color: var(--chb-text, #71717a);
  border-radius: 6px;
  cursor: pointer;
  transition: color 150ms, background-color 150ms;
  line-height: 0;
}

@container (min-width: 480px) {
  .type-selector-trigger {
    padding: 6px;
    gap: 4px;
  }
}

.type-selector-trigger:hover {
  color: var(--chb-text-hover, #18181b);
  background: var(--chb-accent-hover, rgba(0,0,0,0.06));
}

.type-menu {
  position: absolute;
  right: 0;
  top: 100%;
  margin-top: 4px;
  background: var(--chb-menu-bg, white);
  border: 1px solid var(--chb-border, #e4e4e7);
  border-radius: 8px;
  box-shadow: 0 4px 12px var(--chb-menu-shadow, rgba(0,0,0,0.12));
  z-index: 100;
  padding: 4px;
  min-width: 140px;
}

.type-menu-item {
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 8px;
  border: none;
  background: transparent;
  color: var(--chb-text, #71717a);
  border-radius: 4px;
  cursor: pointer;
  font-size: 12px;
  font-family: inherit;
  line-height: 16px;
  transition: color 100ms, background-color 100ms;
  white-space: nowrap;
}

.type-menu-item:hover {
  color: var(--chb-text-hover, #18181b);
  background: var(--chb-accent-hover, rgba(0,0,0,0.06));
}

.type-menu-item.active {
  color: var(--chb-text-hover, #18181b);
  font-weight: 600;
}

/* ── Modifier chips ── */

.modifier-chip {
  display: inline-flex;
  align-items: center;
  padding: 2px 5px;
  font-size: 11px;
  font-weight: 500;
  border-radius: 9999px;
  border: 1px solid var(--chb-border, #e4e4e7);
  background: transparent;
  color: var(--chb-text, #71717a);
  cursor: pointer;
  transition: all 0.15s ease;
  white-space: nowrap;
  font-family: var(--chb-font-family, system-ui, sans-serif);
}

.modifier-chip .chip-full { display: none; }

@container (min-width: 480px) {
  .modifier-chip {
    padding: 2px 8px;
  }
  .modifier-chip .chip-full { display: inline; }
  .modifier-chip .chip-abbr { display: none; }
}

.modifier-chip:hover {
  border-color: var(--chb-text-hover, #3f3f46);
  color: var(--chb-text-hover, #3f3f46);
}

.modifier-chip.active {
  background: var(--chb-accent-hover, rgba(0,0,0,0.06));
  border-color: var(--chb-text-hover, #18181b);
  color: var(--chb-text-hover, #18181b);
}
`, v = [
  { key: "bar", type: "bar", label: "Bar", icon: C },
  { key: "line", type: "line", label: "Line", icon: H },
  { key: "area", type: "area", label: "Area", icon: L },
  { key: "scatter", type: "scatter", label: "Scatter", icon: S },
  { key: "pie", type: "pie", label: "Pie", icon: I },
  { key: "doughnut", type: "doughnut", label: "Doughnut", icon: j },
  { key: "table", type: "table", label: "Table", icon: R },
  { key: "metric", type: "metric", label: "Metric", icon: z }
], Z = new Set(v.map((t) => t.type));
class P extends HTMLElement {
  static get observedAttributes() {
    return [
      "last-updated",
      "refreshing",
      "show-refresh",
      "show-edit",
      "show-delete",
      "show-save-to-dashboard",
      "show-info",
      "show-ask-about",
      "chart-type",
      "chart-orientation",
      "chart-mode",
      "show-type-selector"
    ];
  }
  constructor() {
    super(), this.attachShadow({ mode: "open" }), this._intervalId = null, this._tooltipEl = null, this._tooltipTimeout = null, this._typeMenuOpen = !1, this._outsideClickHandler = null, this._escapeHandler = null;
  }
  connectedCallback() {
    this._render(), this._startTimestampUpdater();
  }
  disconnectedCallback() {
    this._stopTimestampUpdater(), this._hideTooltip(), this._removeMenuListeners();
  }
  attributeChangedCallback() {
    this.shadowRoot && (this._render(), this._startTimestampUpdater());
  }
  // ── Helpers ──
  _bool(o) {
    return this.hasAttribute(o);
  }
  _emit(o, e = null) {
    this.dispatchEvent(
      new CustomEvent(o, {
        bubbles: !0,
        composed: !0,
        ...e !== null && { detail: e }
      })
    );
  }
  _startTimestampUpdater() {
    this._stopTimestampUpdater(), this.getAttribute("last-updated") && (this._intervalId = setInterval(() => this._updateTimestamps(), 3e4));
  }
  _stopTimestampUpdater() {
    this._intervalId && (clearInterval(this._intervalId), this._intervalId = null);
  }
  // ── Tooltip ──
  _showTooltip(o, e) {
    clearTimeout(this._tooltipTimeout), this._tooltipTimeout = setTimeout(() => {
      if (!this._tooltipEl) return;
      this._tooltipEl.textContent = e;
      const n = o.getBoundingClientRect();
      let a = n.left + n.width / 2, s = n.bottom + 6;
      this._tooltipEl.classList.add("visible");
      const i = this._tooltipEl.getBoundingClientRect(), l = document.documentElement.clientWidth, h = document.documentElement.clientHeight;
      a - i.width / 2 < 4 && (a = 4 + i.width / 2), a + i.width / 2 > l - 4 && (a = l - 4 - i.width / 2), s + i.height > h - 4 && (s = n.top - i.height - 6), this._tooltipEl.style.left = `${a - i.width / 2}px`, this._tooltipEl.style.top = `${s}px`;
    }, 400);
  }
  _hideTooltip() {
    clearTimeout(this._tooltipTimeout), this._tooltipEl && this._tooltipEl.classList.remove("visible");
  }
  _bindTooltips() {
    this.shadowRoot.querySelectorAll("[data-tip]").forEach((e) => {
      e.addEventListener("mouseenter", () => {
        this._showTooltip(e, e.dataset.tip);
      }), e.addEventListener("mouseleave", () => {
        this._hideTooltip();
      });
    });
  }
  // ── Type menu ──
  _toggleTypeMenu() {
    this._typeMenuOpen ? this._closeTypeMenu() : (this._typeMenuOpen = !0, this._addMenuListeners(), this._render());
  }
  _closeTypeMenu() {
    this._typeMenuOpen && (this._typeMenuOpen = !1, this._removeMenuListeners(), this._render());
  }
  _addMenuListeners() {
    this._outsideClickHandler = (o) => {
      const e = o.composedPath(), n = this.shadowRoot.querySelector(".type-selector");
      n && !e.includes(n) && this._closeTypeMenu();
    }, this._escapeHandler = (o) => {
      o.key === "Escape" && this._closeTypeMenu();
    }, document.addEventListener("click", this._outsideClickHandler, !0), document.addEventListener("keydown", this._escapeHandler);
  }
  _removeMenuListeners() {
    this._outsideClickHandler && (document.removeEventListener("click", this._outsideClickHandler, !0), this._outsideClickHandler = null), this._escapeHandler && (document.removeEventListener("keydown", this._escapeHandler), this._escapeHandler = null);
  }
  _selectType(o) {
    this._closeTypeMenu();
    const e = v.find((n) => n.key === o);
    e && this._emit("header-type-change", { type: e.type });
  }
  _toggleOrientation() {
    const e = this.getAttribute("chart-orientation") === "horizontal" ? null : "horizontal";
    this._emit("header-orientation-change", { orientation: e });
  }
  _toggleMode() {
    const o = this.getAttribute("chart-type"), e = this.getAttribute("chart-mode");
    if (o === "bar") {
      const n = e === "grouped" ? null : "grouped";
      this._emit("header-mode-change", { mode: n });
    } else if (o === "area") {
      const n = e === "normalized" ? null : "normalized";
      this._emit("header-mode-change", { mode: n });
    }
  }
  // ── Timestamp ──
  _updateTimestamps() {
    const o = this.getAttribute("last-updated");
    if (!o) return;
    const e = Number(o), n = `Last refreshed ${y(e)}`, a = this.shadowRoot.querySelector(".timestamp-full"), s = this.shadowRoot.querySelector(".timestamp-compact");
    if (a && (a.textContent = n), s) {
      s.dataset.tip = n;
      const i = s.querySelector("span");
      i && (i.textContent = x(e));
    }
  }
  // ── Render ──
  _render() {
    const o = this.getAttribute("last-updated"), e = o ? Number(o) : null, n = this._bool("refreshing"), a = e ? `Last refreshed ${y(e)}` : "", s = e ? x(e) : "", i = [];
    this._bool("show-refresh") && i.push({
      event: "header-refresh",
      icon: w(),
      tooltip: "Refresh data",
      disabled: n,
      cls: n ? "spin" : ""
    }), this._bool("show-edit") && i.push({
      event: "header-edit",
      icon: _(),
      tooltip: "Edit chart"
    }), this._bool("show-save-to-dashboard") && i.push({
      event: "header-save-to-dashboard",
      icon: $(),
      tooltip: "Save to dashboard"
    }), this._bool("show-ask-about") && i.push({
      event: "header-ask-about",
      icon: E(),
      tooltip: "Ask about this chart"
    }), this._bool("show-info") && i.push({
      event: "header-info",
      icon: A(),
      tooltip: "Chart info"
    }), this._bool("show-delete") && i.push({
      event: "header-delete",
      icon: T(),
      tooltip: "Delete chart",
      cls: "destructive"
    });
    const l = i.map(
      (r) => `<button class="btn ${r.cls || ""}" data-event="${r.event}" data-tip="${r.tooltip}" aria-label="${r.tooltip}"${r.disabled ? " disabled" : ""}>${r.icon}</button>`
    ).join("");
    let h;
    e ? h = `
        <span class="timestamp-full">${a}</span>
        <span class="timestamp-compact" data-tip="${a}">
          ${M()}
          <span>${s}</span>
        </span>` : h = '<span class="nbsp"></span>';
    let u = "", b = "";
    const m = this.getAttribute("chart-type"), k = this.getAttribute("chart-orientation"), g = this.getAttribute("chart-mode");
    if (this._bool("show-type-selector") && m && Z.has(m)) {
      const r = v.find((d) => d.type === m), p = v.map(
        (d) => `<button class="type-menu-item${d.key === (r == null ? void 0 : r.key) ? " active" : ""}" data-key="${d.key}">${d.icon(14)}<span>${d.label}</span></button>`
      ).join("");
      u = `
        <div class="type-selector">
          <button class="type-selector-trigger" data-tip="Change chart type" aria-label="Change chart type">
            ${r ? r.icon(14) : ""}${D()}
          </button>
          ${this._typeMenuOpen ? `<div class="type-menu">${p}</div>` : ""}
        </div>`, m === "bar" ? b = `
          <button class="modifier-chip ${k === "horizontal" ? "active" : ""}"
                  data-chip="orientation" data-tip="Toggle horizontal orientation"><span class="chip-full">Horizontal</span><span class="chip-abbr">Horiz</span></button>
          <button class="modifier-chip ${g === "grouped" ? "active" : ""}"
                  data-chip="mode" data-tip="Toggle grouped mode"><span class="chip-full">Grouped</span><span class="chip-abbr">Grpd</span></button>` : m === "area" && (b = `
          <button class="modifier-chip ${g === "normalized" ? "active" : ""}"
                  data-chip="mode" data-tip="Toggle 100% stacked"><span class="chip-full">Normalized</span><span class="chip-abbr">Norm</span></button>`);
    }
    this.shadowRoot.innerHTML = `
      <style>${V}</style>
      <div class="bar">
        <div class="left"><slot name="before"></slot>${h}</div>
        <div class="actions"><slot name="actions-before"></slot>${u}${b}${l}</div>
      </div>
      <div class="tooltip"></div>
    `, this._tooltipEl = this.shadowRoot.querySelector(".tooltip"), this.shadowRoot.querySelectorAll(".btn[data-event]").forEach((r) => {
      r.addEventListener("click", (p) => {
        p.stopPropagation(), this._emit(r.dataset.event);
      });
    });
    const f = this.shadowRoot.querySelector(".type-selector-trigger");
    f && f.addEventListener("click", (r) => {
      r.stopPropagation(), this._toggleTypeMenu();
    }), this.shadowRoot.querySelectorAll(".type-menu-item").forEach((r) => {
      r.addEventListener("click", (p) => {
        p.stopPropagation(), this._selectType(r.dataset.key);
      });
    }), this.shadowRoot.querySelectorAll(".modifier-chip").forEach((r) => {
      r.addEventListener("click", (p) => {
        p.stopPropagation(), r.dataset.chip === "orientation" ? this._toggleOrientation() : r.dataset.chip === "mode" && this._toggleMode();
      });
    }), this._bindTooltips();
  }
}
customElements.get("chart-header-bar") || customElements.define("chart-header-bar", P);
export {
  P as ChartHeaderBarElement
};
