// SPDX-License-Identifier: AGPL-3.0-or-later

//! ChartML extension for the kode WYSIWYG editor.
//!
//! Renders `chartml` fenced code blocks as live interactive charts with
//! tooltips and animations — identical to the dashboard viewer. The kode
//! tree editor uses `<For>` keyed rendering, so chart components persist
//! across editor re-renders as long as their content doesn't change.

use chartml_leptos::ChartMLChart;
use kode_leptos::extension::Extension;
use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;

use crate::chartml_provider::configured_chartml;
use crate::components::dashboard::chart_header_bar::ChartHeaderBar;
use crate::components::dashboard::markdown_renderer::{
    apply_spec_overrides, extract_chart_mode, extract_chart_orientation, extract_chart_type,
    split_chartml_block,
};

// Valid colSpan values for the 12-column grid — these snap points look correct
// across common container widths. Odd values like 1, 5, 7, 11 are excluded
// because they produce awkward proportions in typical 2+ chart layouts.
const VALID_COL_SPANS: &[u8] = &[2, 3, 4, 6, 8, 9, 10, 12];

/// Minimum resize height in pixels — prevents charts collapsing too small.
const MIN_CHART_HEIGHT_PX: f64 = 100.0;

/// Height snap interval — rounds to nearest 5px increment during drag.
const HEIGHT_SNAP_PX: f64 = 5.0;

/// Kode extension that renders `chartml` code blocks as live charts.
///
/// Stores the palette name and a reactive `is_dark` memo rather than a
/// pre-built `ChartMLRef`. Each rendered chart block creates its own
/// `ChartMLRef` inside a reactive closure, so charts re-mount with the
/// correct palette when the system theme changes.
pub struct ChartMLExtension {
    palette: String,
    is_dark: send_wrapper::SendWrapper<Memo<bool>>,
}

impl ChartMLExtension {
    /// Create the extension with the named palette and a reactive dark-mode memo.
    ///
    /// Charts are rendered lazily inside reactive closures that call
    /// [`configured_chartml`] — the shared factory that registers all 9 Kyomi
    /// renderers, the DataFusion transform, the palette, the Kyomi editorial
    /// theme, and tracing-based resolver hooks.
    ///
    /// # Arguments
    ///
    /// * `palette_name` — Kyomi palette name (e.g. `"kyomi"`).
    /// * `is_dark` — reactive memo that tracks whether the UI is in dark mode.
    pub fn new(palette_name: &str, is_dark: Memo<bool>) -> Self {
        Self {
            palette: palette_name.to_string(),
            is_dark: send_wrapper::SendWrapper::new(is_dark),
        }
    }
}

impl Extension for ChartMLExtension {
    fn name(&self) -> &str {
        "chartml"
    }

    fn code_block_languages(&self) -> &[&str] {
        &["chartml"]
    }

    fn block_col_span(&self, content: &str) -> Option<u8> {
        // Parse the YAML to extract layout.colSpan.
        // For YAML sequences (Array), use the first item's colSpan.
        // Returns None for blocks without an explicit colSpan — kode treats
        // None as "break the grid group and render full-width".
        let parsed: serde_json::Value = serde_yaml::from_str(content.trim()).ok()?;
        let spec = match &parsed {
            serde_json::Value::Array(items) => items.first()?,
            other => other,
        };
        let col_span = spec
            .get("layout")
            .and_then(|l| l.get("colSpan").or_else(|| l.get("col_span")))
            .and_then(|v| v.as_u64())?;
        if (1..=12).contains(&col_span) {
            Some(col_span as u8)
        } else {
            None
        }
    }

