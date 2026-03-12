// SPDX-License-Identifier: AGPL-3.0-or-later
import { App, applyDocumentTheme } from "@modelcontextprotocol/ext-apps";
import { ChartML } from "@chartml/core";
import "@chartml/core/style.css";
import "@kyomi/chart-header";
import yaml from "js-yaml";

// Convert visualize structure when switching between incompatible type categories
function convertVisualizeForTypeChange(viz, fromType, toType) {
  const cat = (t) => t === "table" ? "table" : t === "metric" ? "metric" : "chart";
  const fromCat = cat(fromType), toCat = cat(toType);
  if (fromCat === toCat) return;
  const toArr = (v) => Array.isArray(v) ? v : v ? [v] : [];
  const addField = (f, arr) => { if (typeof f === "string") arr.push({ field: f }); else if (f && typeof f === "object") arr.push({ ...f }); };

  if (fromCat === "table") {
    const cols = Array.isArray(viz.columns) ? viz.columns : [];
    if (toCat === "chart") {
      const first = cols[0];
      viz.columns = first?.field || first;
      viz.rows = cols.slice(1).map(c => { if (typeof c === "string") return c; const { width, ...k } = c; return Object.keys(k).length === 1 && k.field ? k.field : k; });
    } else {
      viz.value = (cols[1] || cols[0])?.field; delete viz.columns; delete viz.rows;
    }
  } else if (fromCat === "metric") {
    const v = viz.value, l = viz.label, f = viz.format;
    delete viz.value; delete viz.label; delete viz.format; delete viz.compareWith; delete viz.invertTrend;
    if (toCat === "chart") { delete viz.columns; viz.rows = v ? [v] : []; }
    else { viz.columns = v ? [{ field: v, ...(l && { label: l }), ...(f && { format: f }) }] : []; delete viz.rows; }
  } else {
    const sc = toArr(viz.columns), sr = toArr(viz.rows);
    if (toCat === "table") {
      const cols = []; sc.forEach(f => addField(f, cols)); sr.forEach(f => addField(f, cols)); viz.columns = cols; delete viz.rows;
    } else {
      const firstRow = sr[0]; viz.value = typeof firstRow === "string" ? firstRow : firstRow?.field; delete viz.columns; delete viz.rows;
    }
  }
}

const container = document.getElementById("chart");

// 1. Create app instance (autoResize reports content size to host)
const app = new App({ name: "Kyomi Chart Viewer", version: "1.0.0" });

// Theme: sync with host's light/dark mode.
// applyDocumentTheme sets `color-scheme` on <html> (activates prefers-color-scheme),
// and we also toggle the `.dark` class for ChartML's explicit class-based selector.
// Backgrounds are transparent so the host page color shows through naturally.
function applyHostContext(ctx) {
  if (ctx.theme) {
    applyDocumentTheme(ctx.theme);
    document.documentElement.classList.toggle("dark", ctx.theme === "dark");
  }
}

// Listen for host theme changes (e.g. user toggles Claude.ai dark mode)
app.onhostcontextchanged = (params) => {
  applyHostContext(params);
  // Re-render the chart so D3-generated inline styles pick up new CSS vars
  if (params.theme && lastSpec) renderChart(lastSpec, lastPalette);
};

let lastSpec = null;
let lastSourceSpec = null;
let lastPalette = null;
let chartContextId = null;
let appUrl = null;
let infoPanelOpen = false;
let dashboardPanelOpen = false;

const SWITCHABLE_TYPES = new Set(["bar", "line", "area", "scatter", "pie", "doughnut", "table", "metric"]);

// -- Info panel helpers --

function createCopyButton(text) {
  const btn = document.createElement("button");
  btn.className = "info-copy-btn";
  btn.textContent = "Copy";
  btn.addEventListener("click", async () => {
    try {
      await navigator.clipboard.writeText(text);
      btn.textContent = "Copied!";
      btn.classList.add("copied");
      setTimeout(() => {
        btn.textContent = "Copy";
        btn.classList.remove("copied");
      }, 2000);
    } catch {
      btn.textContent = "Failed";
      setTimeout(() => { btn.textContent = "Copy"; }, 2000);
    }
  });
  return btn;
}

