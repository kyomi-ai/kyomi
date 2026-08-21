// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback modal — lets users submit bug reports, feature requests, or questions.
//!
//! Matches the React `FeedbackModal` component with full feature parity:
//! - Feedback type selector (pill buttons) with dynamic placeholder
//! - Description textarea
//! - Screenshot capture (screen capture API) or image upload
//! - "Include technical context" checkbox
//! - Console error / failed request context collection
//!
//! A fourth, normally-hidden type — "Request BigQuery Access" — appears only
//! when opened via `access_request_context` (KYO-417). See
//! [`ACCESS_REQUEST_TYPE`] and the `FeedbackModal` prop doc.

use leptos::ev;
use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::alert::{Alert, AlertDescription, AlertVariant};
use crate::components::button::{Button, ButtonSize, ButtonVariant};
use crate::components::checkbox::Checkbox;
use crate::components::modal::{Modal, ModalSize};
use crate::server_fns::feedback::submit_feedback;

/// Feedback type options shown to every caller of `FeedbackModal`.
///
/// These are the only options visible from the normal entry point
/// (`crates/kyomi-ui/src/components/layout.rs`'s "Send Feedback" nav item).
/// See [`ACCESS_REQUEST_TYPE`] for the one option that is *not* here.
const FEEDBACK_TYPES: &[(&str, &str, phosphor_leptos::IconData)] = &[
    ("bug", "Bug", phosphor_leptos::BUG),
    ("feature", "Feature Request", phosphor_leptos::LIGHTBULB),
    ("question", "Question", phosphor_leptos::QUESTION),
];

/// The BigQuery-access-request option (KYO-417).
///
/// Deliberately excluded from [`FEEDBACK_TYPES`] — it is meaningless
/// outside the context of a user blocked on BigQuery Kyomi OAuth, and
/// would clutter the Type selector for everyone else. It appears only
/// when the modal is opened with `access_request_context` set; see
/// [`visible_feedback_types`].
///
/// KYO-408 will drive this from a "Request BigQuery Access" link on the
/// datasource connection page — that link doesn't exist yet, so today the
/// only way to reach this option is `access_request_context=true`.
const ACCESS_REQUEST_TYPE: (&str, &str, phosphor_leptos::IconData) =
    ("access_request", "Request BigQuery Access", phosphor_leptos::KEY);

/// The feedback type options to render in the Type selector.
///
/// `access_request_context` mirrors the modal's prop of the same name —
/// pass `true` only when the modal was opened from a context where
/// requesting BigQuery access makes sense (KYO-408's future link).
/// Server-side validation accepts `access_request` regardless of this
/// flag (see `kyomi_auth::feedback_service::is_valid_feedback_type`) —
/// this is a UX gate, not a security boundary.
fn visible_feedback_types(
    access_request_context: bool,
) -> Vec<(&'static str, &'static str, phosphor_leptos::IconData)> {
    if access_request_context {
        FEEDBACK_TYPES
            .iter()
            .copied()
            .chain(std::iter::once(ACCESS_REQUEST_TYPE))
            .collect()
    } else {
        FEEDBACK_TYPES.to_vec()
    }
}

/// The feedback type preselected when the modal opens.
fn default_feedback_type(access_request_context: bool) -> &'static str {
    if access_request_context {
        ACCESS_REQUEST_TYPE.0
    } else {
        "bug"
    }
}

/// Placeholder text for the description textarea, keyed by feedback type.
///
/// `access_request`'s prompt is the one piece of information that request
/// needs to be actionable: the Google account email to allowlist — Kyomi
/// has no programmatic access to the OAuth consent screen's test-user
/// list, so a human has to add it by hand from what's in this field.
fn feedback_placeholder(feedback_type: &str) -> &'static str {
    match feedback_type {
        "bug" => "What happened? What did you expect to happen?",
        "feature" => "What would you like to see? How would it help you?",
        "question" => "What's your question? What are you trying to do?",
        "access_request" => {
            "What Google account email should we allowlist for BigQuery access? (e.g. jane@company.com)"
        }
        _ => "Describe your feedback in detail...",
    }
}