    fn render_code_block(
        &self,
        language: &str,
        content: &str,
        _block_start: usize,
        _block_end: usize,
    ) -> Option<AnyView> {
        if language != "chartml" {
            return None;
        }

        // A chartml block may hold a single YAML mapping (one chart) or a
        // YAML sequence with `- type: chart` items (N charts). Split here so
        // the WYSIWYG editor mirrors the viewer: every item becomes its own
        // rendered chart with its own header/edit-chrome, rather than only
        // the first item being visible. See KYO-107.
        let yamls = split_chartml_block(content.trim());
        if yamls.is_empty() {
            return None;
        }

        let palette = self.palette.clone();
        let is_dark = *self.is_dark;

        // The full block content is used by the edit-request listener in the
        // dashboard editor to locate which fence in the source was clicked
        // (each block's content is used as a fingerprint to find its index).
        let full_block_content = content.to_string();

        let views: Vec<AnyView> = yamls
            .into_iter()
            .enumerate()
            .map(|(array_index, item_yaml)| {
                let palette_clone = palette.clone();
                let block_content = full_block_content.clone();
                let chart_view = move || {
                    let chartml = configured_chartml(&palette_clone, is_dark.get());
                    render_one_chart(
                        item_yaml.clone(),
                        array_index,
                        block_content.clone(),
                        chartml,
                    )
                };
                chart_view().into_any()
            })
            .collect();

        Some(
            view! {
                // not-prose prevents Tailwind typography styles from interfering
                // with chart content. The grid layout is handled natively by kode
                // via block_col_span — no inner grid wrapper needed here.
                <div class="not-prose">
                    {views}
                </div>
            }
            .into_any(),
        )
    }
}