function createCodeSection(label, code) {
  const section = document.createElement("div");
  section.className = "info-section";

  const row = document.createElement("div");
  row.className = "info-label-row";

  const labelEl = document.createElement("span");
  labelEl.className = "info-label";
  labelEl.textContent = label;
  row.appendChild(labelEl);
  row.appendChild(createCopyButton(code));
  section.appendChild(row);

  const pre = document.createElement("pre");
  pre.className = "info-code";
  pre.textContent = code;
  section.appendChild(pre);

  return section;
}

function createInfoPanel(sourceSpec) {
  const panel = document.createElement("div");
  panel.className = "chart-info-panel";

  // Datasource (from original spec which has datasource + query)
  const ds = sourceSpec.data?.datasource || sourceSpec.data?.source || "Not specified";
  const dsSection = document.createElement("div");
  dsSection.className = "info-section";
  const dsLabel = document.createElement("span");
  dsLabel.className = "info-label";
  dsLabel.textContent = "Datasource";
  const dsValue = document.createElement("span");
  dsValue.className = "info-value";
  dsValue.textContent = ds;
  dsSection.appendChild(dsLabel);
  dsSection.appendChild(dsValue);
  panel.appendChild(dsSection);

  // SQL Query (from original spec)
  const query = sourceSpec.data?.query?.trim();
  if (query) {
    panel.appendChild(createCodeSection("SQL Query", query));
  }

  // ChartML Source (YAML of original spec with datasource + query)
  const chartYaml = yaml.dump(sourceSpec, { lineWidth: -1, quotingType: '"', forceQuotes: false });
  panel.appendChild(createCodeSection("ChartML Source", chartYaml));

  return panel;
}

// -- Dashboard panel helpers --

function getChartMarkdownBlock() {
  if (!lastSourceSpec) return null;
  const chartYaml = yaml.dump(lastSourceSpec, { lineWidth: -1, quotingType: '"', forceQuotes: false });
  return "```chartml\n" + chartYaml + "```";
}

async function callMcpTool(name, args) {
  const result = await app.callServerTool({ name, arguments: args });
  if (result.isError) {
    const errorText = result.content?.map(c => c.text).join("\n") || "Unknown error";
    throw new Error(errorText);
  }
  const text = result.content?.[0]?.text;
  if (!text) throw new Error("Empty response from server");
  return JSON.parse(text);
}

