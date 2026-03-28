// SPDX-License-Identifier: AGPL-3.0-or-later

// MCP Apps SDK bridge — thin JS layer that handles protocol communication
// and forwards data to/from the WASM module.
//
// This file is bundled with esbuild (along with @modelcontextprotocol/ext-apps
// and @kyomi/chart-header) into a single bridge-bundle.js.

import { App, applyDocumentTheme } from "@modelcontextprotocol/ext-apps";
import "@kyomi/chart-header"; // registers <chart-header-bar> custom element

const app = new App({ name: "Kyomi Chart Viewer", version: "2.0.0" });

// Apply theme from host context
function applyHostContext(ctx) {
  if (ctx.theme) {
    applyDocumentTheme(ctx.theme);
  }
}

// -- Expose bridge functions for WASM to call --
window.__mcp = {
  callServerTool: async (name, argsJson) => {
    const args = JSON.parse(argsJson);
    const result = await app.callServerTool({ name, arguments: args });
    if (result.isError) {
      const errorText = result.content?.map(c => c.text).join("\n") || "Unknown error";
      throw new Error(errorText);
    }
    const text = result.content?.[0]?.text;
    if (!text) throw new Error("Empty response from server");
    return text; // Return raw JSON string — WASM parses it
  },

  openLink: (url) => {
    app.openLink({ url });
  },
};

// -- Wait for WASM to initialize, then connect --
// Trunk generates an init() function in the JS glue. We wait for it before
// connecting to the MCP host, ensuring WASM exports are available.
async function start() {
  // WASM init happens via Trunk's auto-generated script loader.
  // The exports (on_tool_result, on_host_context_changed) are registered
  // on the wasm-bindgen generated module and available globally after init.

  // Connect to MCP host
  await app.connect();

  // Apply initial theme
  const ctx = app.getHostContext();
  if (ctx) applyHostContext(ctx);
}

// -- MCP event handlers --
app.ontoolresult = (result) => {
  try {
    if (result.isError) {
      const errorText = result.content?.map(c => c.text).join("\n") || "Unknown error";
      throw new Error(errorText);
    }
    const data = result.structuredContent || {};
    // Pass entire structured content to WASM as JSON
    if (typeof window.on_tool_result === "function") {
      window.on_tool_result(JSON.stringify(data));
    }
  } catch (error) {
    console.error("Tool result error:", error);
    if (typeof window.on_tool_result === "function") {
      window.on_tool_result(JSON.stringify({ error: error.message }));
    }
  }
};

app.onhostcontextchanged = (params) => {
  applyHostContext(params);
  if (typeof window.on_host_context_changed === "function") {
    window.on_host_context_changed(JSON.stringify(params));
  }
};

app.onerror = console.error;

start().catch(console.error);