/// Render a single chart view for the WYSIWYG editor. Owns its own
/// override/refresh signals so per-chart header controls (type/orientation/mode
/// selectors, refresh button) act independently from sibling charts in the
/// same fenced block.
///
/// - `item_yaml` — the YAML for THIS chart (already split out of a sequence
///   block by [`split_chartml_block`] if applicable).
/// - `array_index` — index of this item within its block (0 for mappings).
/// - `block_content` — the full block's YAML (unsplit) used by the edit
///   listener to disambiguate which fence in the source was clicked.
/// - `chartml` — the configured ChartML renderer passed down from the
///   extension instance.
fn render_one_chart(
    item_yaml: String,
    array_index: usize,
    block_content: String,
    chartml: chartml_leptos::ChartMLRef,
) -> AnyView {
    let yaml = item_yaml;
    // Parse initial chart metadata from YAML
    let parsed_spec: Option<serde_json::Value> = serde_yaml::from_str(&yaml).ok();
    let initial_chart_type = parsed_spec.as_ref().and_then(extract_chart_type);
    let initial_orientation = parsed_spec.as_ref().and_then(extract_chart_orientation);
    let initial_mode = parsed_spec.as_ref().and_then(extract_chart_mode);

    // Override signals — same pattern as ChartBlock in markdown_renderer
    let (type_override, set_type_override) = signal(None::<String>);
    let (orientation_override, set_orientation_override) = signal(None::<Option<String>>);
    let (mode_override, set_mode_override) = signal(None::<Option<String>>);

    let initial_type_stored = StoredValue::new(initial_chart_type.clone());
    let initial_orient_stored = StoredValue::new(initial_orientation.clone());
    let initial_mode_stored = StoredValue::new(initial_mode.clone());

    // Derived current values for the header bar display
    let current_chart_type = Memo::new(move |_| {
        type_override
            .get()
            .or_else(|| initial_type_stored.get_value())
    });
    let current_orientation = Memo::new(move |_| match orientation_override.get() {
        Some(o) => o,
        None => initial_orient_stored.get_value(),
    });
    let current_mode = Memo::new(move |_| match mode_override.get() {
        Some(m) => m,
        None => initial_mode_stored.get_value(),
    });

    // Derive effective YAML spec with overrides applied
    let yaml_for_spec = yaml.clone();
    let effective_spec = Memo::new(move |_| {
        let t_ovr = type_override.get();
        let o_ovr = orientation_override.get();
        let m_ovr = mode_override.get();

        if t_ovr.is_none() && o_ovr.is_none() && m_ovr.is_none() {
            return yaml_for_spec.clone();
        }

        apply_spec_overrides(
            &yaml_for_spec,
            t_ovr.as_deref(),
            o_ovr.as_ref().map(|o| o.as_deref()),
            m_ovr.as_ref().map(|m| m.as_deref()),
        )
    });

    // Callbacks for the header bar
    let yaml_for_type_persist = yaml.clone();
    let block_content_for_type = block_content.clone();
    let on_type_change = Callback::new(move |t: String| {
        set_type_override.set(Some(t.clone()));
        dispatch_chart_type_change_event(&yaml_for_type_persist, &block_content_for_type, array_index, &t);
    });
    let on_orientation_change = Callback::new(move |o: Option<String>| {
        set_orientation_override.set(Some(o));
    });
    let on_mode_change = Callback::new(move |m: Option<String>| {
        set_mode_override.set(Some(m));
    });

    let yaml_for_info = yaml.clone();
    let on_info = Callback::new(move |()| {
        dispatch_chart_info_event(&yaml_for_info);
    });

    // Edit dispatch carries enough info for the listener to locate this
    // chart in the editor source:
    //   - `yaml` — the per-item yaml to populate the chart builder
    //   - `block_content` — the full block's content (fingerprint used to
    //     find `block_index` by matching against the source's ```chartml fences)
    //   - `array_index` — which item within the block was clicked
    let yaml_for_edit = yaml.clone();
    let block_content_for_edit = block_content.clone();
    let on_edit = Callback::new(move |()| {
        dispatch_chart_edit_event(&yaml_for_edit, &block_content_for_edit, array_index);
    });

    // Resize callbacks — stored values to avoid clone-into-closure issues
    let yaml_for_resize = yaml.clone();
    let block_content_for_resize = block_content.clone();
    let on_resize = Callback::new(
        move |(new_col_span, new_height): (Option<u8>, Option<f64>)| {
            dispatch_chart_resize_event(
                &yaml_for_resize,
                &block_content_for_resize,
                array_index,
                new_col_span,
                new_height,
            );
        },
    );
    let on_resize_stored = StoredValue::new(on_resize);

    // Drag cleanup slot: holds a teardown FnOnce that removes document-level
    // mousemove/mouseup listeners if the component unmounts mid-drag.
    // Pattern mirrors `right_panel.rs CleanupSlot`. Only populated during an
    // active drag; cleared by mouseup (normal end) or on_cleanup (navigate away).
    let drag_cleanup: StoredValue<
        Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>,
    > = StoredValue::new(None);

    on_cleanup(move || {
        if let Some(teardown) = drag_cleanup.try_update_value(|v| v.take()).flatten() {
            teardown.take()();
        }
    });

    // Last refreshed tracking — set_last_refreshed used in cfg(wasm32) blocks
    let (last_refreshed, set_last_refreshed) = signal(None::<f64>);
    let (is_refreshing, _set_is_refreshing) = signal(false);

    // Set initial timestamp and build refresh callback
    #[cfg(target_arch = "wasm32")]
    {
        set_last_refreshed.set(Some(js_sys::Date::now()));
    }
    let on_refresh = Callback::new(move |()| {
        // Inline charts don't fetch remote data — just update the timestamp
        set_last_refreshed.set(Some(
            #[cfg(target_arch = "wasm32")]
            {
                js_sys::Date::now()
            },
            #[cfg(not(target_arch = "wasm32"))]
            {
                0.0
            },
        ));
    });

    // Store callbacks for use inside the reactive header closure
    let on_type_stored = StoredValue::new(on_type_change);
    let on_orient_stored = StoredValue::new(on_orientation_change);
    let on_mode_stored = StoredValue::new(on_mode_change);
    let on_info_stored = StoredValue::new(on_info);
    let on_edit_stored = StoredValue::new(on_edit);
    let on_refresh_stored = StoredValue::new(on_refresh);

    view! {
        // Resize container — `position: relative` so the ghost overlay and
        // handles can be positioned relative to the chart block.
        <div class="chartml-resize-container">
            <div class="chart-card">
                // Reactive header bar — re-renders when type/orientation/mode change
                {move || {
                    let ct = current_chart_type.get();
                    let co = current_orientation.get();
                    let cm = current_mode.get();
                    let type_cb = on_type_stored.get_value();
                    let orient_cb = on_orient_stored.get_value();
                    let mode_cb = on_mode_stored.get_value();
                    let info_cb = on_info_stored.get_value();
                    let edit_cb = on_edit_stored.get_value();
                    let refresh_cb = on_refresh_stored.get_value();
                    let last_sig = Signal::derive(move || last_refreshed.get());
                    let refreshing_sig = Signal::derive(move || is_refreshing.get());
                    view! {
                        <ChartHeaderBar
                            chart_type=ct.unwrap_or_default()
                            chart_orientation=co.unwrap_or_default()
                            chart_mode=cm.unwrap_or_default()
                            show_type_selector=true
                            show_refresh=true
                            show_info=true
                            show_edit=true
                            on_type_change=type_cb
                            on_orientation_change=orient_cb
                            on_mode_change=mode_cb
                            on_info=info_cb
                            on_edit=edit_cb
                            on_refresh=refresh_cb
                            last_updated=last_sig
                            is_refreshing=refreshing_sig
                        />
                    }
                }}
                <ChartMLChart
                    spec=Signal::derive(move || effective_spec.get())
                    chartml=chartml
                />
            </div>
            // ── Resize handles ─────────────────────────────────────────────
            // Handles are rendered outside .chart-card so they don't affect
            // chart layout. They use position:absolute relative to the
            // .chartml-resize-container wrapper.
            //
            // Right handle: drag to change colSpan (width)
            <div
                class="chartml-resize-handle chartml-resize-handle-e"
                on:mousedown=move |ev: leptos::ev::MouseEvent| {
                    ev.prevent_default();
                    ev.stop_propagation();
                    let resize_cb = on_resize_stored.get_value();
                    start_resize(ev, ResizeAxis::Width, resize_cb, drag_cleanup);
                }
            />
            // Bottom handle: drag to change height
            <div
                class="chartml-resize-handle chartml-resize-handle-s"
                on:mousedown=move |ev: leptos::ev::MouseEvent| {
                    ev.prevent_default();
                    ev.stop_propagation();
                    let resize_cb = on_resize_stored.get_value();
                    start_resize(ev, ResizeAxis::Height, resize_cb, drag_cleanup);
                }
            />
            // Corner handle: drag to change both
            <div
                class="chartml-resize-handle chartml-resize-handle-se"
                on:mousedown=move |ev: leptos::ev::MouseEvent| {
                    ev.prevent_default();
                    ev.stop_propagation();
                    let resize_cb = on_resize_stored.get_value();
                    start_resize(ev, ResizeAxis::Both, resize_cb, drag_cleanup);
                }
            />
        </div>
    }
    .into_any()
}