function createDashboardPanel() {
  const panel = document.createElement("div");
  panel.className = "dashboard-panel";

  let activeTab = "create";
  let dashboards = null;
  let loadingDashboards = false;
  let selectedDashboardId = null;
  let saving = false;

  function render() {
    panel.innerHTML = "";

    // Tab bar
    const tabs = document.createElement("div");
    tabs.className = "dashboard-tabs";

    const createTab = document.createElement("button");
    createTab.className = "dashboard-tab" + (activeTab === "create" ? " active" : "");
    createTab.textContent = "Create New";
    createTab.addEventListener("click", () => {
      activeTab = "create";
      render();
    });

    const existingTab = document.createElement("button");
    existingTab.className = "dashboard-tab" + (activeTab === "existing" ? " active" : "");
    existingTab.textContent = "Add to Existing";
    existingTab.addEventListener("click", () => {
      activeTab = "existing";
      if (!dashboards && !loadingDashboards) loadDashboards();
      render();
    });

    tabs.appendChild(createTab);
    tabs.appendChild(existingTab);
    panel.appendChild(tabs);

    if (activeTab === "create") {
      renderCreateTab();
    } else {
      renderExistingTab();
    }
  }

  function renderCreateTab() {
    const form = document.createElement("div");
    form.className = "dashboard-form";

    const label = document.createElement("label");
    label.className = "dashboard-field-label";
    label.textContent = "Dashboard Title";
    form.appendChild(label);

    const input = document.createElement("input");
    input.type = "text";
    input.className = "dashboard-input";
    input.placeholder = "Enter dashboard title...";
    // Pre-fill from chart title
    const chartTitle = lastSourceSpec?.title || lastSpec?.title || "";
    input.value = chartTitle;
    form.appendChild(input);

    const btnRow = document.createElement("div");
    btnRow.className = "dashboard-btn-row";

    const saveBtn = document.createElement("button");
    saveBtn.className = "dashboard-btn-primary";
    saveBtn.textContent = saving ? "Saving..." : "Create Dashboard";
    saveBtn.disabled = saving || !input.value.trim();
    saveBtn.addEventListener("click", () => handleCreate(input.value.trim()));
    btnRow.appendChild(saveBtn);
    form.appendChild(btnRow);

    input.addEventListener("input", () => {
      saveBtn.disabled = saving || !input.value.trim();
    });

    panel.appendChild(form);
  }

  function renderExistingTab() {
    const content = document.createElement("div");
    content.className = "dashboard-form";

    if (loadingDashboards) {
      const spinner = document.createElement("div");
      spinner.className = "dashboard-loading";
      spinner.textContent = "Loading dashboards...";
      content.appendChild(spinner);
      panel.appendChild(content);
      return;
    }

    if (!dashboards || dashboards.length === 0) {
      const empty = document.createElement("div");
      empty.className = "dashboard-empty";
      empty.textContent = "No dashboards yet. Create one using the \"Create New\" tab.";
      content.appendChild(empty);
      panel.appendChild(content);
      return;
    }

    const list = document.createElement("div");
    list.className = "dashboard-list";
    for (const dash of dashboards) {
      const item = document.createElement("button");
      item.className = "dashboard-item" + (selectedDashboardId === dash.dashboard_id ? " selected" : "");
      const title = document.createElement("span");
      title.className = "dashboard-item-title";
      title.textContent = dash.title;
      item.appendChild(title);

      if (dash.updated_at) {
        const date = document.createElement("span");
        date.className = "dashboard-item-date";
        date.textContent = formatRelativeDate(dash.updated_at);
        item.appendChild(date);
      }

      item.addEventListener("click", () => {
        selectedDashboardId = dash.dashboard_id;
        render();
      });
      list.appendChild(item);
    }
    content.appendChild(list);

    const btnRow = document.createElement("div");
    btnRow.className = "dashboard-btn-row";

    const addBtn = document.createElement("button");
    addBtn.className = "dashboard-btn-primary";
    addBtn.textContent = saving ? "Saving..." : "Add to Dashboard";
    addBtn.disabled = saving || !selectedDashboardId;
    addBtn.addEventListener("click", () => handleAddToExisting(selectedDashboardId));
    btnRow.appendChild(addBtn);
    content.appendChild(btnRow);

    panel.appendChild(content);
  }

  async function loadDashboards() {
    loadingDashboards = true;
    render();
    try {
      const data = await callMcpTool("search_dashboards", { sort_by: "recent", limit: 20 });
      dashboards = data.dashboards || [];
    } catch (e) {
      showStatus("Failed to load dashboards: " + e.message, true);
      dashboards = [];
    }
    loadingDashboards = false;
    render();
  }

  async function handleCreate(title) {
    saving = true;
    render();
    try {
      const chartBlock = getChartMarkdownBlock();
      if (!chartBlock) throw new Error("No chart data available");

      const data = await callMcpTool("create_dashboard", {
        title,
        content: chartBlock,
        verified_no_duplicates: true,
      });

      if (data.error) throw new Error(data.error);
      showSuccess("Dashboard created!", data.url);
    } catch (e) {
      showStatus("Failed to create dashboard: " + e.message, true);
      saving = false;
      render();
    }
  }

  async function handleAddToExisting(dashboardId) {
    saving = true;
    render();
    try {
      const chartBlock = getChartMarkdownBlock();
      if (!chartBlock) throw new Error("No chart data available");

      // Get existing content
      const info = await callMcpTool("get_dashboard_info", { dashboard_id: dashboardId });
      if (info.error) throw new Error(info.error);

      const existingContent = info.content || "";
      const newContent = existingContent
        ? existingContent + "\n\n" + chartBlock
        : chartBlock;

      const data = await callMcpTool("modify_dashboard", {
        dashboard_id: dashboardId,
        content: newContent,
        change_summary: "Added chart from MCP",
      });

      if (data.error) throw new Error(data.error);
      showSuccess("Chart added to dashboard!", data.url);
    } catch (e) {
      showStatus("Failed to add to dashboard: " + e.message, true);
      saving = false;
      render();
    }
  }

  function showStatus(message, isError) {
    // Remove any existing status
    const existing = panel.querySelector(".dashboard-status");
    if (existing) existing.remove();

    const status = document.createElement("div");
    status.className = "dashboard-status" + (isError ? " error" : "");
    status.textContent = message;
    panel.appendChild(status);
  }

  function showSuccess(message, url) {
    panel.innerHTML = "";

    const success = document.createElement("div");
    success.className = "dashboard-success";

    const msg = document.createElement("div");
    msg.className = "dashboard-success-message";
    msg.textContent = message;
    success.appendChild(msg);

    if (url) {
      const link = document.createElement("button");
      link.className = "dashboard-btn-primary";
      link.textContent = "Open Dashboard";
      link.addEventListener("click", () => {
        app.openLink({ url });
      });
      success.appendChild(link);
    }

    panel.appendChild(success);
  }

  function formatRelativeDate(isoStr) {
    const date = new Date(isoStr);
    const now = new Date();
    const diffMs = now - date;
    const diffDays = Math.floor(diffMs / 86400000);
    if (diffDays === 0) return "today";
    if (diffDays === 1) return "yesterday";
    if (diffDays < 30) return diffDays + "d ago";
    const diffMonths = Math.floor(diffDays / 30);
    if (diffMonths < 12) return diffMonths + "mo ago";
    return Math.floor(diffDays / 365) + "y ago";
  }

  render();
  return panel;
}

