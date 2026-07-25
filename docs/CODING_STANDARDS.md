# Coding Standards

This document captures coding standards learned from code reviews. It evolves over time — the orchestrator updates it at the start of each `/agent-driven-development` session by mining recent review logs for recurring patterns.

**Read this document before implementing any feature.** Every rule here exists because agents have repeatedly made the same mistake, and a code reviewer had to catch it.

Rules in this document are specific to patterns observed in this codebase. For general architecture principles, see `CLAUDE.md`. For the full anti-pattern checklist used by reviewers, see `.claude/agents/code-review-architect.md`.

---

## Error Handling

*Standards for how errors should be propagated, contextualized, and reported.*

### Never use `.ok()` or `.ok()?` to discard errors without logging

Converting a `Result` to `Option` via `.ok()` silently swallows the error — no log, no trace, no signal that something went wrong. In production, this turns debugging into archaeology: the symptom appears far from the cause, and there's no record of what failed.

**Rule:** Before calling `.ok()`, add a `.map_err()` that logs the error at `warn!` or `debug!` level. If the error is truly unactionable (best-effort fire-and-forget), at minimum log at `debug!` so it shows up in verbose traces.

```rust
// WRONG — error silently discarded, no trace
let cost = fetch_openrouter_cost(&client, &gen_id).await.ok();
let settings = load_title_model(&pool, workspace_id).await.ok().flatten();

// RIGHT — error logged before discarding
let cost = fetch_openrouter_cost(&client, &gen_id)
    .await
    .map_err(|e| debug!(generation_id = %gen_id, error = %e, "cost fetch failed"))
    .ok();

let settings = load_title_model(&pool, workspace_id)
    .await
    .map_err(|e| warn!(%workspace_id, error = %e, "failed to load title model"))
    .ok()
    .flatten();
```

Flagged across 3 reviews in May 2026: KYO-37 (`serde_json::Error` discarded), KYO-36 (network errors discarded), KYO-34 (`WorkspaceAiConfigError` discarded).

## Leptos / Frontend Patterns

*Standards specific to Leptos components, reactivity, SSR/hydration, and frontend architecture.*

### Never use raw `spawn_local` for user-triggered mutations — use `Action`

**This is the #1 source of WASM panics in the codebase.** Raw `spawn_local` spawns a detached future that outlives the component scope. When the user navigates away mid-async, the future completes and accesses disposed signals → panic at `reactive_graph/src/traits.rs:361`.

Leptos provides `Action` as the framework primitive for "user clicks → async call → update UI." It manages pending state, result signals, and scope lifecycle automatically.

**Rule:** If the pattern is "user interaction triggers async server call, then update UI," use `Action`. Reserve `spawn_local` for cases where Action genuinely doesn't fit (WebSocket handlers, multi-step orchestrations, fire-and-forget with no UI update).

```rust
// WRONG — raw spawn_local for a user-triggered mutation
let (creating, set_creating) = signal(false);
let handle_create = move |_| {
    set_creating.set(true);
    spawn_local(async move {
        match create_thing().await {
            Ok(id) => navigate(&format!("/thing/{id}"));
            Err(e) => {
                toast_error(format!("Failed: {e}"));
                set_creating.set(false);  // 💥 panics if navigated away
            }
        }
    });
};

// RIGHT — Action handles lifecycle, pending state, and result
let create_action = Action::new(move |_: &()| async move {
    create_thing().await
});

// React to result in an Effect (dies with the component — no panic)
Effect::new(move |_| {
    if let Some(Ok(id)) = create_action.value().get() {
        navigate(&format!("/thing/{id}"));
    }
    if let Some(Err(e)) = create_action.value().get() {
        toast_error(format!("Failed: {e}"));
    }
});

// In view — pending state is built-in
view! {
    <Button on:click=move |_| create_action.dispatch(())
            disabled=Signal::derive(move || create_action.pending().get())>
        "Create"
    </Button>
}
```

**When Action doesn't fit:** Reserve `spawn_local` for cases where Action genuinely can't work:
- **`!Send` browser APIs** (`JsFuture`, `TimeoutFuture`, clipboard, `web_sys` DOM calls) — `Action::new` requires a `Send` future. Browser-only APIs are `!Send` on wasm32 and will cause compilation failures. Use `spawn_local` with `try_` signal writes for the deferred part.
- **WebSocket callbacks, long-lived subscriptions** — use `on_cleanup` to unsubscribe/teardown, and `try_set`/`try_get_untracked` for any signal access that might race with disposal.
- **Fire-and-forget with no UI update** (e.g., analytics pings, mark-as-read calls with no UI feedback).

