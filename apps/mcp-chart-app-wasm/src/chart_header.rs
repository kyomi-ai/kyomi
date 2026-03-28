// SPDX-License-Identifier: AGPL-3.0-or-later

//! Leptos wrapper for the `<chart-header-bar>` web component.
//!
//! The web component is bundled via the JS bridge (from `@kyomi/chart-header`).
//! This wrapper creates the element via web-sys, sets attributes reactively,
//! and listens for custom events (composed: true, so they cross shadow DOM).

use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use crate::mcp_interop;

/// Kyomi logo SVG for the "before" slot.
const KYOMI_LOGO_SVG: &str = r#"<svg width="18" height="18" viewBox="0 0 50 50" xmlns="http://www.w3.org/2000/svg"><g transform="translate(25, 25)"><g fill="currentColor"><polygon points="0,-22 3.5,-9 0,-5.5 -3.5,-9"/><polygon points="15.5,-15.5 9,-3.5 5.5,-5.5 9,-9"/><polygon points="22,0 9,3.5 5.5,0 9,-3.5"/><polygon points="15.5,15.5 3.5,9 0,5.5 3.5,9"/><polygon points="0,22 -3.5,9 0,5.5 3.5,9"/><polygon points="-15.5,15.5 -9,3.5 -5.5,5.5 -9,9"/><polygon points="-22,0 -9,-3.5 -5.5,0 -9,3.5"/><polygon points="-15.5,-15.5 -3.5,-9 0,-5.5 -3.5,-9"/></g><circle cx="0" cy="0" r="4.5" fill="currentColor"/></g></svg>"#;

#[component]
pub fn ChartHeaderBar(
    chart_type: Option<String>,
    chart_orientation: Option<String>,
    chart_mode: Option<String>,
    show_type_selector: bool,
    show_info: bool,
    show_save_to_dashboard: bool,
    show_ask_about: bool,
    on_type_change: impl Fn(String) + 'static + Clone + Send,
    on_orientation_change: impl Fn(Option<String>) + 'static + Clone + Send,
    on_mode_change: impl Fn(Option<String>) + 'static + Clone + Send,
    on_info: impl Fn(()) + 'static + Clone + Send,
    on_save_to_dashboard: impl Fn(()) + 'static + Clone + Send,
    on_ask_about: impl Fn(()) + 'static + Clone + Send,
) -> impl IntoView {
    let container_ref = NodeRef::<leptos::html::Div>::new();

    Effect::new(move || {
        let Some(container) = container_ref.get() else { return };
        let document = leptos::prelude::document();

        // Create the web component
        let header = document
            .create_element("chart-header-bar")
            .expect("failed to create chart-header-bar");

        // Set timestamp
        let now = js_sys::Date::now().to_string();
        let _ = header.set_attribute("last-updated", &now);

        // Chart type attributes
        if let Some(ref ct) = chart_type {
            let _ = header.set_attribute("chart-type", ct);
        }
        if let Some(ref co) = chart_orientation {
            let _ = header.set_attribute("chart-orientation", co);
        }
        if let Some(ref cm) = chart_mode {
            let _ = header.set_attribute("chart-mode", cm);
        }
        if show_type_selector {
            let _ = header.set_attribute("show-type-selector", "");
        }
        if show_info {
            let _ = header.set_attribute("show-info", "");
        }
        if show_save_to_dashboard {
            let _ = header.set_attribute("show-save-to-dashboard", "");
        }
        if show_ask_about {
            let _ = header.set_attribute("show-ask-about", "");
        }

        // Kyomi logo in the "before" slot
        let logo = document.create_element("button").unwrap();
        logo.set_attribute("slot", "before").unwrap();
        logo.set_attribute("class", "kyomi-logo-link").unwrap();
        logo.set_attribute("aria-label", "Open Kyomi").unwrap();
        logo.set_inner_html(KYOMI_LOGO_SVG);

        let logo_cb = Closure::<dyn Fn()>::new(move || {
            mcp_interop::open_link("https://kyomi.ai");
        });
        logo.add_event_listener_with_callback("click", logo_cb.as_ref().unchecked_ref()).unwrap();
        logo_cb.forget();
        header.append_child(&logo).unwrap();

        // Event listeners
        let on_type = on_type_change.clone();
        let type_cb = Closure::<dyn Fn(web_sys::CustomEvent)>::new(move |e: web_sys::CustomEvent| {
            if let Some(detail) = e.detail().dyn_ref::<js_sys::Object>() {
                if let Ok(t) = js_sys::Reflect::get(detail, &"type".into()) {
                    if let Some(s) = t.as_string() {
                        on_type(s);
                    }
                }
            }
        });
        header.add_event_listener_with_callback(
            "header-type-change",
            type_cb.as_ref().unchecked_ref(),
        ).unwrap();
        type_cb.forget();

        let on_orient = on_orientation_change.clone();
        let orient_cb = Closure::<dyn Fn(web_sys::CustomEvent)>::new(move |e: web_sys::CustomEvent| {
            if let Some(detail) = e.detail().dyn_ref::<js_sys::Object>() {
                if let Ok(o) = js_sys::Reflect::get(detail, &"orientation".into()) {
                    on_orient(o.as_string());
                }
            }
        });
        header.add_event_listener_with_callback(
            "header-orientation-change",
            orient_cb.as_ref().unchecked_ref(),
        ).unwrap();
        orient_cb.forget();

        let on_m = on_mode_change.clone();
        let mode_cb = Closure::<dyn Fn(web_sys::CustomEvent)>::new(move |e: web_sys::CustomEvent| {
            if let Some(detail) = e.detail().dyn_ref::<js_sys::Object>() {
                if let Ok(m) = js_sys::Reflect::get(detail, &"mode".into()) {
                    on_m(m.as_string());
                }
            }
        });
        header.add_event_listener_with_callback(
            "header-mode-change",
            mode_cb.as_ref().unchecked_ref(),
        ).unwrap();
        mode_cb.forget();

        let on_i = on_info.clone();
        let info_cb = Closure::<dyn Fn()>::new(move || { on_i(()); });
        header.add_event_listener_with_callback(
            "header-info",
            info_cb.as_ref().unchecked_ref(),
        ).unwrap();
        info_cb.forget();

        let on_d = on_save_to_dashboard.clone();
        let dash_cb = Closure::<dyn Fn()>::new(move || { on_d(()); });
        header.add_event_listener_with_callback(
            "header-save-to-dashboard",
            dash_cb.as_ref().unchecked_ref(),
        ).unwrap();
        dash_cb.forget();

        let on_a = on_ask_about.clone();
        let ask_cb = Closure::<dyn Fn()>::new(move || { on_a(()); });
        header.add_event_listener_with_callback(
            "header-ask-about",
            ask_cb.as_ref().unchecked_ref(),
        ).unwrap();
        ask_cb.forget();

        // Clear container and insert the header
        container.set_inner_html("");
        container.append_child(&header).unwrap();
    });

    view! { <div node_ref=container_ref></div> }
}