/// Dispatch a `chart-info-request` CustomEvent carrying raw YAML in `detail`.
/// The dashboard editor's listener reads `ev.detail()` as a string and opens
/// the chart info modal. No structured payload is needed — info is read-only.
fn dispatch_chart_info_event(yaml: &str) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let detail = wasm_bindgen::JsValue::from_str(yaml);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("chart-info-request", &init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = yaml;
    }
}

/// Dispatch a `chart-type-change-request` CustomEvent so the dashboard editor
/// can persist the new chart type directly to the source YAML without opening
/// the chart builder modal.
///
/// Payload shape (JSON-stringified in `detail`):
/// ```json
/// { "yaml": "...", "block_content": "...", "array_index": 0, "new_type": "line" }
/// ```
/// - `yaml`: the per-item YAML (used to apply the type mutation)
/// - `block_content`: the full block's YAML (fingerprint for the listener to
///   find `block_index` by matching against ```chartml fences in the source)
/// - `array_index`: which item within the block was changed (0 for mapping blocks)
/// - `new_type`: the chart type string selected by the user
fn dispatch_chart_type_change_event(
    yaml: &str,
    block_content: &str,
    array_index: usize,
    new_type: &str,
) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let payload = serde_json::json!({
            "yaml": yaml,
            "block_content": block_content,
            "array_index": array_index,
            "new_type": new_type,
        });
        let json = payload.to_string();
        let detail = wasm_bindgen::JsValue::from_str(&json);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("chart-type-change-request", &init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (yaml, block_content, array_index, new_type);
    }
}

// ── Resize drag logic ─────────────────────────────────────────────────────────

/// Which dimension(s) a resize handle controls.
#[derive(Clone, Copy)]
enum ResizeAxis {
    Width,
    Height,
    Both,
}