### Thread dispatch-time values through the Action return type

When converting `spawn_local` to `Action`, the `spawn_local` pattern captures values by closure at dispatch time. But the `Effect` watching `action.value()` fires at effect-evaluation time — if it reads live signals instead of values returned from the action, it can see post-edit values that the server never received.

**Rule:** Any value that was captured at dispatch time in the original `spawn_local` must either be part of the Action input or returned through the Action result. Never read live signals in the Effect for values that should reflect the dispatch-time state.

```rust
// WRONG — Effect reads live signals that may have changed during the async call
let save_action = Action::new(move |_: &()| {
    let title = title_signal.get_untracked();
    async move { save_to_server(title).await }
});
Effect::new(move |_| {
    if let Some(Ok(())) = save_action.value().get() {
        set_saved_title.set(title_signal.get());  // 💥 reads CURRENT value, not saved value
    }
});

// RIGHT — dispatch-time values returned through the action result
let save_action = Action::new(move |_: &()| {
    let title = title_signal.get_untracked();
    async move {
        save_to_server(&title).await?;
        Ok(title)  // return the value that was actually saved
    }
});
Effect::new(move |_| {
    if let Some(Ok(saved_title)) = save_action.value().get() {
        set_saved_title.set(saved_title.clone());  // uses the actual saved value
    }
});
```

### Use `action.pending()` as the double-dispatch guard

When the original `spawn_local` code used a manual `is_toggling` or `is_loading` signal to prevent concurrent dispatches, the `Action` equivalent is `action.pending().get_untracked()` in the dispatch callback — not dropping the guard entirely.

```rust
// WRONG — drops the concurrency guard when converting to Action
let toggle_action = Action::new(move |id: &String| { /* ... */ });
let handle_toggle = move |_| {
    toggle_action.dispatch(id.clone());  // no guard — double-click races
};

// RIGHT — Action's built-in pending() replaces the manual guard
let toggle_action = Action::new(move |id: &String| { /* ... */ });
let handle_toggle = move |_| {
    if !toggle_action.pending().get_untracked() {
        toggle_action.dispatch(id.clone());
    }
};
```

### Never mix signal lifetimes in `Signal::derive` without `try_get()`

A `Signal::derive` that subscribes to BOTH a long-lived signal (Layout-scoped, e.g. `SyncStore` data) AND a page-scoped signal (e.g. search/sort/filter) creates a disposal race. When the user navigates away, the page-scoped signals are disposed — but the Layout-scoped signal can still trigger re-evaluation of the derive (e.g. via a WebSocket sync update), causing it to call `.get()` on the disposed page signals → panic.

**Rule:** If a `Signal::derive` reads from signals with different lifetimes, use `.try_get()` for the shorter-lived ones. Return a sensible default (empty vec, default sort, etc.) if they're disposed.

```rust
// WRONG — derive subscribes to both sync_store (Layout) and query (page-scoped)
let filtered = Signal::derive(move || {
    let items = sync_store.all_items().get();    // Layout-scoped, lives forever
    let q = search_query.get();                  // 💥 page-scoped, may be disposed
    filter(items, q)
});

// RIGHT — try_get() for page-scoped signals, graceful fallback
let filtered = Signal::derive(move || {
    let items = sync_store.all_items().get();
    let q = search_query.try_get().flatten();    // None if disposed
    match q {
        Some(ref query) => filter(items, query),
        None => items,                           // unfiltered fallback
    }
});
```

**This is the root cause of most "reactive value already disposed" panics.** The previous 12+ tickets for this panic class were fixed one-by-one; this pattern prevents the entire class.

### Use `.try_set()` / `.try_update()` in ALL deferred execution contexts

