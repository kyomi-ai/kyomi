// SPDX-License-Identifier: AGPL-3.0-or-later

//! Feedback modal — lets users submit bug reports, feature requests, or questions.
//!
//! Matches the React `FeedbackModal` component with full feature parity:
//! - Feedback type selector (pill buttons) with dynamic placeholder
//! - Description textarea
//! - Screenshot capture (screen capture API) or image upload
//! - "Include technical context" checkbox
//! - Console error / failed request context collection

use leptos::prelude::*;
use phosphor_leptos::{Icon, IconWeight};

use crate::components::alert::{Alert, AlertDescription, AlertVariant};
use crate::components::button::{Button, ButtonSize, ButtonVariant};
use crate::components::checkbox::Checkbox;
use crate::components::modal::{Modal, ModalSize};
use crate::server_fns::feedback::submit_feedback;

/// Feedback type options matching the backend's allowed values.
const FEEDBACK_TYPES: &[(&str, &str, phosphor_leptos::IconData)] = &[
    ("bug", "Bug", phosphor_leptos::BUG),
    ("feature", "Feature Request", phosphor_leptos::LIGHTBULB),
    ("question", "Question", phosphor_leptos::QUESTION),
];

/// Textarea class — based on INPUT_CLASS but adapted for multi-line input.
const TEXTAREA_CLASS: &str = "w-full min-h-[120px] resize-y bg-transparent border border-input rounded-md px-3 py-2 text-sm text-foreground shadow-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring md:text-sm";

/// Active pill button class — matches FilterButton active state from chat_list.rs.
const PILL_ACTIVE: &str = "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-primary text-primary-foreground";

/// Inactive pill button class — matches FilterButton inactive state from chat_list.rs.
const PILL_INACTIVE: &str = "px-3 py-1.5 text-sm rounded-lg transition-colors flex items-center gap-1.5 bg-secondary text-foreground border border-border hover:bg-secondary/80";

/// JS interop for screen capture via `getDisplayMedia`.
///
/// Uses a canvas to grab a single frame from the display media stream,
/// converts to a PNG data URL, and stops all tracks immediately.
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

    return canvas.toDataURL('image/png');
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

    // Dynamic placeholder based on feedback type — matches React getPlaceholder()
    let placeholder = Memo::new(move |_| {
        match feedback_type.get().as_str() {
            "bug" => "What happened? What did you expect to happen?",
            "feature" => "What would you like to see? How would it help you?",
            "question" => "What's your question? What are you trying to do?",
            _ => "Describe your feedback in detail...",
        }
    });

    // Description must be >= 10 chars to enable submit
    let can_submit = Memo::new(move |_| {
        let desc = description.get();
        desc.trim().len() >= 10 && !submitting.get()
    });

    // Reset form state when modal opens
    Effect::new(move |_| {
        if open.get() {
            set_feedback_type.set("bug".to_string());
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

    // Screen capture handler — WASM only
    let handle_capture = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            set_capturing.set(true);
            set_error.set(None);

            // Close modal temporarily so user can select what to capture
            on_open_change.run(false);

            leptos::task::spawn_local(async move {
                // Small delay to let the modal close
                gloo_timers::future::TimeoutFuture::new(200).await;

                match capture_screenshot_js().await {
                    Ok(val) => {
                        if let Some(data_url) = val.as_string() {
                            set_screenshot_preview.set(Some(data_url.clone()));
                            set_screenshot_data.set(Some(data_url));
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
                            set_error.set(Some(
                                "Screen capture failed. Try \"Upload Image\" instead.".to_string(),
                            ));
                        }
                    }
                }
                set_capturing.set(false);

                // Reopen the modal after capture completes
                on_open_change.run(true);
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = set_capturing; // suppress unused warning
    };

    // File upload handler — uses a hidden input element (WASM only)
    let handle_upload = move |_| {
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::prelude::*;

            let document = web_sys::window().unwrap().document().unwrap();
            let input: web_sys::HtmlInputElement = document
                .create_element("input")
                .unwrap()
                .unchecked_into();
            input.set_type("file");
            input.set_accept("image/*");

            let set_preview = set_screenshot_preview;
            let set_data = set_screenshot_data;
            let set_err = set_error;

            let closure = Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
                let input_el: web_sys::HtmlInputElement =
                    web_sys::window()
                        .unwrap()
                        .document()
                        .unwrap()
                        .query_selector("input[type='file'][data-feedback-upload]")
                        .unwrap()
                        .unwrap()
                        .unchecked_into();
                if let Some(files) = input_el.files() {
                    if let Some(file) = files.get(0) {
                        // Validate size: 2MB decoded ~ 2.67MB base64 (matching REST route MAX_SCREENSHOT_BYTES)
                        if file.size() > 2.67 * 1024.0 * 1024.0 {
                            set_err.set(Some("Image must be less than 2MB".to_string()));
                            return;
                        }
                        let reader = web_sys::FileReader::new().unwrap();
                        let reader_clone = reader.clone();
                        let onload = Closure::<dyn Fn(web_sys::Event)>::new(
                            move |_: web_sys::Event| {
                                if let Ok(result) = reader_clone.result() {
                                    if let Some(data_url) = result.as_string() {
                                        set_preview.set(Some(data_url.clone()));
                                        set_data.set(Some(data_url));
                                    }
                                }
                            },
                        );
                        reader.set_onload(Some(onload.as_ref().unchecked_ref()));
                        onload.forget(); // prevent GC
                        let _ = reader.read_as_data_url(&file);
                    }
                }
                // Clean up the hidden input
                if let Some(el) = web_sys::window()
                    .unwrap()
                    .document()
                    .unwrap()
                    .query_selector("input[data-feedback-upload]")
                    .ok()
                    .flatten()
                {
                    el.remove();
                }
            });

            input.set_attribute("data-feedback-upload", "").unwrap();
            input.set_attribute("style", "display:none").unwrap();
            input.add_event_listener_with_callback("change", closure.as_ref().unchecked_ref()).unwrap();
            closure.forget(); // prevent GC
            document.body().unwrap().append_child(&input).unwrap();
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
                            {FEEDBACK_TYPES.iter().map(|(value, label, icon)| {
                                let value = *value;
                                let label = *label;
                                let icon = *icon;
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
                            }).collect_view()}
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