/// Textarea class — based on INPUT_CLASS but adapted for multi-line input.
const TEXTAREA_CLASS: &str = "w-full min-h-[120px] resize-y bg-transparent border border-input rounded-md px-3 py-2 text-sm text-foreground shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring md:text-sm";

/// Active pill button class — matches FilterButton active state from chat_list.rs.
const PILL_ACTIVE: &str = "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground";

/// Inactive pill button class — matches FilterButton inactive state from chat_list.rs.
const PILL_INACTIVE: &str = "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-secondary text-foreground border border-border hover:bg-secondary/80";

/// JS interop for screen capture via `getDisplayMedia`.
///
/// Uses a canvas to grab a single frame from the display media stream,
/// converts to a JPEG data URL (85% quality — 5-10x smaller than PNG for
/// screenshots, well within the 2MB server limit even on HiDPI displays),
/// and stops all tracks immediately.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(inline_js = r#"
export async function captureScreenshot() {
    const stream = await navigator.mediaDevices.getDisplayMedia({
        video: { displaySurface: 'browser' },
        preferCurrentTab: true,
    });
    const track = stream.getVideoTracks()[0];

    // Create a video element to capture a frame
    const video = document.createElement('video');
    video.srcObject = stream;
    video.autoplay = true;
    await new Promise(resolve => { video.onloadeddata = resolve; });
    // Small delay to ensure the frame is fully rendered
    await new Promise(resolve => setTimeout(resolve, 100));

    const canvas = document.createElement('canvas');
    canvas.width = video.videoWidth;
    canvas.height = video.videoHeight;
    const ctx = canvas.getContext('2d');
    ctx.drawImage(video, 0, 0);

    stream.getTracks().forEach(t => t.stop());

    return canvas.toDataURL('image/jpeg', 0.85);
}
"#)]
extern "C" {
    #[wasm_bindgen::prelude::wasm_bindgen(js_name = "captureScreenshot", catch)]
    async fn capture_screenshot_js() -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue>;
}