Signal writes inside `spawn_local`, `spawn_scoped`, `Closure::new`, `set_timeout`, or any callback that outlives the reactive scope must use `.try_set()` / `.try_update()` instead of `.set()` / `.update()`. The same applies to `Callback::run()` — use `Callback::try_run()` when the callback is invoked inside an Action's async block or any deferred context, because the component that created the callback may have been unmounted. The user may navigate away before the callback fires, disposing the signal or stored value — `.set()` / `Callback::run()` panics, `.try_set()` / `Callback::try_run()` silently returns `false` / `None`.

**Rule:** Synchronous writes *before* a `spawn_local` or in `Effect::new` blocks are fine with `.set()` — the signal is guaranteed to be alive. Only deferred writes (inside the async block, inside a `.forget()`-ed Closure, inside a Timeout callback) need the `try_` variant. This is a belt-and-suspenders defense — the primary fix is to use `Action` or `spawn_scoped`, but `try_` methods catch any remaining edge cases.

```rust
// WRONG — panics if user navigates away before the fetch completes
spawn_local(async move {
    let result = fetch_data().await;
    loading.set(false);        // 💥 signal may be disposed
    data.update(|d| *d = result);
});

// RIGHT — deferred writes use try_ variants
spawn_local(async move {
    let result = fetch_data().await;
    loading.try_set(false);    // returns false if disposed, no panic
    data.try_update(|d| *d = result);
});

// ALSO RIGHT — synchronous write before spawn_local is safe
loading.set(true);             // signal is alive here, .set() is fine
spawn_local(async move { /* ... */ });
```

### WASM-only `#[cfg]` blocks must compile on the WASM target

Variables used inside `#[cfg(target_arch = "wasm32")]` blocks must be parameters of the enclosing function (or otherwise available in scope on WASM). `cargo check` on the native target silently skips the block body, so missing variables won't be caught until the WASM build.

**Rule:** After modifying any function that contains `#[cfg(target_arch = "wasm32")]`, verify with:
```bash
cargo check --target wasm32-unknown-unknown -p kyomi-ui --features hydrate
```

```rust
// WRONG — compiles on native, fails on WASM (datasource_type not in scope)
fn run_arrow_query(slug: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let dt = datasource_type.clone(); // E0425 on WASM
    }
}

// RIGHT — parameter is available on both targets
fn run_arrow_query(slug: &str, datasource_type: String) {
    #[cfg(target_arch = "wasm32")]
    {
        let dt = datasource_type.clone(); // works
    }
}
```

### Never reference `kyomi-core` from client-side code — it is an `ssr`-only dependency

`kyomi-ui` declares `kyomi-core` as `dep:kyomi-core` under the `ssr` feature only (`crates/kyomi-ui/Cargo.toml`); the `hydrate` feature does not enable it. Inside a `#[server]` function body this is fine — the body is stripped out of non-ssr builds, so the client never needs the crate. But any code that actually compiles to WASM (a `Memo`/`Signal::derive` closure, a `view!` closure, an event handler, a plain non-`#[server]` helper) that names `kyomi_core::` fails with `E0433: failed to resolve: use of undeclared crate or module` on the `wasm32-unknown-unknown` + `hydrate` build.

This is invisible locally: `cargo check -p kyomi-ui --features ssr` passes, so the break only surfaces in CI or on the trunk build.

**Rule:** `kyomi_core::` may only appear inside `#[server]` function bodies or `#[cfg(feature = "ssr")]` blocks. If client-side code appears to need a `kyomi-core` constant or enum, either restructure the expression so the value isn't needed, or compute the derived value server-side and put a plain type (`bool`, `String`) on the wire DTO. Never widen the `hydrate` feature to pull `kyomi-core` in.

```rust
// WRONG — memo runs on the client; E0433 on wasm32 + hydrate
let seat_capped = Memo::new(move |_| {
    subscription.get()
        .map(|info| info.user_limit.unwrap_or(UNLIMITED_USER_LIMIT) <= 1)
        .unwrap_or(false)
});

// RIGHT — same semantics, no server-only dependency on the client
let seat_capped = Memo::new(move |_| {
    subscription.get()
        .map(|info| info.user_limit.is_some_and(|limit| limit <= 1))
        .unwrap_or(false)
});

// ALSO RIGHT — typed enum stays server-side, client gets a plain bool on the DTO
#[server]
pub async fn catalog_stats() -> Result<CatalogStatsResult, ServerFnError> {
    let row: RefreshRow = /* decodes kyomi_core::enums::CatalogRefreshStatus */;
    Ok(CatalogStatsResult {
        refresh_failed: row.catalog_refresh_status == Some(CatalogRefreshStatus::Failed),
    })
}
```

