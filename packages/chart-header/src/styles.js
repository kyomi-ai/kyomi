// SPDX-License-Identifier: AGPL-3.0-or-later
/**
 * Shadow DOM CSS for <chart-header-bar>.
 *
 * Uses CSS custom properties with sensible defaults so consumers
 * can theme the component from outside the shadow boundary.
 */
export const styles = `
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
`;