/// Modal for submitting user feedback.
///
/// Shows a type selector (Bug / Feature Request / Question), a description
/// textarea, screenshot capture/upload, context checkbox, and a submit button.
/// On success, displays a thank-you message and auto-closes after 1.5 seconds.
#[component]
pub fn FeedbackModal(
    /// Whether the modal is visible.
    #[prop(into)]
    open: Signal<bool>,
    /// Called when the modal should open or close.
    on_open_change: Callback<bool>,
    /// Reveals and preselects the "Request BigQuery Access" type (KYO-417).
    ///
    /// Defaults to `false`, so the normal entry point
    /// (`crates/kyomi-ui/src/components/layout.rs`'s "Send Feedback" nav
    /// item, which does not pass this prop) never sees the option. KYO-408
    /// drives this from the datasource connection page: set a signal to
    /// `true` immediately before opening the modal from its
    /// "Request BigQuery Access" link, mirroring how `open`/`on_open_change`
    /// are already driven by caller-owned state.
    #[prop(into, default = Signal::stored(false))]
    access_request_context: Signal<bool>,
) -> impl IntoView {
    // Form state
    let (feedback_type, set_feedback_type) = signal("bug".to_string());
    let (description, set_description) = signal(String::new());
    let (include_context, set_include_context) = signal(true);
    let (screenshot_data, set_screenshot_data) = signal(Option::<String>::None);
    let (screenshot_preview, set_screenshot_preview) = signal(Option::<String>::None);
    let (submitting, set_submitting) = signal(false);
    let (capturing, set_capturing) = signal(false);
    let (error, set_error) = signal(Option::<String>::None);
    let (success, set_success) = signal(false);
    let (reopening_after_capture, set_reopening_after_capture) = signal(false);

    // Dynamic placeholder based on feedback type — matches React getPlaceholder()
    let placeholder = Memo::new(move |_| feedback_placeholder(feedback_type.get().as_str()));

    // Description must be >= 10 chars to enable submit
    let can_submit = Memo::new(move |_| {
        let desc = description.get();
        desc.trim().len() >= 10 && !submitting.get()
    });

    // Reset form state when modal opens
    Effect::new(move |_| {
        if open.get() {
            if reopening_after_capture.get_untracked() {
                set_reopening_after_capture.set(false);
                return;
            }
            // Read at the moment the modal opens, not reactively — a later
            // change to access_request_context (e.g. the caller resetting it
            // after close) must not re-trigger this whole reset while the
            // modal is already open and the user has typed something.
            set_feedback_type
                .set(default_feedback_type(access_request_context.get_untracked()).to_string());
            set_description.set(String::new());
            set_include_context.set(true);
            set_screenshot_data.set(None);
            set_screenshot_preview.set(None);
            set_error.set(None);
            set_success.set(false);
            set_submitting.set(false);
            set_capturing.set(false);
        }
    });

    // Submit action — passes all fields including context and screenshot
    let submit = Action::new(
        move |(ft, desc, inc_ctx, ctx, screenshot): &(String, String, bool, String, Option<String>)| {
            let ft = ft.clone();
            let desc = desc.clone();
            let inc_ctx = *inc_ctx;
            let ctx = ctx.clone();
            let screenshot = screenshot.clone();
            async move { submit_feedback(ft, desc, inc_ctx, ctx, screenshot).await }
        },
    );

    // Handle submit result
    Effect::new(move |_| {
        if let Some(result) = submit.value().get() {
            set_submitting.set(false);
            match result {
                Ok(_) => {
                    set_success.set(true);
                    // Clear the feedback context after successful submission
                    #[cfg(target_arch = "wasm32")]
                    crate::utils::feedback_context::clear();
                    // Auto-close after 1.5 seconds
                    let open_change = on_open_change;
                    set_timeout(
                        move || {
                            open_change.run(false);
                        },
                        std::time::Duration::from_millis(1500),
                    );
                }
                Err(e) => {
                    set_error.set(Some(e.to_string()));
                }
            }
        }
    });

    // Close handler that resets state
    let handle_close = Callback::new(move |()| {
        on_open_change.run(false);
    });

    // Screen capture logic — extracted so it can be called from both the initial
    // capture button and the retake button. All captured bindings are Copy signals.
    let trigger_capture = move || {
        #[cfg(target_arch = "wasm32")]
        {
            set_capturing.set(true);
            set_error.set(None);
            set_reopening_after_capture.set(true);

            // Close modal temporarily so user can select what to capture
            on_open_change.run(false);

            leptos::task::spawn_local(async move {
                // Small delay to let the modal close
                gloo_timers::future::TimeoutFuture::new(200).await;

                match capture_screenshot_js().await {
                    Ok(val) => {
                        if let Some(data_url) = val.as_string() {
                            // Validate size: base64 length * 3/4 estimates decoded bytes.
                            // Reject anything that exceeds the server's MAX_SCREENSHOT_BYTES (2MB).
                            let estimated_bytes = data_url.len() * 3 / 4;
                            if estimated_bytes > 2 * 1024 * 1024 {
                                let _ = set_error.try_set(Some(
                                    "Image too large (max 2MB). Try \"Upload Image\" instead.".to_string(),
                                ));
                            } else {
                                let _ = set_screenshot_preview.try_set(Some(data_url.clone()));
                                let _ = set_screenshot_data.try_set(Some(data_url));
                            }
                        }
                    }
                    Err(e) => {
                        let msg = e
                            .as_string()
                            .or_else(|| {
                                js_sys::Reflect::get(&e, &"message".into())
                                    .ok()
                                    .and_then(|v| v.as_string())
                            })
                            .unwrap_or_else(|| "Screen capture failed".to_string());
                        if !msg.contains("NotAllowedError") && !msg.contains("cancelled") {
                            let _ = set_error.try_set(Some(
                                "Screen capture failed. Try \"Upload Image\" instead.".to_string(),
                            ));
                        }
                    }
                }
                let _ = set_capturing.try_set(false);

                // Reopen the modal after capture completes
                on_open_change.run(true);
            });
        }
    };

    let handle_capture = move |_: ev::MouseEvent| {
        trigger_capture();
        #[cfg(not(target_arch = "wasm32"))]
        let _ = set_capturing;
    };

    // File upload handler — uses a hidden input element (WASM only)
    let handle_upload = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::prelude::*;

            let Some(window) = web_sys::window() else { return };
            let Some(document) = window.document() else { return };
            let Ok(el) = document.create_element("input") else { return };
            let input: web_sys::HtmlInputElement = el.unchecked_into();
            input.set_type("file");
            input.set_accept("image/*");

            let set_preview = set_screenshot_preview;
            let set_data = set_screenshot_data;
            let set_err = set_error;

            let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |ev: web_sys::Event| {
                let Some(input_el) = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                else {
                    return;
                };
                if let Some(files) = input_el.files()
                    && let Some(file) = files.get(0) {
                        // Validate size: 2MB decoded ~ 2.67MB base64 (matching REST route MAX_SCREENSHOT_BYTES)
                        if file.size() > 2.67 * 1024.0 * 1024.0 {
                            let _ = set_err.try_set(Some("Image must be less than 2MB".to_string()));
                            return;
                        }
                        let Ok(reader) = web_sys::FileReader::new() else { return };
                        let reader_clone = reader.clone();
                        let onload = Closure::<dyn Fn(web_sys::Event)>::new(
                            move |_: web_sys::Event| {
                                if let Ok(result) = reader_clone.result()
                                    && let Some(data_url) = result.as_string() {
                                        let _ = set_preview.try_set(Some(data_url.clone()));
                                        let _ = set_data.try_set(Some(data_url));
                                    }
                            },
                        );
                        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
                        onload.forget(); // prevent GC
                        let _ = reader.read_as_data_url(&file);
                    }
                // Clean up the hidden input
                if let Some(el) = web_sys::window()
                    .and_then(|w| w.document())
                    .and_then(|d| d.query_selector("input[data-feedback-upload]").ok().flatten())
                {
                    el.remove();
                }
            });

            let _ = input.set_attribute("data-feedback-upload", "");
            let _ = input.set_attribute("style", "display:none");
            let _ = input.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref());
            closure.forget(); // prevent GC
            if let Some(body) = document.body() {
                let _ = body.append_child(&input);
            }
            input.click();
        }
    };

    view! {
        <Modal
            show=open
            on_close=handle_close
            title="Send Feedback"
            size=ModalSize::Md
        >
            <Show
                when=move || !success.get()
                fallback=move || view! {
                    // Success state
                    <div class="flex flex-col items-center justify-center py-8 gap-3">
                        <div class="w-12 h-12 rounded-full bg-success/20 flex items-center justify-center">
                            <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Fill size="28px" attr:class="text-success-foreground"/>
                        </div>
                        <p class="text-sm text-foreground font-medium">"Thank you for your feedback!"</p>
                    </div>
                }
            >
                <div class="space-y-4">
                    // Type selector — pill buttons
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Type"</label>
                        <div class="flex gap-2">
                            {move || {
                                visible_feedback_types(access_request_context.get())
                                    .into_iter()
                                    .map(|(value, label, icon)| {
                                        view! {
                                            <button
                                                type="button"
                                                class=move || {
                                                    if feedback_type.get() == value {
                                                        PILL_ACTIVE
                                                    } else {
                                                        PILL_INACTIVE
                                                    }
                                                }
                                                on:click=move |_| set_feedback_type.set(value.to_string())
                                            >
                                                <Icon icon=icon weight=IconWeight::Regular size="16px"/>
                                                {label}
                                            </button>
                                        }
                                    })
                                    .collect_view()
                            }}
                        </div>
                    </div>

                    // Description textarea with dynamic placeholder
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Description"</label>
                        <textarea
                            class=TEXTAREA_CLASS
                            placeholder=move || placeholder.get()
                            prop:value=move || description.get()
                            on:input=move |ev| {
                                set_description.set(event_target_value(&ev));
                                // Clear error when user starts typing
                                set_error.set(None);
                            }
                        />
                        <p class="mt-1 text-xs text-muted-foreground">
                            {move || {
                                let len = description.get().trim().len();
                                if len < 10 {
                                    format!("{} more character{} needed", 10 - len, if 10 - len == 1 { "" } else { "s" })
                                } else {
                                    format!("{len} characters")
                                }
                            }}
                        </p>
                    </div>

                    // Screenshot section — capture or upload
                    <div>
                        <label class="block text-sm font-medium text-foreground mb-2">"Screenshot (optional)"</label>
                        <Show
                            when=move || screenshot_preview.get().is_some()
                            fallback=move || view! {
                                <div class="flex gap-2">
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        disabled=MaybeProp::derive(move || Some(capturing.get()))
                                        on:click=handle_capture
                                    >
                                        <Icon icon=phosphor_leptos::CAMERA weight=IconWeight::Regular size="16px"/>
                                        {move || if capturing.get() { "Capturing..." } else { "Capture Screen" }}
                                    </Button>
                                    <Button
                                        variant=ButtonVariant::Outline
                                        size=ButtonSize::Sm
                                        on:click=handle_upload
                                    >
                                        <Icon icon=phosphor_leptos::UPLOAD weight=IconWeight::Regular size="16px"/>
                                        "Upload Image"
                                    </Button>
                                </div>
                            }
                        >
                            <div class="flex items-start gap-3">
                                <div class="relative inline-block shrink-0">
                                    <img
                                        src=move || screenshot_preview.get().unwrap_or_default()
                                        alt="Screenshot preview"
                                        class="max-h-32 rounded border border-border"
                                    />
                                </div>
                                <div class="flex flex-col gap-2 min-w-0">
                                    <div class="flex items-center gap-1.5 text-sm font-medium text-success-foreground">
                                        <Icon icon=phosphor_leptos::CHECK_CIRCLE weight=IconWeight::Fill size="16px"/>
                                        "Screenshot attached"
                                    </div>
                                    <div class="flex gap-2">
                                        <Button
                                            variant=ButtonVariant::Outline
                                            size=ButtonSize::Sm
                                            on:click=move |_| { trigger_capture(); }
                                        >
                                            <Icon icon=phosphor_leptos::CAMERA weight=IconWeight::Regular size="14px"/>
                                            "Retake"
                                        </Button>
                                        <Button
                                            variant=ButtonVariant::Outline
                                            size=ButtonSize::Sm
                                            on:click=move |_| {
                                                set_screenshot_data.set(None);
                                                set_screenshot_preview.set(None);
                                            }
                                        >
                                            <Icon icon=phosphor_leptos::X weight=IconWeight::Regular size="14px"/>
                                            "Remove"
                                        </Button>
                                    </div>
                                </div>
                            </div>
                        </Show>
                    </div>

                    // Context consent checkbox — matches React FeedbackModal
                    <div class="flex items-start space-x-3 rounded-md border border-border p-3 bg-muted/50">
                        <div class="mt-1">
                            <Checkbox
                                checked=Signal::derive(move || include_context.get())
                                on_change=Callback::new(move |v: bool| set_include_context.set(v))
                            />
                        </div>
                        <div class="space-y-1">
                            <label
                                class="text-sm font-medium cursor-pointer text-foreground"
                                on:click=move |_| set_include_context.update(|v| *v = !*v)
                            >
                                "Include technical details to help us debug faster"
                            </label>
                            <p class="text-xs text-muted-foreground">
                                "Current page, browser info, and recent errors"
                            </p>
                        </div>
                    </div>

                    // Error message
                    <Show when=move || error.get().is_some()>
                        <Alert variant=AlertVariant::Error>
                            <AlertDescription>
                                {move || error.get().unwrap_or_default()}
                            </AlertDescription>
                        </Alert>
                    </Show>

                    // Submit button
                    <div class="flex justify-end">
                        <Button
                            variant=ButtonVariant::Default
                            disabled=MaybeProp::derive(move || Some(!can_submit.get()))
                            on:click=move |_| {
                                set_submitting.set(true);
                                set_error.set(None);

                                // Collect context if user opted in
                                let context_json = {
                                    #[cfg(target_arch = "wasm32")]
                                    {
                                        if include_context.get() {
                                            crate::utils::feedback_context::collect_context()
                                        } else {
                                            "{}".to_string()
                                        }
                                    }
                                    #[cfg(not(target_arch = "wasm32"))]
                                    { "{}".to_string() }
                                };

                                submit.dispatch((
                                    feedback_type.get(),
                                    description.get(),
                                    include_context.get(),
                                    context_json,
                                    screenshot_data.get(),
                                ));
                            }
                        >
                            {move || if submitting.get() { "Sending..." } else { "Send Feedback" }}
                        </Button>
                    </div>
                </div>
            </Show>
        </Modal>
    }
}

