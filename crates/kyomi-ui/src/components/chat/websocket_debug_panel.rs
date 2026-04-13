// SPDX-License-Identifier: AGPL-3.0-or-later

//! Floating dev panel for the shared WebSocket context.
//!
//! Visible when `localStorage.ws_debug === "1"`. Reads the `WsDiagnostics`
//! signal from `WebSocketContext` and renders a compact, always-on-top
//! summary of connection state, reconnect count, subscriber map, and the
//! rolling log of recent close events.
//!
//! Exists to replace "click around for 5 minutes and guess" with real
//! observability — specifically the WebSocket close code distribution,
//! which is the one data point that tells us whether instability is
//! network (1006), server (1011/4xxx), or our own Effect churning
//! (1000 with fast cycles).
//!
//! Enable in the browser console:
//!
//! ```js
//! localStorage.setItem('ws_debug', '1'); location.reload();
//! ```
//!
//! Disable:
//!
//! ```js
//! localStorage.removeItem('ws_debug'); location.reload();
//! ```

use leptos::prelude::*;

use super::websocket_client::{ConnectionState, WebSocketContext};

/// Read the `ws_debug` flag from localStorage at mount time.
///
/// Returns `false` on SSR (no window) or when the key is absent / not "1".
#[cfg(target_arch = "wasm32")]
fn debug_enabled() -> bool {
    web_sys::window()
        .and_then(|w| w.local_storage().ok().flatten())
        .and_then(|s| s.get_item("ws_debug").ok().flatten())
        .map(|v| v == "1")
        .unwrap_or(false)
}

#[cfg(not(target_arch = "wasm32"))]
fn debug_enabled() -> bool {
    false
}

/// Format a Unix ms timestamp as `HH:MM:SS` in local time.
///
/// WASM-only helper; on SSR the panel never renders so this isn't needed.
#[cfg(target_arch = "wasm32")]
fn format_ts(ms: f64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms));
    let h = date.get_hours();
    let m = date.get_minutes();
    let s = date.get_seconds();
    format!("{h:02}:{m:02}:{s:02}")
}

/// Human-readable label for common WebSocket close codes.
fn close_code_label(code: u16) -> &'static str {
    match code {
        1000 => "normal",
        1001 => "going away",
        1002 => "protocol error",
        1003 => "unsupported data",
        1005 => "no status",
        1006 => "abnormal",
        1007 => "invalid payload",
        1008 => "policy violation",
        1009 => "message too big",
        1010 => "extension required",
        1011 => "server error",
        1012 => "service restart",
        1013 => "try again later",
        1014 => "bad gateway",
        1015 => "tls failure",
        4000..=4999 => "application",
        _ => "other",
    }
}