/// Snap a pixel X offset to the nearest valid colSpan value.
///
/// `container_width` is the full width of the `.kode-block-grid` container
/// (12 columns). Each column is `container_width / 12` pixels wide.
fn snap_to_col_span(container_width: f64, px_x: f64) -> u8 {
    if container_width <= 0.0 {
        return 12;
    }
    let col_width = container_width / 12.0;
    let raw_cols = (px_x / col_width).round() as i64;
    let clamped = raw_cols.clamp(2, 12) as u8;
    // Snap to nearest valid colSpan
    VALID_COL_SPANS
        .iter()
        .copied()
        .min_by_key(|&v| (v as i64 - clamped as i64).unsigned_abs())
        .unwrap_or(12)
}

/// Snap a height value to the nearest `HEIGHT_SNAP_PX` increment, clamped to
/// `MIN_CHART_HEIGHT_PX`.
fn snap_height(px: f64) -> f64 {
    let snapped = (px / HEIGHT_SNAP_PX).round() * HEIGHT_SNAP_PX;
    snapped.max(MIN_CHART_HEIGHT_PX)
}

/// Start a resize drag from a mousedown event on a resize handle.
///
/// Installs document-level `mousemove` and `mouseup` listeners that:
/// - On `mousemove`: create/update a ghost overlay element showing the
///   snapped target size.
/// - On `mouseup`: remove the ghost, dispatch `chart-resize-request` if the
///   size changed, and remove the document listeners.
///
/// `drag_cleanup` is a slot that receives a teardown closure so the owning
/// component's `on_cleanup` can remove the listeners if the user navigates
/// away mid-drag. Follows the pattern established in `right_panel.rs`.
///
/// This function is a no-op on non-WASM targets (SSR).
fn start_resize(
    ev: leptos::ev::MouseEvent,
    axis: ResizeAxis,
    on_resize: Callback<(Option<u8>, Option<f64>)>,
    drag_cleanup: StoredValue<Option<send_wrapper::SendWrapper<Box<dyn FnOnce()>>>>,
) {
    #[cfg(target_arch = "wasm32")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;
        use wasm_bindgen::closure::Closure;
        use wasm_bindgen::JsCast;

        let window = match web_sys::window() {
            Some(w) => w,
            None => return,
        };

        // The resize handle's parent is .chartml-resize-container.
        let target_el: web_sys::Element = match ev
            .current_target()
            .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
        {
            Some(el) => el,
            None => return,
        };
        let container_el: web_sys::Element = match target_el.parent_element() {
            Some(el) => el,
            None => return,
        };

        // Find the .kode-block-grid ancestor to measure column width.
        // Walk up from container_el until we find it (or give up at 10 levels).
        let mut grid_el: Option<web_sys::Element> = container_el.parent_element();
        for _ in 0..10 {
            match grid_el.as_ref() {
                None => break,
                Some(el) => {
                    if el.class_list().contains("kode-block-grid") {
                        break;
                    }
                    grid_el = el.parent_element();
                }
            }
        }

        // Measure starting dimensions from the container element.
        let rect = container_el.get_bounding_client_rect();
        let start_x = ev.client_x() as f64;
        let start_y = ev.client_y() as f64;
        let start_w = rect.width();
        let start_h = rect.height();

        // Container width for column calculation.
        let grid_width = grid_el
            .as_ref()
            .map(|el| el.client_width() as f64)
            .unwrap_or(0.0);

        // Compute initial col span from the .kode-grid-item data attribute.
        // DOM hierarchy: .kode-grid-item > .kode-extension-block > .chartml-resize-container
        let initial_col_span: u8 = container_el
            .parent_element()           // .kode-extension-block
            .and_then(|el| el.parent_element()) // .kode-grid-item
            .and_then(|el| el.get_attribute("data-col-span"))
            .and_then(|s| s.parse::<u8>().ok())
            .unwrap_or(12);

        // Create ghost overlay element and append to container.
        let ghost: web_sys::HtmlElement = match window
            .document()
            .and_then(|doc| doc.create_element("div").ok())
            .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
        {
            Some(el) => el,
            None => return,
        };
        {
            let s = ghost.style();
            let _ = s.set_property("position", "absolute");
            let _ = s.set_property("top", "0");
            let _ = s.set_property("left", "0");
            let _ = s.set_property("width", &format!("{start_w}px"));
            let _ = s.set_property("height", &format!("{start_h}px"));
            let _ = s.set_property("pointer-events", "none");
            let _ = s.set_property("z-index", "100");
        }
        ghost.set_class_name("chartml-resize-ghost");
        let _ = container_el.append_child(&ghost);

        // Shared state between mousemove and mouseup via Rc<Cell>.
        let last_col_span = Rc::new(std::cell::Cell::new(initial_col_span));
        let last_height = Rc::new(std::cell::Cell::new(start_h));

        // Lock body cursor for the duration of the drag.
        if let Some(doc) = window.document() {
            if let Some(body) = doc.body() {
                let cursor = match axis {
                    ResizeAxis::Width => "ew-resize",
                    ResizeAxis::Height => "ns-resize",
                    ResizeAxis::Both => "nwse-resize",
                };
                let _ = body.style().set_property("cursor", cursor);
                let _ = body.style().set_property("user-select", "none");
            }
        }

        // ── Build both closures, store them together in an Rc so teardown ──
        // can remove either listener even if the other has already fired.
        // Pattern mirrors `right_panel.rs DragClosures`.
        type DragClosures = Rc<
            RefCell<
                Option<(
                    Closure<dyn FnMut(web_sys::MouseEvent)>,
                    Closure<dyn FnMut(web_sys::MouseEvent)>,
                )>,
            >,
        >;

        let closures: DragClosures = Rc::new(RefCell::new(None));

        let mousemove_cb = {
            let container_clone = container_el.clone();
            let ghost_clone = ghost.clone();
            let axis_move = axis;
            let last_col_clone = last_col_span.clone();
            let last_h_clone = last_height.clone();

            Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
                move |me: web_sys::MouseEvent| {
                    let dx = me.client_x() as f64 - start_x;
                    let dy = me.client_y() as f64 - start_y;

                    let new_col_span = match axis_move {
                        ResizeAxis::Width | ResizeAxis::Both => {
                            let new_w = (start_w + dx).max(40.0);
                            let new_cs = if grid_width > 0.0 {
                                snap_to_col_span(grid_width, new_w)
                            } else {
                                initial_col_span
                            };
                            last_col_clone.set(new_cs);
                            let col_w = if grid_width > 0.0 {
                                grid_width / 12.0
                            } else {
                                start_w / initial_col_span as f64
                            };
                            let ghost_w = new_cs as f64 * col_w;
                            let _ = ghost_clone
                                .style()
                                .set_property("width", &format!("{ghost_w}px"));
                            Some(new_cs)
                        }
                        ResizeAxis::Height => None,
                    };

                    let new_height = match axis_move {
                        ResizeAxis::Height | ResizeAxis::Both => {
                            let snapped = snap_height(start_h + dy);
                            last_h_clone.set(snapped);
                            let _ = ghost_clone
                                .style()
                                .set_property("height", &format!("{snapped}px"));
                            Some(snapped)
                        }
                        ResizeAxis::Width => None,
                    };

                    let label = match (new_col_span, new_height) {
                        (Some(cs), Some(h)) => format!("{cs}/12 · {h:.0}px"),
                        (Some(cs), None) => format!("{cs}/12"),
                        (None, Some(h)) => format!("{h:.0}px"),
                        (None, None) => String::new(),
                    };
                    ghost_clone.set_inner_text(&label);
                    let _ = container_clone.class_list().add_1("chartml-resizing");
                },
            )
        };

        let move_fn: js_sys::Function = mousemove_cb
            .as_ref()
            .unchecked_ref::<js_sys::Function>()
            .clone();

        // ── mouseup closure ──────────────────────────────────────────────
        let ghost_up = ghost.clone();
        let container_up = container_el.clone();
        let window_up = window.clone();
        let move_fn_for_up = move_fn.clone();
        let closures_for_up: DragClosures = closures.clone();

        let mouseup_cb = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(
            move |_: web_sys::MouseEvent| {
                // Remove both document listeners.
                let _ = window_up
                    .remove_event_listener_with_callback("mousemove", &move_fn_for_up);
                if let Some((_, ref up_cb)) = *closures_for_up.borrow() {
                    let _ = window_up.remove_event_listener_with_callback(
                        "mouseup",
                        up_cb.as_ref().unchecked_ref(),
                    );
                }
                // Remove ghost and resizing class.
                let _ = ghost_up.remove();
                let _ = container_up.class_list().remove_1("chartml-resizing");
                // Restore body cursor.
                if let Some(doc) = window_up.document() {
                    if let Some(body) = doc.body() {
                        let _ = body.style().remove_property("cursor");
                        let _ = body.style().remove_property("user-select");
                    }
                }
                // Drop closure storage (mirrors right_panel.rs teardown).
                closures_for_up.borrow_mut().take();
                drag_cleanup.set_value(None);
                // Dispatch resize event if dimensions changed.
                let final_col = last_col_span.get();
                let final_h = last_height.get();
                let new_cs = match axis {
                    ResizeAxis::Width | ResizeAxis::Both => {
                        if final_col != initial_col_span {
                            Some(final_col)
                        } else {
                            None
                        }
                    }
                    ResizeAxis::Height => None,
                };
                let new_h = match axis {
                    ResizeAxis::Height | ResizeAxis::Both => {
                        if (final_h - start_h).abs() > 1.0 {
                            Some(final_h)
                        } else {
                            None
                        }
                    }
                    ResizeAxis::Width => None,
                };
                if new_cs.is_some() || new_h.is_some() {
                    on_resize.run((new_cs, new_h));
                }
            },
        );

        let _ = window.add_event_listener_with_callback("mousemove", &move_fn);
        let _ = window.add_event_listener_with_callback(
            "mouseup",
            mouseup_cb.as_ref().unchecked_ref(),
        );

        // Store both closures so teardown can remove their listeners.
        *closures.borrow_mut() = Some((mousemove_cb, mouseup_cb));

        // Build teardown for on_cleanup (navigate-away-mid-drag safety).
        let move_fn_for_teardown = move_fn;
        let closures_for_teardown: DragClosures = closures;
        let window_for_teardown = window.clone();
        let teardown: Box<dyn FnOnce()> = Box::new(move || {
            if let Some((_, ref up_cb)) = *closures_for_teardown.borrow() {
                let _ = window_for_teardown
                    .remove_event_listener_with_callback("mousemove", &move_fn_for_teardown);
                let _ = window_for_teardown.remove_event_listener_with_callback(
                    "mouseup",
                    up_cb.as_ref().unchecked_ref(),
                );
            }
            closures_for_teardown.borrow_mut().take();
        });
        drag_cleanup.set_value(Some(send_wrapper::SendWrapper::new(teardown)));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (ev, axis, on_resize, drag_cleanup);
    }
}