Verify with `cargo check --target wasm32-unknown-unknown -p kyomi-ui --features hydrate` before pushing. Flagged in KYO-167 (cycle 3 — a `seat_capped` memo referencing `kyomi_core::capability::UNLIMITED_USER_LIMIT` broke CI) and re-checked as an explicit sign-off condition in KYO-169; the same constraint shaped the KYO-126 fix, where the typed `CatalogRefreshStatus` was kept out of the wire struct in favour of a server-computed `bool`.

### Never snapshot a `Signal` prop into a local variable — derive from it

Calling `signal.get()` on a prop and storing the result in a local `String` (or any non-reactive variable) creates a frozen snapshot. Wrapping that snapshot in a new `Signal::stored` or `create_signal` doesn't restore reactivity — it just adds a layer of indirection over dead data. Use `Signal::derive(move || original_signal.get())` to keep the derived value reactive.

**Rule:** If a component receives a `Signal<T>` prop and needs to pass a derived value to a child, use `Signal::derive` closing over the original signal — never `.get()` into a local and re-wrap.

```rust
// WRONG — captures a snapshot, child never sees updates
let val = signal_prop.get();            // snapshot at mount time
let val_sig = Signal::stored(val);      // wraps the dead snapshot
// child sees the initial value forever

// RIGHT — derive keeps reactivity alive
let derived = Signal::derive(move || signal_prop.get());
// child re-renders when signal_prop changes
```

### Resolve derived signal values at click time, not render time

When a reactive closure (`{move || ...}`) builds a button whose `on:click` opens a popup or navigates to a URL derived from signals, the URL must be resolved inside the click handler — not captured into the closure's scope at render time. The outer closure re-runs when its tracked signals change, but intermediate signal values (like a `slug`-derived URL) may update independently without re-triggering the closure.

**Rule:** Use `signal.get_untracked()` inside `on:click` handlers for values that should reflect the current state at interaction time.

```rust
// WRONG — URL captured at render time, stale if slug changes
let connect_url_val = connect_url.get(); // captured when closure runs
view! {
    <button on:click=move |_| {
        open_oauth_popup(&connect_url_val); // uses stale value
    }>"Connect"</button>
}

// RIGHT — URL resolved at click time
view! {
    <button on:click=move |_| {
        let url = connect_url.get_untracked(); // fresh value at click time
        open_oauth_popup(&url);
    }>"Connect"</button>
}
```

### Never read signals eagerly inside `ChildrenFn` / `Arc<dyn Fn() -> AnyView>` closures that share scope with inputs

If a `ChildrenFn` closure (used by `Modal` footer, `Transition` fallback, etc.) reads a signal with `.get()`, every signal change re-executes the entire closure and rebuilds its DOM. If the Modal/component re-renders children alongside the footer, this destroys any `<input>` elements in the body — causing focus loss on every keystroke.

**Rule:** Never call `.get()` on a signal directly inside a `ChildrenFn` closure if that signal is also written by an `<input>` in the component's body. Instead, use `Signal::derive` to create fine-grained derived signals that only update the specific prop (e.g. `disabled`) without rebuilding the DOM.

```rust
// WRONG — reads transfer_confirmation.get() in the footer closure.
// Every keystroke in the input rebuilds the footer, which triggers
// the Modal to re-call children(), destroying the input.
let footer: Arc<dyn Fn() -> AnyView> = Arc::new(move || {
    let conf = transfer_confirmation.get(); // ← causes full rebuild
    let disabled = conf != workspace_name.get();
    view! { <Button disabled=disabled>"Submit"</Button> }.into_any()
});

// RIGHT — Signal::derive creates a fine-grained subscription.
// Only the button's `disabled` attribute updates, no DOM rebuild.
let footer: Arc<dyn Fn() -> AnyView> = Arc::new(move || {
    let disabled = Signal::derive(move || {
        transfer_confirmation.get() != workspace_name.get()
    });
    view! { <Button disabled=disabled>"Submit"</Button> }.into_any()
});
```