async function renderChart(spec, palette) {
  const chartml = new ChartML({
    defaultPalette: palette || null,
  });

  container.innerHTML = "";

  // Add chart header bar
  const headerBar = document.createElement("chart-header-bar");
  headerBar.setAttribute("last-updated", String(Date.now()));

  // Add Kyomi logo link in the "before" slot (far left of header)
  const logoLink = document.createElement("button");
  logoLink.slot = "before";
  logoLink.className = "kyomi-logo-link";
  logoLink.setAttribute("aria-label", "Open Kyomi");
  logoLink.innerHTML = `<svg width="18" height="18" viewBox="0 0 50 50" xmlns="http://www.w3.org/2000/svg"><g transform="translate(25, 25)"><g fill="currentColor"><polygon points="0,-22 3.5,-9 0,-5.5 -3.5,-9"/><polygon points="15.5,-15.5 9,-3.5 5.5,-5.5 9,-9"/><polygon points="22,0 9,3.5 5.5,0 9,-3.5"/><polygon points="15.5,15.5 3.5,9 0,5.5 3.5,9"/><polygon points="0,22 -3.5,9 0,5.5 3.5,9"/><polygon points="-15.5,15.5 -9,3.5 -5.5,5.5 -9,9"/><polygon points="-22,0 -9,-3.5 -5.5,0 -9,3.5"/><polygon points="-15.5,-15.5 -3.5,-9 0,-5.5 -3.5,-9"/></g><circle cx="0" cy="0" r="4.5" fill="currentColor"/></g></svg>`;
  logoLink.addEventListener("click", () => {
    app.openLink({ url: "https://kyomi.ai" });
  });
  headerBar.appendChild(logoLink);

  // Enable type selector for switchable chart types
  const chartType = spec?.visualize?.type;
  if (chartType && SWITCHABLE_TYPES.has(chartType)) {
    headerBar.setAttribute("chart-type", chartType);
    if (spec.visualize.orientation) {
      headerBar.setAttribute("chart-orientation", spec.visualize.orientation);
    }
    if (spec.visualize.mode) {
      headerBar.setAttribute("chart-mode", spec.visualize.mode);
    }
    headerBar.setAttribute("show-type-selector", "");
  }

  // "Continue in Kyomi" button — opens the chart in a new Kyomi conversation
  if (chartContextId && appUrl) {
    headerBar.setAttribute("show-ask-about", "");
    headerBar.addEventListener("header-ask-about", () => {
      app.openLink({ url: `${appUrl}/chat?chart=${chartContextId}` });
    });
  }

  // Info panel toggle
  headerBar.setAttribute("show-info", "");
  headerBar.addEventListener("header-info", () => {
    infoPanelOpen = !infoPanelOpen;
    dashboardPanelOpen = false;
    renderChart(lastSpec, lastPalette);
  });

  // Save to dashboard panel toggle
  headerBar.setAttribute("show-save-to-dashboard", "");
  headerBar.addEventListener("header-save-to-dashboard", () => {
    dashboardPanelOpen = !dashboardPanelOpen;
    infoPanelOpen = false;
    renderChart(lastSpec, lastPalette);
  });

  headerBar.addEventListener("header-type-change", (e) => {
    const newSpec = JSON.parse(JSON.stringify(lastSpec));
    const previousType = newSpec.visualize.type;
    const newType = e.detail.type;
    newSpec.visualize.type = newType;
    // Clean up incompatible properties when switching types
    if (newType !== "bar") {
      delete newSpec.visualize.orientation;
    }
    if (newType !== "bar" && newType !== "area") {
      delete newSpec.visualize.mode;
    }
    // Convert visualize structure when crossing type categories (chart/table/metric)
    convertVisualizeForTypeChange(newSpec.visualize, previousType, newType);
    lastSpec = newSpec;
    renderChart(newSpec, lastPalette);
  });

  headerBar.addEventListener("header-orientation-change", (e) => {
    const newSpec = JSON.parse(JSON.stringify(lastSpec));
    if (e.detail.orientation) {
      newSpec.visualize.orientation = e.detail.orientation;
    } else {
      delete newSpec.visualize.orientation;
    }
    lastSpec = newSpec;
    renderChart(newSpec, lastPalette);
  });

  headerBar.addEventListener("header-mode-change", (e) => {
    const newSpec = JSON.parse(JSON.stringify(lastSpec));
    if (e.detail.mode) {
      newSpec.visualize.mode = e.detail.mode;
    } else {
      delete newSpec.visualize.mode;
    }
    lastSpec = newSpec;
    renderChart(newSpec, lastPalette);
  });

  container.appendChild(headerBar);

  // Show info panel if toggled open (uses original spec with datasource + query)
  if (infoPanelOpen && lastSourceSpec) {
    container.appendChild(createInfoPanel(lastSourceSpec));
  }

  // Show dashboard panel if toggled open
  if (dashboardPanelOpen) {
    container.appendChild(createDashboardPanel());
  }

  // Render chart into a wrapper below the header
  const chartWrapper = document.createElement("div");
  container.appendChild(chartWrapper);

  await chartml.render(spec, chartWrapper);
}

// 2. Register handlers BEFORE connecting
app.ontoolresult = async (result) => {
  try {
    // If tool errored, show the error text from content
    if (result.isError) {
      const errorText = result.content?.map(c => c.text).join("\n") || "Unknown error";
      throw new Error(errorText);
    }

    const data = result.structuredContent || {};
    const { spec, palette } = data;

    if (!spec) {
      throw new Error("No chart specification in structuredContent");
    }

    lastSpec = spec;
    lastSourceSpec = data.sourceSpec || spec;
    lastPalette = palette;
    chartContextId = data.chartContextId || null;
    appUrl = data.appUrl || null;
    infoPanelOpen = false;
    dashboardPanelOpen = false;

    await renderChart(spec, palette);
  } catch (error) {
    container.innerHTML = `<div style="color: #dc2626; padding: 20px; text-align: center; background: #fef2f2; border: 1px solid #fecaca; border-radius: 8px; margin: 20px;">Chart rendering failed: ${error.message}</div>`;
    console.error("ChartML render error:", error);
  }
};

app.onerror = console.error;

// 3. Connect to host, then apply initial theme and host styles
app.connect().then(() => {
  const ctx = app.getHostContext();
  if (ctx) applyHostContext(ctx);
});