#[cfg(test)]
mod tests {
    //! KYO-417: the access-request option must be invisible from the normal
    //! entry point and appear (preselected) only when the modal is opened
    //! with `access_request_context` set. These tests cover the pure
    //! selection logic the view closures call — see `visible_feedback_types`
    //! and `default_feedback_type` above.

    use super::*;

    #[test]
    fn access_request_is_absent_from_the_default_list() {
        let types = visible_feedback_types(false);
        assert_eq!(
            types.len(),
            3,
            "the normal entry point (layout.rs) must see exactly bug/feature/question"
        );
        assert!(
            !types.iter().any(|(value, _, _)| *value == "access_request"),
            "access_request must not appear when access_request_context is false"
        );
    }

    #[test]
    fn access_request_appears_when_context_gated() {
        let types = visible_feedback_types(true);
        assert_eq!(types.len(), 4);
        assert!(
            types.iter().any(|(value, _, _)| *value == "access_request"),
            "access_request must appear when access_request_context is true"
        );
    }

    #[test]
    fn default_type_is_bug_outside_access_request_context() {
        assert_eq!(default_feedback_type(false), "bug");
    }

    #[test]
    fn default_type_is_access_request_when_gated() {
        assert_eq!(
            default_feedback_type(true),
            "access_request",
            "opening the modal via the gated context must preselect the option"
        );
    }

    #[test]
    fn placeholder_asks_for_the_google_account_email() {
        let placeholder = feedback_placeholder("access_request");
        assert!(
            placeholder.to_lowercase().contains("google")
                && placeholder.to_lowercase().contains("email"),
            "the access_request placeholder must ask for the Google account \
             email to allowlist — that's the one piece of information the \
             request needs to be actionable, got: {placeholder}"
        );
    }

    #[test]
    fn ui_and_server_type_lists_do_not_drift() {
        // The gated list (superset of the default list) is the full set the
        // UI knows about. It must match the server's validation allowlist
        // exactly — see kyomi_types::FEEDBACK_TYPE_VALUES and
        // kyomi_auth::feedback_service::is_valid_feedback_type.
        let mut ui_values: Vec<&str> = visible_feedback_types(true)
            .into_iter()
            .map(|(value, _, _)| value)
            .collect();
        ui_values.sort_unstable();

        let mut shared_values: Vec<&str> = kyomi_types::FEEDBACK_TYPE_VALUES.to_vec();
        shared_values.sort_unstable();

        assert_eq!(
            ui_values, shared_values,
            "kyomi-ui's feedback types and kyomi_types::FEEDBACK_TYPE_VALUES have drifted apart"
        );
    }
}