**General principle:** Reactive closures that rebuild DOM (`{move || ...}` in views, `ChildrenFn`, `Arc<dyn Fn() -> AnyView>`) must never contain `<input>` elements AND read the signal that input writes. Either:
1. Move the input outside the reactive closure (into static view structure)
2. Use `Signal::derive` so the signal subscription is scoped to a leaf attribute, not the whole closure

### Never call `.get()` on signals inside `<Show>` children — pass Signals as props

In Leptos 0.7, `<Show>` wraps children in a reactive closure: `move || if when() { children() } else { fallback }`. Any `.get()` call inside `children()` subscribes the **parent reactive scope** to that signal. When the signal changes, the parent scope re-runs, calling `children()` again — destroying and recreating all child components, even if the `when` condition hasn't changed. This causes visible DOM flashing and destroys internal component state (timers, scroll position, expanded/collapsed state).

**Rule:** Never call `.get()` on signals inside a `<Show>` children block. Instead, create `Signal::derive` closures *outside* the `<Show>` and pass them as `Signal<T>` props to child components. If the child component only accepts plain values (not Signals), refactor it to accept `Signal<T>` props so it can be created once and reactively update.

```rust
// WRONG — .get() inside <Show> children subscribes parent scope to thinking_state.
// Every thinking_state change destroys and recreates AgentThinking.
<Show when=should_show_thinking>
    {
        let events = thinking_events_fn();  // calls thinking_state.get()
        let active = is_active_fn();        // calls thinking_state.get()
        view! { <AgentThinking thinking_events=events is_active=active /> }
    }
</Show>

// RIGHT — Signal::derive created outside <Show>, passed as Signal props.
// AgentThinking is created once and reactively updates without remounting.
let events_signal = Signal::derive(thinking_events_fn);
let active_signal = Signal::derive(is_active_fn);
<Show when=should_show_thinking>
    <AgentThinking thinking_events=events_signal is_active=active_signal />
</Show>
```

Flagged in KYO-38 rework — the `AgentThinking` component flashed on every tool call because `thinking_state.get()` inside `<Show>` children caused full component re-creation on each thinking event.

### Use `<Show>` for conditional component rendering, not reactive closure branches

When a reactive closure (`{move || { ... }.into_any()}`) conditionally renders a component that owns its own reactive scope (e.g., `DynSelect` → `Popover` → `Effect::new`), the branch switch destroys and recreates the component's internal signals. If the component's `Effect` fires during disposal, it accesses dead signals → panic. The Leptos `<Show>` component handles component lifecycle correctly — it mounts/unmounts through the framework's ownership tree.

**Rule:** Never gate a component with internal reactive state (popover, modal, effect-owning widget) inside a `{move || ...}` view closure. Use `<Show when=condition>` with a `fallback` instead.

```rust
// WRONG — reactive closure branch creates/destroys DynSelect (which owns Popover/Effect)
view! {
    {move || {
        if has_projects.get() {
            view! { <DynSelect options=projects /> }.into_any()
        } else {
            view! { <input type="text" /> }.into_any()
        }
    }}
}

// RIGHT — <Show> manages component lifecycle safely
view! {
    <Show when=move || has_projects.get()
          fallback=move || view! { <input type="text" /> }>
        <DynSelect options=projects />
    </Show>
}
```

Flagged in KYO-14 review — four `DynSelect` instances gated by reactive closures caused disposal panic risk on OAuth connect.

### Add `Effect::new` re-fetch when auth mode toggles in an open modal

When a datasource modal supports multiple auth modes (e.g. `service_account` / `kyomi_oauth` / `enterprise_oauth`), the OAuth status panel must re-fetch status when the user switches modes. Without an `Effect::new` subscribing to the auth mode signal, the panel shows stale data from the previously-fetched mode.

**Rule:** Any `AuthModeSection` component that displays OAuth status must include an `Effect::new` that subscribes to the auth mode signal, resets state to "disconnected" while in-flight, and fetches the correct status for the new mode. Skip the fetch for modes that don't use OAuth (e.g. `service_account`).