/// Dispatch a `chart-resize-request` CustomEvent so the dashboard editor can
/// persist the new colSpan and/or height directly to the source YAML without
/// opening any modal.
///
/// Payload shape (JSON-stringified in `detail`):
/// ```json
/// { "block_content": "...", "array_index": 0, "new_col_span": 6, "new_height": 350 }
/// ```
/// - `block_content`: the full block's YAML (fingerprint for the listener to
///   find `block_index` by matching against ```chartml fences in the source)
/// - `array_index`: which item within the block was resized (0 for mapping blocks)
/// - `new_col_span`: optional new column span (1-12)
/// - `new_height`: optional new height in pixels
fn dispatch_chart_resize_event(
    yaml: &str,
    block_content: &str,
    array_index: usize,
    new_col_span: Option<u8>,
    new_height: Option<f64>,
) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let mut payload = serde_json::json!({
            "yaml": yaml,
            "block_content": block_content,
            "array_index": array_index,
        });
        if let Some(cs) = new_col_span {
            payload["new_col_span"] = serde_json::json!(cs);
        }
        if let Some(h) = new_height {
            payload["new_height"] = serde_json::json!(h);
        }
        let json = payload.to_string();
        let detail = wasm_bindgen::JsValue::from_str(&json);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("chart-resize-request", &init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (yaml, block_content, array_index, new_col_span, new_height);
    }
}