/// Floating dev-only WebSocket diagnostics panel.
///
/// Mount this once inside `Layout` under `WebSocketProvider`. It self-hides
/// when `localStorage.ws_debug` is not set to `"1"`, so it's safe to leave
/// mounted in all builds — no prod visibility, no code-path impact.
#[component]
pub fn WebSocketDebugPanel() -> impl IntoView {
    let enabled = debug_enabled();
    if !enabled {
        return ().into_any();
    }

    let ctx = match use_context::<WebSocketContext>() {
        Some(c) => c,
        None => return ().into_any(),
    };

    let connection_state = ctx.connection_state;
    let diagnostics = ctx.diagnostics;
    let (collapsed, set_collapsed) = signal(false);

    // State badge color by current connection state — keeps the panel
    // glanceable without reading text.
    let state_badge_class = move || {
        let base = "inline-block px-1.5 py-0.5 rounded text-[10px] font-semibold uppercase tracking-wide";
        match connection_state.get() {
            ConnectionState::Connected => format!("{base} bg-green-500/20 text-green-400"),
            ConnectionState::Connecting => format!("{base} bg-amber-500/20 text-amber-400"),
            ConnectionState::Reconnecting => format!("{base} bg-amber-500/20 text-amber-400"),
            ConnectionState::Disconnected => format!("{base} bg-red-500/20 text-red-400"),
        }
    };

    view! {
        <div
            class="fixed bottom-2 right-2 z-[9999] font-mono text-[11px] text-slate-200 bg-slate-900/95 border border-slate-700 rounded-md shadow-xl backdrop-blur-sm"
            style="max-width: 22rem; min-width: 14rem;"
        >
            // Header — state badge + collapse toggle.
            <button
                type="button"
                class="w-full flex items-center justify-between gap-2 px-2 py-1 border-b border-slate-700 hover:bg-slate-800/50"
                on:click=move |_| set_collapsed.update(|c| *c = !*c)
            >
                <div class="flex items-center gap-2">
                    <span class="text-slate-400">"WS"</span>
                    <span class=state_badge_class>
                        {move || format!("{}", connection_state.get())}
                    </span>
                </div>
                <span class="text-slate-500">
                    {move || if collapsed.get() { "▶" } else { "▼" }}
                </span>
            </button>

            <Show when=move || !collapsed.get()>
                <div class="p-2 space-y-2">
                    // ── Counters row ─────────────────────────────────────
                    {move || {
                        let d = diagnostics.get();
                        view! {
                            <div class="grid grid-cols-3 gap-2 text-center">
                                <div>
                                    <div class="text-slate-500 text-[10px]">"opens"</div>
                                    <div class="text-slate-200 font-semibold">{d.connect_count}</div>
                                </div>
                                <div>
                                    <div class="text-slate-500 text-[10px]">"attempts"</div>
                                    <div class="text-slate-200 font-semibold">{d.reconnect_attempts}</div>
                                </div>
                                <div>
                                    <div class="text-slate-500 text-[10px]">"subs"</div>
                                    <div class="text-slate-200 font-semibold">{d.total_subscribers}</div>
                                </div>
                            </div>
                        }
                    }}

                    // ── Subscribers by type ──────────────────────────────
                    {move || {
                        let d = diagnostics.get();
                        if d.subscriber_counts.is_empty() {
                            view! {
                                <div class="text-slate-500 italic">"no subscribers"</div>
                            }.into_any()
                        } else {
                            let rows = d.subscriber_counts
                                .iter()
                                .map(|(k, v)| {
                                    let k = k.clone();
                                    let v = *v;
                                    view! {
                                        <div class="flex justify-between gap-2 py-px">
                                            <span class="text-slate-400 truncate">{k}</span>
                                            <span class="text-slate-200 tabular-nums">{v}</span>
                                        </div>
                                    }
                                })
                                .collect_view();
                            view! {
                                <div class="border-t border-slate-700 pt-1">
                                    <div class="text-slate-500 text-[10px] mb-1">"subscribers"</div>
                                    {rows}
                                </div>
                            }.into_any()
                        }
                    }}

                    // ── Recent close history ─────────────────────────────
                    {move || {
                        let d = diagnostics.get();
                        if d.close_history.is_empty() {
                            view! {
                                <div class="text-slate-500 italic border-t border-slate-700 pt-1">
                                    "no closes yet"
                                </div>
                            }.into_any()
                        } else {
                            let rows = d.close_history
                                .iter()
                                .rev()
                                .map(|c| {
                                    #[cfg(target_arch = "wasm32")]
                                    let ts = format_ts(c.ts_ms);
                                    #[cfg(not(target_arch = "wasm32"))]
                                    let ts = String::from("--:--:--");
                                    let code = c.code;
                                    let label = close_code_label(code);
                                    let reason = c.reason.clone();
                                    let reason_title = reason.clone();
                                    let clean = c.was_clean;
                                    // Colour 1000 clean green, 1006 and 4xxx red, others amber.
                                    let color = match code {
                                        1000 if clean => "text-green-400",
                                        1006 => "text-red-400",
                                        4000..=4999 => "text-red-400",
                                        _ => "text-amber-400",
                                    };
                                    view! {
                                        <div class="flex justify-between gap-2 py-px">
                                            <span class="text-slate-500 tabular-nums">{ts}</span>
                                            <span class=format!("{color} tabular-nums font-semibold")>
                                                {code}
                                            </span>
                                            <span class="text-slate-400 flex-1 truncate text-right" title=reason_title>
                                                {if reason.is_empty() { label.to_string() } else { reason }}
                                            </span>
                                        </div>
                                    }
                                })
                                .collect_view();
                            view! {
                                <div class="border-t border-slate-700 pt-1">
                                    <div class="text-slate-500 text-[10px] mb-1">
                                        "recent closes (newest first)"
                                    </div>
                                    {rows}
                                </div>
                            }.into_any()
                        }
                    }}
                </div>
            </Show>
        </div>
    }
    .into_any()
}