```rust
// WRONG — status fetched only on modal open, not on mode switch
// User switches from kyomi_oauth (connected) to enterprise_oauth (not configured)
// → panel still shows "Connected: user@example.com" from the kyomi_oauth fetch

// RIGHT — Effect re-fetches on mode change
Effect::new(move |_| {
    let mode = bq_auth_mode.get();
    let current_slug = slug.get();
    if mode == "service_account" || current_slug.is_empty() { return; }
    set_oauth_connected.set(false);
    set_oauth_email.set(String::new());
    set_oauth_expired.set(false);
    spawn_local(async move {
        // fetch status for the correct mode
        let result = match mode.as_str() {
            "kyomi_oauth" => get_google_oauth_status().await,
            "enterprise_oauth" => get_datasource_oauth_status(/*...*/).await,
            _ => return,
        };
        // update signals from result...
    });
});
```

This pattern was independently flagged in KYO-13 (BigQuery) and KYO-17 (Databricks) reviews. The BigQuery implementation is the canonical template.

## Email Templates

*Standards for HTML email templates (alert.rs, email_service.rs, feedback_service.rs, analytics_notifications.rs).*

### Never put `color:` in inline styles on elements that need dark mode overrides

Per the CSS spec, `!important` rules in `<style>` blocks DO override inline styles. However, for email templates, the practical rule is simpler: keep `color` values out of inline styles entirely.

**Why:** Consistency and maintainability. With `color` only in class/element rules, you have one place to update for light mode and one place for dark mode. Inline `color` is never needed because email clients that strip `<style>` blocks (Gmail) still render readable text with browser defaults. This is unlike `background-color` on containers (see next rule).

**Rule:** For any text element, put `color` values in class-based CSS rules only, not inline. Inline styles should only contain layout properties (`padding`, `margin`, `font-size`, `font-weight`, `vertical-align`, `text-align`).

```html
<!-- WRONG — inline color is unnecessary; Gmail renders readable text without it -->
<td style="color:#1C1917; padding:12px 16px;">Label</td>
<!-- Removing it lets td { color: #A8A29E !important; } apply cleanly in dark mode -->

<!-- RIGHT — color comes from class rule, inline has only layout -->
<td style="padding:12px 16px;">Label</td>
<!-- Light mode: td { color: #1C1917; } applies -->
<!-- Dark mode: td { color: #A8A29E !important; } overrides -->
```

This applies to dynamically-built rows too (e.g. `push_str` loops building `<td>` elements). The pattern was missed twice in KYO-6 — once in `alert.rs` and once in `feedback_service.rs` — because dynamic row construction happened before the `html_body` template and was not updated alongside static rows.

### Inline `background-color` on containers IS correct for email dark mode

Unlike text `color`, structural elements (`<tr>`, `<td>` containers, content cards) SHOULD have inline `background-color` for light mode. This is the standard email dark mode pattern:

- Inline `background-color` provides the light-mode fallback for clients that strip `<style>` (Gmail)
- `@media (prefers-color-scheme: dark) { tr { background-color: #1A1715 !important; } }` overrides it in clients that support dark mode (Apple Mail, iOS Mail)

Per the CSS Cascading and Inheritance spec, `!important` author stylesheet declarations outrank normal inline `style=""` attributes. This was confirmed in the KYO-8 review dispute (2026-05-17).

```html
<!-- CORRECT — inline bg for Gmail fallback, !important dark override works -->
<tr style="background-color: #FAFAF8;">
  <td style="padding: 12px 16px;">Label</td>
</tr>
<!-- Dark mode: tr { background-color: #1A1715 !important; } wins over inline -->
```

**Do NOT confuse this with the `color:` rule above.** Text `color` should never be inline because it's not needed for Gmail fallback (browser defaults handle text). Container `background-color` is inline because without it, Gmail renders a transparent/white background regardless of your design.

## API & Server Functions

*Standards for server functions, REST endpoints, and the boundary between frontend and backend.*

### Never reimplement server-owned URL/routing logic on the client

OAuth connect URLs, callback URLs, and API endpoint paths are owned by the server. Client-side functions that reconstruct these URLs by pattern-matching on provider strings duplicate logic that `get_oauth_connect_url` (or equivalent server_fn) already handles correctly — and inevitably miss edge cases (e.g. BigQuery `enterprise_oauth` needing a different endpoint than `kyomi_oauth`).

**Rule:** Call the server_fn that owns the URL logic. If no server_fn exists for the use case, create one. Never build API URLs client-side from provider/mode strings.

