// SPDX-License-Identifier: AGPL-3.0-or-later

//! Stripe.js interop for embedded checkout.
//!
//! Wraps `window.Stripe(pk).initEmbeddedCheckout({ clientSecret, onComplete })`
//! for use from WASM. On non-WASM targets, stub implementations are provided
//! so the crate compiles for SSR.

// ---------------------------------------------------------------------------
// WASM implementation (browser)
// ---------------------------------------------------------------------------
#[cfg(target_arch = "wasm32")]
mod inner {
    use js_sys::{Function, Object, Promise, Reflect};
    use wasm_bindgen::prelude::*;
    use wasm_bindgen_futures::JsFuture;

    /// Handle to an active embedded checkout instance.
    /// Call `destroy()` to unmount the Stripe form and clean up.
    pub struct EmbeddedCheckoutHandle {
        instance: JsValue,
    }

    impl EmbeddedCheckoutHandle {
        /// Unmount the embedded checkout form and release resources.
        pub fn destroy(&self) {
            if let Ok(destroy_fn) = Reflect::get(&self.instance, &"destroy".into()) {
                if let Ok(func) = destroy_fn.dyn_into::<Function>() {
                    let _ = func.call0(&self.instance);
                }
            }
        }
    }

    impl Drop for EmbeddedCheckoutHandle {
        fn drop(&mut self) {
            self.destroy();
        }
    }

    /// Initialize and mount an embedded Stripe checkout form.
    ///
    /// - `publishable_key`: Stripe publishable key (pk_test_ or pk_live_)
    /// - `client_secret`: From the embedded checkout session
    /// - `mount_selector`: CSS selector for the mount point (e.g. "#checkout-mount")
    /// - `on_complete`: Called when the user completes checkout
    ///
    /// Returns a handle that must be kept alive. Dropping it destroys the form.
    pub async fn mount_embedded_checkout(
        publishable_key: &str,
        client_secret: &str,
        mount_selector: &str,
        on_complete: impl Fn() + 'static,
    ) -> Result<EmbeddedCheckoutHandle, String> {
        let window = web_sys::window().ok_or("No window object")?;

        // Get the Stripe constructor from window.Stripe
        let stripe_constructor = Reflect::get(&window, &"Stripe".into())
            .map_err(|_| "Stripe.js not loaded. Ensure the script tag is present.")?;

        if stripe_constructor.is_undefined() || stripe_constructor.is_null() {
            return Err("Stripe.js not loaded yet. Please try again.".into());
        }

        let stripe_fn = stripe_constructor
            .dyn_into::<Function>()
            .map_err(|_| "window.Stripe is not a function")?;

        // Create Stripe instance: Stripe(publishableKey)
        let stripe = stripe_fn
            .call1(&JsValue::NULL, &publishable_key.into())
            .map_err(|e| format!("Failed to initialize Stripe: {e:?}"))?;

        // Build options: { fetchClientSecret, onComplete }
        // Stripe requires fetchClientSecret as a callback that returns the secret.
        let options = Object::new();

        let secret = client_secret.to_string();
        let fetch_secret_closure = Closure::wrap(Box::new(move || {
            let secret = secret.clone();
            Promise::resolve(&JsValue::from_str(&secret))
        }) as Box<dyn Fn() -> Promise>);
        Reflect::set(
            &options,
            &"fetchClientSecret".into(),
            fetch_secret_closure.as_ref().unchecked_ref(),
        )
        .map_err(|_| "Failed to set fetchClientSecret")?;
        fetch_secret_closure.forget();

        let on_complete_closure = Closure::wrap(Box::new(on_complete) as Box<dyn Fn()>);
        Reflect::set(
            &options,
            &"onComplete".into(),
            on_complete_closure.as_ref().unchecked_ref(),
        )
        .map_err(|_| "Failed to set onComplete")?;
        on_complete_closure.forget();

        // stripe.initEmbeddedCheckout({ fetchClientSecret, onComplete })
        let init_fn = Reflect::get(&stripe, &"initEmbeddedCheckout".into())
            .map_err(|_| "Stripe object missing initEmbeddedCheckout method")?
            .dyn_into::<Function>()
            .map_err(|_| "initEmbeddedCheckout is not a function")?;

        let promise = init_fn
            .call1(&stripe, &options)
            .map_err(|e| format!("initEmbeddedCheckout failed: {e:?}"))?;

        let checkout = JsFuture::from(Promise::from(promise))
            .await
            .map_err(|e| format!("initEmbeddedCheckout rejected: {e:?}"))?;

        // Mount the checkout form into the DOM
        let mount_fn = Reflect::get(&checkout, &"mount".into())
            .map_err(|_| "Checkout object missing mount method")?
            .dyn_into::<Function>()
            .map_err(|_| "mount is not a function")?;

        mount_fn
            .call1(&checkout, &mount_selector.into())
            .map_err(|e| format!("mount failed: {e:?}"))?;

        Ok(EmbeddedCheckoutHandle { instance: checkout })
    }
}

// ---------------------------------------------------------------------------
// SSR stubs
// ---------------------------------------------------------------------------
#[cfg(not(target_arch = "wasm32"))]
mod inner {
    pub struct EmbeddedCheckoutHandle;

    impl EmbeddedCheckoutHandle {
        pub fn destroy(&self) {}
    }

    pub async fn mount_embedded_checkout(
        _publishable_key: &str,
        _client_secret: &str,
        _mount_selector: &str,
        _on_complete: impl Fn() + 'static,
    ) -> Result<EmbeddedCheckoutHandle, String> {
        Err("Embedded checkout is only available in the browser".into())
    }
}

pub use inner::*;
