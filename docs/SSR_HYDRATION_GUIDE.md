# SSR + Hydration Guide

How server-side rendering works in Kyomi and how to add it to new pages.

## Architecture

Kyomi uses Leptos 0.8 with a **hybrid CSR/SSR** approach:

- **Default (CSR)**: Trunk builds the WASM bundle. The server sends a static `index.html` shell with a loading spinner. WASM boots and mounts the entire app client-side via `mount_to_body(App)`.
- **SSR pages**: The server pre-renders the page HTML using `leptos_axum::render_app_to_stream_with_context`, wraps it in the Trunk-built `index.html` template, and sends the full page. The WASM then **hydrates** (attaches event handlers to existing DOM) via `hydrate_body(App)` instead of replacing it.

The WASM entry point (`crates/kyomi-ui/src/main.rs`) detects SSR via a `data-ssr` attribute on `<body>`:

```rust
if body.get_attribute("data-ssr").is_some() {
    hydrate_body(App);         // attach to existing DOM
    body.remove_attribute("data-ssr");  // re-enable buttons, hide progress bar
} else {
    mount_to_body(App);        // CSR: replace DOM entirely
}
```

### Hydration gap UX

Between the HTML arriving and WASM hydrating, the page looks interactive but isn't. Two CSS rules in `index.html` handle this:

```css
/* Disable all buttons until hydrated */
body[data-ssr] button { pointer-events: none; opacity: 0.5; }

/* Amber progress bar via pseudo-element */
body[data-ssr]::before {
    content: ''; position: fixed; top: 0; left: 0; height: 2px;
    background: var(--color-primary, #d97706); z-index: 99999;
    animation: ssr-bar 2.5s ease-out forwards;
}
```

Both disappear automatically when WASM removes `data-ssr` from `<body>`.

---

## How to add SSR to a new page

### Step 1: Wire the route in the server

In `apps/server/src/lib.rs`, change the page's route from `serve_leptos_shell` to `login_ssr_handler` (or create a similar handler):

```rust
// Before (CSR):
.route("/my-page", get(serve_leptos_shell))

// After (SSR):
.route("/my-page", get(my_page_ssr_handler))
```

### Step 2: Create the SSR handler (if needed)

If the page needs custom server context, create a handler in `leptos_frontend.rs` following the `login_ssr_handler` pattern. If not, a generic SSR handler works for any page — the `App` component's `Router` matches the URL and renders the correct page automatically.

The handler:
1. Calls `render_app_to_stream_with_context` with the `App` component
2. Collects the response body bytes
3. Wraps them in the template: `prefix + ssr_html + suffix`

### Step 3: No changes needed to the page component

The page component (`LoginPage`, etc.) doesn't need any SSR-specific code. Leptos renders the same component tree on both server and client. The Router matches the URL and renders the correct page.

### Step 4: Verify

```bash
# Check SSR HTML is returned
curl -s http://localhost:PORT/my-page | grep "data-ssr"

# Check WASM hydrates without errors
# Use Playwright to load the page and verify:
# - data-ssr is removed from <body>
# - Button opacity is 1 (interactive)
# - No console errors (especially no tachys panics)
```

---

## Pitfalls — read before implementing

### 1. Template splitting: `<body` can appear inside CSS comments

`get_template_parts()` splits the Trunk-built `index.html` at the `<body` tag. But `<body` can appear inside CSS comments or selectors (e.g. `body[data-ssr] button`). Always search for `<body` AFTER `</head>` to find the real tag:

```rust
let head_end = html.find("</head>")?;
let body_start = head_end + html[head_end..].find("<body")?;
```

### 2. Never inject DOM elements into `<body>` outside of `<App/>`

Any DOM element in `<body>` that isn't part of the `App` component tree will cause a **tachys hydration panic**. During hydration, tachys walks `<body>`'s children and the virtual DOM in lockstep — an extra element breaks the alignment.