```rust
// WRONG — client reimplements URL routing, misses enterprise_oauth branch
fn oauth_url_for_datasource(provider: &str, slug: &str) -> String {
    match provider {
        "bigquery" => "/api/v1/auth/google-oauth/connect".to_string(),
        // missing: enterprise_oauth needs /api/v1/auth/oauth/bigquery-enterprise/connect?datasource_slug=...
        "snowflake" => format!("/api/v1/auth/oauth/snowflake/connect?datasource_slug={slug}"),
        _ => String::new(),
    }
}

// RIGHT — server_fn owns the routing
let url = get_oauth_connect_url(provider.clone(), slug.clone(), auth_mode).await?;
```

Flagged in KYO-12 review — `oauth_url_for_datasource` silently routed enterprise BigQuery users to the wrong OAuth endpoint.

### Server functions must call service-layer functions directly — never HTTP loopback

A `#[server]` function runs inside the same process as the axum route handlers. Calling an HTTP endpoint from a server fn (e.g. `reqwest::get("http://localhost:PORT/api/v1/...")`) is a loopback anti-pattern that:
- Creates fragile port/path coupling (wrong prefix = silent 404)
- Bypasses service-layer error typing (HTTP responses must be re-parsed)
- Adds latency and failure modes (TCP connection to self)
- Diverges from every other server fn in the codebase

**Rule:** Extract shared logic into a service function in the appropriate crate (`kyomi-auth`, `kyomi-core`, etc.). Both the REST route handler and the server fn call the same service function directly. If the service function doesn't exist yet, create it — don't shortcut through HTTP.

```rust
// WRONG — HTTP loopback from server fn to own process
#[server]
pub async fn link_google_account(code: String) -> Result<(), ServerFnError> {
    let resp = reqwest::get(format!("http://localhost:8001/api/v1/auth/google/link-callback?code={code}"))
        .await?;
    // fragile: wrong path = 404, must parse HTTP response, port coupling
}

// RIGHT — both callers use the same service function
#[server]
pub async fn link_google_account(code: String) -> Result<LinkResult, ServerFnError> {
    let pool = extract_pool().await?;
    let result = google_link_callback_service(&pool, &params).await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(result)
}
```

## Data & State Management

*Standards for database access, caching, state synchronization, and data flow.*

*(No entries yet.)*

## Security

*Standards for encryption, authentication, credential handling, and input validation.*

### `workspace_id` is not an authorization boundary for `dashboards` reads

`dashboards` rows are visible to a user only if they own the row or it belongs to a public collection — that check lives in `kyomi_auth::dashboard_service::visibility_predicate`. A query that filters on `workspace_id` alone returns every member's private documents, not just the requesting user's.

**Rule:** Every query that reads `dashboards` rows on behalf of a user must apply `visibility_predicate`, not just the user-facing list/search endpoints. This includes anything feeding the LLM system prompt or tool output, background jobs, and sync/export paths — any consumer that renders titles, content, or metadata back to a specific user or agent turn scoped to a user.

Gating the row is not sufficient on its own: any `JOIN`ed metadata pulled alongside a visible row — most notably a dashboard's `collections` name via `collection_dashboards` — needs its own independent visibility check (mirror `collection_service::list_collections`'s `created_by = $user OR is_public` rule in the `JOIN`'s `ON` clause, not the `WHERE`). A document can be visible through one collection membership while also belonging to a second, invisible one; without gating the join itself, `ORDER BY`/dedup logic can surface the invisible collection's name for a document the viewer is otherwise allowed to see.

Flagged in KYO-182: the agent's system-prompt document list (`build_documents_text`) selected every dashboard in the workspace with only `WHERE d.workspace_id = $1`, so every member's private document titles, collection names, and update times were injected into every other member's chat and freely recited by the agent — while `search_dashboards` and the dashboards page were correctly filtered. The first fix pass gated which documents appeared but left the joined collection name unfiltered, still leaking a private collection's name for any document that also had a visible membership. Same root cause as KYO-172 (sync-engine leak): a code path that reads `dashboards` (and its joined metadata) outside the shared visibility check.

## Code Organization

*Standards for module structure, imports, shared utilities, and avoiding duplication.*

### Use `OnceLock<Regex>` for repeated regex patterns

If a `Regex::new(...)` pattern appears more than once in the codebase (or is called in a hot path), extract it into a `static OnceLock<Regex>`. Compiling the same regex repeatedly wastes CPU and invites copy-paste drift when the pattern needs updating.