/// Dispatch a `chart-edit-request` CustomEvent carrying a structured JSON
/// payload with enough information for the dashboard editor's listener to
/// locate the exact chart in the source and open the chart builder with the
/// correct per-item YAML.
///
/// Payload shape (JSON-stringified in `detail`):
/// ```json
/// { "yaml": "...", "block_content": "...", "array_index": 0 }
/// ```
/// - `yaml`: the per-item YAML to populate the chart builder
/// - `block_content`: the full block's YAML (fingerprint for the listener to
///   find `block_index` by matching against ```chartml fences in the source)
/// - `array_index`: which item within the block was clicked (0 for mapping blocks)
fn dispatch_chart_edit_event(yaml: &str, block_content: &str, array_index: usize) {
    #[cfg(target_arch = "wasm32")]
    if let Some(window) = web_sys::window() {
        let payload = serde_json::json!({
            "yaml": yaml,
            "block_content": block_content,
            "array_index": array_index,
        });
        let json = payload.to_string();
        let detail = wasm_bindgen::JsValue::from_str(&json);
        let init = web_sys::CustomEventInit::new();
        init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("chart-edit-request", &init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = (yaml, block_content, array_index);
    }
}

// ── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── snap_to_col_span ─────────────────────────────────────────────────────

    #[test]
    fn snap_col_span_zero_container_returns_12() {
        assert_eq!(snap_to_col_span(0.0, 500.0), 12);
    }

    #[test]
    fn snap_col_span_negative_container_returns_12() {
        assert_eq!(snap_to_col_span(-100.0, 500.0), 12);
    }

    #[test]
    fn snap_col_span_full_width_snaps_to_12() {
        // px_x = full container width → raw_cols = 12 → nearest valid = 12
        assert_eq!(snap_to_col_span(1200.0, 1200.0), 12);
    }

    #[test]
    fn snap_col_span_half_width_snaps_to_6() {
        // px_x = 600 in a 1200px container → raw_cols = 6 → valid
        assert_eq!(snap_to_col_span(1200.0, 600.0), 6);
    }

    #[test]
    fn snap_col_span_5_cols_snaps_to_nearest_valid() {
        // 5 is not in VALID_COL_SPANS → should snap to 4 or 6 (both equidistant)
        // The implementation uses min_by_key on unsigned_abs, so tie-breaks to the
        // first element with smallest distance. Both 4 and 6 are distance-1 from 5;
        // the iterator yields 4 first.
        let result = snap_to_col_span(1200.0, 500.0); // 500/100 = 5 raw cols
        assert!(result == 4 || result == 6, "expected 4 or 6, got {result}");
    }

    #[test]
    fn snap_col_span_clamps_minimum_to_2() {
        // Very small px_x should not go below the minimum valid colSpan of 2
        let result = snap_to_col_span(1200.0, 10.0); // raw_cols ≈ 0 → clamped to 2
        assert_eq!(result, 2);
    }

    #[test]
    fn snap_col_span_all_valid_values_round_trip() {
        // For each valid colSpan, feeding the exact pixel width should snap back to it
        let container = 1200.0;
        let col_width = container / 12.0;
        for &cs in VALID_COL_SPANS {
            let px = cs as f64 * col_width;
            let result = snap_to_col_span(container, px);
            assert_eq!(
                result, cs,
                "colSpan {cs} should round-trip but got {result}"
            );
        }
    }

    // ── snap_height ─────────────────────────────────────────────────────────

    #[test]
    fn snap_height_clamps_to_minimum() {
        assert_eq!(snap_height(0.0), MIN_CHART_HEIGHT_PX);
        assert_eq!(snap_height(50.0), MIN_CHART_HEIGHT_PX);
        assert_eq!(snap_height(99.0), MIN_CHART_HEIGHT_PX);
    }

    #[test]
    fn snap_height_rounds_to_5px_increment() {
        assert_eq!(snap_height(302.0), 300.0);
        assert_eq!(snap_height(303.0), 305.0);
        assert_eq!(snap_height(350.0), 350.0);
        assert_eq!(snap_height(348.0), 350.0);
    }

    #[test]
    fn snap_height_exact_multiples_unchanged() {
        assert_eq!(snap_height(200.0), 200.0);
        assert_eq!(snap_height(500.0), 500.0);
        assert_eq!(snap_height(1000.0), 1000.0);
    }
}