**Wrong**: `<body data-ssr><div class="progress-bar"></div>{ssr_html}</body>`
**Right**: Use CSS pseudo-elements (`body[data-ssr]::before`) for visual indicators.

### 3. Resource ID alignment between SSR and client (CRITICAL)

Leptos assigns sequential integer IDs to `Resource::new()` calls. The server serializes resolved resources as `__RESOLVED_RESOURCES[id]`. The client reads them back by the same ID.

**If the server and client create Resources in a different order, every Resource reads the wrong data and hydration breaks silently.**

The most common cause: `#[cfg(target_arch = "wasm32")]` blocks that create Resources. These Resources exist on the client but not the server, shifting all subsequent IDs by one.

```rust
// WRONG — creates a Resource only in WASM, desyncs IDs
#[cfg(target_arch = "wasm32")]
{
    let auth_check = Resource::new(|| (), |_| get_sidebar_user());
    // auth_check gets ID 0 on client, but doesn't exist on server
    // auth_config (next Resource) gets ID 0 on server but ID 1 on client
}
let auth_config = Resource::new(|| (), |_| get_auth_config());

// RIGHT — use spawn_local for client-only async work
#[cfg(target_arch = "wasm32")]
{
    leptos::task::spawn_local(async move {
        if get_sidebar_user().await.is_ok() { /* redirect */ }
    });
}
let auth_config = Resource::new(|| (), |_| get_auth_config());
```

**Rule**: Never use `Resource::new()` inside `#[cfg(target_arch = "wasm32")]` blocks. Use `spawn_local`, `Effect::new`, or `LocalResource` instead — these don't consume serialized IDs.

### 4. The custom executor for SSR

Leptos reactive Effects call `spawn_local()`, which in tokio requires a `LocalSet` (non-Send). Since axum handlers must produce `Send` futures, we register a custom executor that no-ops `spawn_local`:

```rust
struct SsrExecutor;
impl any_spawner::CustomExecutor for SsrExecutor {
    fn spawn(&self, fut: Pin<Box<dyn Future<Output = ()> + Send>>) { tokio::spawn(fut); }
    fn spawn_local(&self, _fut: Pin<Box<dyn Future<Output = ()>>>) {}
    fn poll_local(&self) {}
}
_ = any_spawner::Executor::init_custom_executor(SsrExecutor);
```

This is safe because Effects are client-side behavior that don't need to run during SSR. This only needs to be initialized once (returns `Err` on subsequent calls, which we ignore).

### 5. WebAuthn rejects IP addresses

If `FRONTEND_URL` is an IP address (e.g. `http://192.168.1.200:3101`), WebAuthn will panic because `rp_id` must be a domain. Use `http://localhost:PORT` for the server. LAN access works for viewing SSR pages but not for logging in.

---

## File reference

| File | Role |
|------|------|
| `apps/server/src/leptos_frontend.rs` | SSR handler, template splitting, custom executor |
| `crates/kyomi-ui/src/main.rs` | WASM entry: hydrate vs mount decision |
| `crates/kyomi-ui/index.html` | Hydration gap CSS (button disable + progress bar) |
| `apps/server/Cargo.toml` | `any_spawner` dependency for custom executor |

## Debugging hydration failures

If you see `panicked at tachys-0.2.14/src/hydration.rs` in the browser console:

1. **Check Resource ID alignment**: Count `Resource::new()` calls in the component. Are any inside `#[cfg(wasm32)]` blocks? Do server and client create them in the same order?
2. **Check for injected DOM nodes**: Is anything in `<body>` that isn't rendered by `App`?
3. **Check conditional rendering**: Do `<Show>` or `match` blocks depend on Resources? If a Resource reads the wrong serialized data (see pitfall #3), conditionals render differently and break hydration.
4. **Enable debug output**: Add `"--cfg=leptos_debuginfo"` to `.cargo/config.toml` under `[target.wasm32-unknown-unknown] rustflags`, touch `~/.cargo/registry/src/.../tachys-*/src/hydration.rs` to force rebuild, then `trunk build --release`. The panic message will name the specific element and location.