**Rule:** Before writing `Regex::new(...)` inline, grep for the pattern string. If it already exists elsewhere, extract both into a shared static. New regex patterns that will be called more than once should start as statics.

```rust
use std::sync::OnceLock;
use regex::Regex;

// WRONG — same pattern compiled in 4 different call sites
fn find_chartml_block(content: &str) -> Option<&str> {
    let re = Regex::new(r"(?s)```chartml\s*\n(.*?)```").unwrap();
    re.captures(content).map(|c| c.get(1).unwrap().as_str())
}

// RIGHT — compiled once, shared across all call sites
fn chartml_fence_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)```chartml\s*\n(.*?)```").unwrap())
}

fn find_chartml_block(content: &str) -> Option<&str> {
    chartml_fence_regex().captures(content).map(|c| c.get(1).unwrap().as_str())
}
```

### Preserve side-effect and error ordering when refactoring

When extracting a helper or reordering validation in a "no behavior change" refactor, the *observable* behavior includes more than HTTP status and body — it includes **which side effects run** and **in what order errors are produced**. Moving a fallible or side-effecting step earlier changes behavior for the combined-failure input (e.g. bad datasource slug AND missing encryption key now surfaces the encryption error first), and moving a side-effecting call (network, DB write) ahead of an early-return guard causes it to run on paths that previously short-circuited.

**Rule:** When you relocate a validation, guard, or context-building step, trace every input class that could fail at more than one point and confirm the *first* error and the *set of side effects* are unchanged. If a helper needs a value that is expensive or side-effecting to compute, pass it lazily (a closure invoked past the resolve/guard step) rather than eagerly at the call site.

```rust
// WRONG — build_user_context() does a network token refresh + DB write;
// running it before resolve means an invalid slug now triggers that side effect
let user_context = build_user_context(&state, &user).await?;
let ds = resolve_datasource(&db, &slug).await?; // was first before the refactor

// RIGHT — resolve/guard first, build the side-effecting context only after
let ds = resolve_datasource(&db, &slug).await?;
let user_context = build_user_context(&state, &user).await?;
```

Flagged repeatedly in KYO-195/196 reviews (behavior-change-in-refactor): eager `user_context` construction and an encryption-key check both moved ahead of `resolve_datasource`, changing side effects and first-error for invalid-slug inputs.

## String & Text Processing

*Standards for safe string manipulation in Rust.*

### Never byte-slice strings for truncation — use `chars().take(N)`

Rust `&str[..N]` is a byte slice. If `N` falls in the middle of a multi-byte UTF-8 character, the program panics at runtime. This is easy to miss because it works fine with ASCII test data and only fails in production when users enter non-ASCII characters (accented names, emoji, CJK text).

**Rule:** Always use `.chars().take(N).collect::<String>()` for truncation. If the pattern already exists in the same file, use it consistently.

```rust
// WRONG — panics on non-ASCII content at a multi-byte boundary
let preview = if content.len() > 200 { &content[..200] } else { content };

// RIGHT — safe on any UTF-8 content
let preview: String = content.chars().take(200).collect();
```

Flagged in KYO-85 review — two call sites in `dashboard_service.rs` used byte-slicing while a third site in the same file already used the safe `chars().take()` pattern.

## Testing

*Standards for test structure, assertions, and what must be tested.*

### Every bug fix / behavior change ships with a test asserting the exact changed behavior

A fix is not done until a test locks in the precise behavior the ticket changed — not "related functionality," the exact assertion. If the target module has no `#[cfg(test)] mod tests`, add one; these are usually one-liners.

**Rule:** Before calling a fix complete, ask "what one assertion, if it regressed, would silently reintroduce this bug?" and write that test. A `Display`-format fix gets `assert_eq!(Error::X(msg).to_string(), msg)`; a security/validation fix gets one test per rejection path (wrong user, expired, wrong state); a query-identity fix asserts the row count / archived count directly.

Flagged repeatedly in review: KYO-145 (missing `Display` prefix-free assertion blocked sign-off until added), KYO-143 (new status-mapping branch shipped untested), KYO-140 (exemplary — the archival fix shipped with a test running the real query template against an in-memory pool asserting `tables_archived == 0`).
