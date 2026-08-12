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

### Enforcement status — read this before trusting any rule below

Every anti-pattern in this section carries an **Enforcement:** line stating whether CI will catch a violation. There are **three** tiers, not two, and only **one** of the six patterns actually blocks a merge:

| Pattern | Enforcement |
|---|---|
| Bare `.set()` / `.update()` in deferred contexts | **blocking** — `scripts/lint/check-disposal-safety.sh` Rule A; fails CI |
| Bare `.get()` in `Signal::derive` / `Memo::new` | **advisory** — same script, Rule B; prints `WARN:B` but **exits 0** |
| Raw `spawn_local` for user-triggered mutations | **review-only** |
| `.get()` inside `<Show>` children | **review-only** |
| Reactive closure branches gating effect-owning components | **review-only** |
| Eager signal reads in `ChildrenFn` / `Arc<dyn Fn() -> AnyView>` | **review-only** |

**Do not read "the disposal-safety lint covers this" as "CI will stop me."** Only Rule A does. The script's exit logic (`case "$line" in *:WARN*) ;;`) deliberately excludes `WARN`-tagged findings from the failure path, so a Rule B hit is reported and ignored. As of 2026-07-26 the tree carries **422** live `WARN:B` findings and the lint still exits 0 — which is itself the proof that Rule B is not a gate.

Rule B is advisory *by design*, not by accident: it cannot distinguish a derive that genuinely mixes Layout-scoped and page-scoped signals from one that only reads same-scope signals, so its false-positive rate is high (of the candidates inspected during the 2026-07-25 sweep, all were false positives). Gating on it would fail every build. Making it blocking requires the same syntax-tree awareness the four review-only patterns need — see below.

**Why the four have no tooling at all.** They are *structural*: catching them requires knowing where an expression sits in the syntax tree, which the existing pure-bash-and-awk lint cannot do. `.get()` inside a `<Show>`'s **children** is a bug; the identical token inside its `when=` prop is correct and ubiquitous — a proximity grep over `<Show` returns 221 hits, nearly all legitimate. Likewise, `spawn_local` in an `on:click` handler is a bug, but in a WebSocket handler or a `!Send` browser-API call it is explicitly sanctioned. A regex rule here would be noisy enough to get suppressed, which is worse than no rule.

An AST-aware linter (Dylint or a clippy plugin) *could* enforce them, and was evaluated and declined: it requires a pinned nightly toolchain, which the repo does not currently have — there is no `rust-toolchain.toml` and CI runs `dtolnay/rust-toolchain@stable`. That is a new ongoing maintenance commitment, judged not worth it for these four patterns.

**The cost of that trade, recorded honestly so it can be reopened with data rather than re-argued:**

- *Where blocking, the class is dead.* A 2026-07-25 sweep of `crates/kyomi-ui/src` found 132 `spawn_local` blocks containing 318 guarded `try_set`/`try_update` calls and **zero** unguarded ones. Before Rule A existed, this panic class took 12+ tickets fixed one at a time. Rule A is the only pattern here with that record, and it is the only blocking one.
- *Where review-only, it is not.* The `Effect` auth-mode pattern was documented after being caught twice (KYO-13, KYO-17) and still went missing a fourth time in `SynapseAuthModeSection` (KYO-197). KYO-226 and KYO-227 then found **28** raw `spawn_local` user-triggered mutations across 10 files — i.e. the pattern this document calls "the #1 source of WASM panics" is precisely the one with no gate.

If that second count keeps climbing, revisit the Dylint decision.

*Numbers above are point-in-time measurements from the dates given, not continuously verified. Re-measure before relying on them for a decision.*

### Never use raw `spawn_local` for user-triggered mutations — use `Action`

**Enforcement: review-only.** No lint catches this — distinguishing a user-triggered mutation from a sanctioned `spawn_local` needs AST awareness. See *Enforcement status* above.

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

**Enforcement: advisory — this does NOT fail the build.** `scripts/lint/check-disposal-safety.sh` Rule B prints a `WARN:B` line for a bare `.get()` inside `Signal::derive` / `Memo::new`, but `WARN`-tagged findings are excluded from the script's failure path, so CI exits 0 regardless. Rule B cannot tell a genuinely mixed-lifetime derive from a same-scope one, so it is intentionally non-blocking. Treat it as a prompt to check the derive yourself, not as a guarantee. See *Enforcement status* above.

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

### `Signal::derive` is not memoized — use `Memo` when the body does more than read

`Signal::derive` wraps a plain `Fn() -> T`: it re-runs its body on *every* read. So a reactive closure that reads a derive alongside an unrelated signal re-runs the derive's body whenever that unrelated signal changes, not just when the derive's own dependencies do. `Memo` caches its value and recomputes only when the signals it actually reads change. If the derived body is pure and cheap this is invisible — but if it logs, records a metric, or does real work, the re-runs are observable and wrong.

**Rule:** If a derived value's body has a side effect or is expensive, define it with `Memo::new` scoped to only the signals it reads, and have callers read the memo. Reserve `Signal::derive` for cheap, pure projections. Never place a `warn!`/`error!` inside a closure that a multi-resource reactive scope reads — the log stops meaning "this failed" and starts meaning "something nearby re-rendered."

```rust
// WRONG — the closure also reads `members`, so every members refetch re-runs
// current_user_id_from and re-emits its warn! for a failure that already happened
let is_owner = move || {
    let id = current_user_id_from(user_ctx.get()); // contains warn! on the Err path
    members.get().iter().any(|m| m.user_id == id && m.role == "owner")
};

// RIGHT — Memo scoped to user_ctx; the body (and its log) runs once per real change
let current_user_id = Memo::new(move |_| current_user_id_from(user_ctx.get()));
let is_owner = move || {
    let id = current_user_id.get();
    members.get().iter().any(|m| m.user_id == id && m.role == "owner")
};
```

Flagged as 🟡 in KYO-240 (2026-08-03): once the `UserContext` fetch failed, every later team action — remove member, role change, initiate/cancel transfer — bumped `members_version`/`transfers_version`, re-ran the enclosing closure, and re-logged "user context fetch failed", so one stale failure read as a stream of fresh ones. The identical shape one function away (`is_owner_from` re-awaiting a cached `Err`) was ticketed as KYO-304.

### Use `.try_set()` / `.try_update()` in ALL deferred execution contexts

**Enforcement: blocking.** `scripts/lint/check-disposal-safety.sh` Rule A catches bare `.set()` / `.update()` inside `spawn_local` and other deferred callbacks, and **fails CI**. This is the only pattern in this section that stops a merge. Escape hatch, requiring a ≥5-character justification: `// lint-allow: disposal-safe=<why>`. See *Enforcement status* above.

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

**Enforcement: review-only.** No lint catches this — it requires knowing that a `.get()` sits inside a `ChildrenFn` body. See *Enforcement status* above.

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

**Enforcement: review-only.** No lint catches this — `.get()` in a `<Show>`'s `when=` prop is correct while the same call in its children is a bug, and only an AST can tell them apart. See *Enforcement status* above.

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

**Enforcement: review-only.** No lint catches this — it requires knowing that the component inside a reactive branch owns a reactive scope. See *Enforcement status* above.

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

### Use the shared OAuth-status re-fetch hook when auth mode toggles

When a datasource modal supports multiple auth modes (e.g. `service_account` / `kyomi_oauth` / `enterprise_oauth`), the OAuth status panel must re-fetch status when the user switches modes. Without this, the panel shows stale data from the previously-fetched mode.

**Rule:** Any new provider's `*AuthModeSection` component must call the shared `use_oauth_status_refetch` hook, passing a mapping fn (`fn(&str) -> Option<OAuthStatusSource>`) that resolves the current auth mode to its OAuth status source; modes that don't use OAuth (e.g. `service_account`) map to `None`. Do not hand-roll another `Effect::new` for this — the `auth_mode_sections_do_not_hand_roll_oauth_status_effects` guard test fails the build if one is added.

```rust
// WRONG — hand-rolled Effect, now a guard-test failure
Effect::new(move |_| {
    let mode = bq_auth_mode.get();
    let slug_val = slug.get();
    if mode == "service_account" || slug_val.is_empty() { return; }
    set_oauth_connected.set(false);
    set_oauth_email.set(None);
    set_oauth_expired.set(false);
    spawn_local(async move {
        // fetch status for the correct mode...
    });
});

// RIGHT — shared hook + a per-provider mapping fn
fn bigquery_oauth_source(mode: &str) -> Option<OAuthStatusSource> {
    match mode {
        "kyomi_oauth" => Some(OAuthStatusSource::GoogleAccount),
        "enterprise_oauth" => Some(OAuthStatusSource::Datasource("bigquery-enterprise")),
        _ => None,
    }
}

use_oauth_status_refetch(
    bq_auth_mode,
    slug,
    OAuthStatusSetters {
        connected: set_oauth_connected,
        email: set_oauth_email,
        expired: set_oauth_expired,
    },
    bigquery_oauth_source,
);
```

This pattern was independently flagged in KYO-13 (BigQuery) and KYO-17 (Databricks) reviews, and recurred a third time in Synapse (KYO-197) because each fix copy-pasted the Effect instead of sharing it. That third recurrence is what motivated extracting `use_oauth_status_refetch`.

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

### Tightening a column constraint requires auditing every write site for that table

A migration that adds `NOT NULL`, a `UNIQUE`, or a foreign key does not fail at migration time when the offending write lives in Rust — it fails later, at the first `INSERT` that omitted the column and previously relied on a permissive default. If that write happens during server startup, the failure is a boot-loop: the migration has already committed, so every restart re-runs the same crashing code path.

**Rule:** Before shipping a migration that tightens a constraint, grep for every `INSERT INTO <table>` and `UPDATE <table>` across `apps/`, `crates/`, *and* the sibling repos (`~/repos/kyomi-connect` in particular), and confirm each one supplies the newly-required column on **both** the Postgres and SQLite branches. Reviewing the migration file and its own test is not sufficient coverage — the regression is at the call sites, not in the DDL.

Flagged in KYO-293: `00033_fix_collections_created_by_constraints.sql` made `collections.created_by` `NOT NULL`, but `kyomi_knowledge::unify::migrate_folders_to_collections` still ran `INSERT ... INTO collections` without `created_by` (previously absorbed by SQLite's `DEFAULT ''`). The resulting `RowNotFound` propagated through `?` into `main.rs`'s `.expect(...)`, killing the server process on every boot for any self-hosted SQLite install with an un-migrated folder row. Postgres had carried the same latent omission since `20260609000000_add_created_by_to_collections.sql`.

## Security

*Standards for encryption, authentication, credential handling, and input validation.*

### `workspace_id` is not an authorization boundary for `dashboards` reads

`dashboards` rows are visible to a user only if they own the row or it belongs to a public collection — that check lives in `kyomi_auth::dashboard_service::visibility_predicate`. A query that filters on `workspace_id` alone returns every member's private documents, not just the requesting user's.

**Rule:** Every query that reads `dashboards` rows on behalf of a user must apply `visibility_predicate`, not just the user-facing list/search endpoints. This includes anything feeding the LLM system prompt or tool output, background jobs, and sync/export paths — any consumer that renders titles, content, or metadata back to a specific user or agent turn scoped to a user.

Gating the row is not sufficient on its own: any `JOIN`ed metadata pulled alongside a visible row — most notably a dashboard's `collections` name via `collection_dashboards` — needs its own independent visibility check (mirror `collection_service::list_collections`'s `created_by = $user OR is_public` rule in the `JOIN`'s `ON` clause, not the `WHERE`). A document can be visible through one collection membership while also belonging to a second, invisible one; without gating the join itself, `ORDER BY`/dedup logic can surface the invisible collection's name for a document the viewer is otherwise allowed to see.

Flagged in KYO-182: the agent's system-prompt document list (`build_documents_text`) selected every dashboard in the workspace with only `WHERE d.workspace_id = $1`, so every member's private document titles, collection names, and update times were injected into every other member's chat and freely recited by the agent — while `search_dashboards` and the dashboards page were correctly filtered. The first fix pass gated which documents appeared but left the joined collection name unfiltered, still leaking a private collection's name for any document that also had a visible membership. Same root cause as KYO-172 (sync-engine leak): a code path that reads `dashboards` (and its joined metadata) outside the shared visibility check.

### A helper that looks like a security control but has no caller is worse than no helper

`mask_credentials(&value, ds_type)` reads like the thing that stops secrets reaching an API response, and the registry arrays it consults (`sensitive_credential_fields`, `AuthModeConfig.sensitive_fields`) read like the configuration that drives it. Nothing calls any of it outside its own tests. The harm is not the missing call — it is that the function's existence answers "is this masked?" for the next engineer, who then builds a new datasource-listing endpoint without masking because it looks handled. A test that pins the array's contents makes the whole apparatus look wired up and load-bearing while proving only that a constant equals itself.

**Rule:** Before treating a function or a registry array as a security control — and before accepting a ticket that says one is broken — grep for its production callers (`grep -rln "<name>" crates apps`) and name the live path that reaches it. If there are none, say so plainly: the change is a metadata-consistency fix, not a fix to a live exposure, and neither the PR nor the ticket may describe it as one. Metadata no code reads should be deleted or wired up, not silently corrected in place; if it must survive, back it with a check that executes the consumer rather than a test that pins the value.

Flagged as 🔴 in KYO-330 (2026-08-08): `crates/kyomi-auth/src/credential_service.rs:40` has zero production callers, so the ticket's premise ("any API response that runs Snowflake credentials through `mask_credentials` returns the PEM private key unmasked") described a path that does not exist. The actual live return path, `get_datasource_settings_detail` → `client_safe_user_settings` (`datasource_service.rs:1603-1613`), is an unrelated hardcoded default-deny allowlist that already excluded `private_key`, added a month earlier by `1949b555c` (KYO-130). The ticket was re-titled and de-prioritised rather than shipped as a security fix, and KYO-332 tracks deleting or wiring the inert metadata. The same week, KYO-274 removed a Synapse `oauth` `AuthModeConfig` entry that no code consumed — the UI never offered it and `synapse_oauth_source` had no arm for it, so it would have silently fallen through to the SQL username/password branch had anyone selected it.

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

### The third copy of a test helper is the extraction trigger — not the second

Two independent copies of a test helper can be justified ("different crate, not worth a shared dependency"). A third copy means the justification was wrong: the helper is general, and the copies will drift. By the third copy, each one has usually already been reviewed and approved individually, so nobody sees the aggregate.

**Rule:** Before writing a test helper, grep for its distinctive symbol names across `crates/` and `apps/`. If two copies already exist, extract all of them into a shared test-support crate (`kyomi-test-tracing`, `kyomi-test-harness`) in the same change rather than adding a third. If you decide against extracting, you must record the reasoning on the tracking ticket — a fresh "it's not worth it" comment in the new file, re-deriving a justification an earlier ticket already evaluated, is what makes the duplication invisible.

```rust
// WRONG — third inline copy, with a comment re-deriving the same justification
// as the two existing copies, and no reference to the ticket tracking them
struct CaptureLayer { /* ... */ }
struct EventLog { /* ... */ }

// RIGHT — one implementation, three consumers
use kyomi_test_tracing::capture_tracing;
let logs = capture_tracing();
assert!(logs.events_at(Level::ERROR).is_empty());
```

Flagged in KYO-240 cycle 1: the PR added a third `CaptureLayer`/`EventLog` copy after `kyomi-auth/src/mcp_session_manager.rs` and `apps/server/src/routes/auth_passkeys.rs`, which is the exact trigger condition KYO-244 had already written down. The extraction into `crates/kyomi-test-tracing` in cycle 2 was accepted as in-scope precisely because it was a direct response to a finding on that PR. The same class is still open elsewhere: duplicated SQLite fixture helpers in `kyomi-slack`, `extract_between` in two settings test modules, and per-module seeding helpers in `kyomi-auth`.

## Comments & Documentation

*Standards for what comments may claim, and keeping those claims true.*

### A comment must describe this code — never mirror another file

A comment of the form `Mirrors POST /x in apps/server/src/routes/y.rs` states a fact about a *different* file. It rots the instant that file changes, and it rots silently when the file is deleted: nothing type-checks a path inside a `///`. It also carries no information a reader of *this* function needs.

**Rule:** Write what the function does, enforces, or returns. If parity with another implementation is genuinely load-bearing, say what the shared property *is* rather than pointing at a path — and only when the other file is still live. When a comment's only content is a comparison, deleting it loses nothing.

```rust
// WRONG — claims a fact about a file deleted in 13e957e1
/// Mirrors POST /api/v1/auth/logout in apps/server/src/routes/auth.rs.
pub async fn logout() -> Result<(), ServerFnError> { /* ... */ }

// RIGHT — states what this function actually does
/// Steps:
/// 1. Revoke the refresh-token family.
/// 2. Clear the session cookies.
/// 3. Return regardless of whether a session existed.
pub async fn logout() -> Result<(), ServerFnError> { /* ... */ }
```

Flagged across all three groups of KYO-239 (+82/-131 of pure comment removal across 18 files, all pointing at routes deleted in `13e957e1`/`60e6f56c`/`0f9390b5`). KYO-302 exists because the informal `matches the REST handler` phrasing escapes that sweep's audit regex.

### A doc comment that outlived its behavior is a defect, not a nit

When behavior deliberately changes, the doc comment above it becomes an active lie — and it is more dangerous than no comment, because the next reader (or agent) trusts it over the code. Tests written against the old comment fail, and the failure looks like a bug in the code rather than a stale contract.

**Rule:** Changing a route's or function's observable contract means updating its doc comment in the same commit. When you find a stale one, correct it and cite the ticket that changed the behavior, so the next reader can see *when* the contract moved rather than guessing which of the two is current.

Flagged in KYO-256: `apps/server/src/routes/mcp.rs` documented `present-and-invalid returns 404 (forces re-initialize)` long after the route was changed to auto-heal invalid sessions into a new 200 response — and three contract tests were still asserting the documented 404.

### Never claim a guarantee stronger than the code enforces

This is not the stale-comment case above — it is a comment that was never true. Two shapes recur: a doc that states an invariant as an inherent property when some *other* function is what actually enforces it, and a `SAFETY` block that asserts the lock/guard excludes more than it does. Both read as verified facts, both are trusted over the code, and both survive review easily because nothing type-checks a claim inside a `///`. The tell is a comment that contradicts a neighbouring doc or test — if two comments in one module state different guarantees, at least one is wrong.

**Rule:** State an invariant once, in the function that enforces it; everywhere else reference that site instead of restating the property. In a `SAFETY` comment, name the half of the obligation you discharge *and* the half you do not — a deliberately accepted, documented risk is honest engineering; silently widening the guarantee is a defect. When in doubt, read the upstream contract (std's own docs, the module doc) rather than paraphrasing it from memory.

```rust
// WRONG — asserts reader-exclusion the mutex does not provide, and directly
// contradicts the module doc's own "What this guard does not guarantee"
// SAFETY: the lock is held, so no other thread can be calling
// `set_var`/`remove_var`/`var` concurrently with this call.
unsafe { std::env::set_var(key, value) };

// RIGHT — names what is covered and what is deliberately not
// SAFETY: the crate-wide lock is held, which discharges only the writer half
// of `set_var`'s contract (no concurrent mutators). Concurrent *readers* are
// not excluded — see the module doc's "What this guard does not guarantee".
unsafe { std::env::set_var(key, value) };
```

Flagged in KYO-240 (`current_user_id_from`'s doc asserted "an empty id can never match a real `member.user_id`" as inherent, when only the guard inside `is_owner_from` makes it true) and in KYO-318 cycle 2, where the per-call `SAFETY` comments claimed the env lock excluded concurrent *readers* while the module doc said the opposite — the contradiction passed a full review pass and was only caught by CI after merge.

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

### Types that cross the client/server boundary belong in `kyomi-types`

`kyomi-ui` compiles to `wasm32` and can only depend unconditionally on `kyomi-types` — `kyomi-core` and `kyomi-auth` are `ssr`-gated optional dependencies, unavailable to the WASM client. When a server_fn's request or response type needs to exist on both sides, defining it in `kyomi-ui` and hand-writing a `From` conversion from the "real" server type is the failure mode this rule prevents: it silently forks the wire contract, and the two copies drift the moment one side gains a field.

**Rule:** Define the type once in `kyomi-types`. Have the owning server crate (`kyomi-auth`, `kyomi-core`) `pub use` it so existing server-side call sites don't churn, and import it directly in `kyomi-ui` — no local redeclaration, no `From` impl.

```rust
// WRONG — kyomi-ui redeclares the server type and hand-converts
// crates/kyomi-ui/src/server_fns/datasource_oauth.rs
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GoogleProject { pub project_id: String, pub name: String }

#[cfg(feature = "ssr")]
impl From<kyomi_auth::google_oauth::GoogleProject> for GoogleProject {
    fn from(p: kyomi_auth::google_oauth::GoogleProject) -> Self {
        Self { project_id: p.project_id, name: p.name }
    }
}

// RIGHT — one definition, re-exported on both sides
// crates/kyomi-types/src/datasource_contracts.rs
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GoogleProject { pub project_id: String, pub name: String }

// crates/kyomi-auth/src/google_oauth.rs
pub use kyomi_types::GoogleProject;

// crates/kyomi-ui/src/server_fns/datasource_oauth.rs
pub use kyomi_types::GoogleProject;
```

The one legitimate exception is a server type that carries server-only derives or dependencies (e.g. `sqlx::FromRow`) — a DB row can't be relocated to a dependency-free crate. In that case, split a plain wire DTO in `kyomi-types` from the DB model, rather than moving the DB model itself.

A same-named type in two crates is not automatically a duplicate of this kind — check the fields and purpose before merging. `kyomi_core::models::QueryCache` is a `sqlx::FromRow` DB row for the `query_cache` table; `kyomi_ui::query_cache::QueryCache` is an unrelated Leptos reactive cache handle. Sharing a name is a naming collision, not a shadow type.

Flagged in KYO-196 review: `GeneratedSshKey`, `GoogleProject`, `GoogleOAuthProjectsResult`, `GoogleOAuthDisconnectResult`, and `DatasourceOAuthDisconnectResult` were each independently redeclared in `kyomi-ui` with a matching `From` impl in `kyomi-auth`, purely to satisfy the wasm32 dependency boundary.

## Testing

*Standards for test structure, assertions, and what must be tested.*

### Every bug fix / behavior change ships with a test asserting the exact changed behavior

A fix is not done until a test locks in the precise behavior the ticket changed — not "related functionality," the exact assertion. If the target module has no `#[cfg(test)] mod tests`, add one; these are usually one-liners.

**Rule:** Before calling a fix complete, ask "what one assertion, if it regressed, would silently reintroduce this bug?" and write that test. A `Display`-format fix gets `assert_eq!(Error::X(msg).to_string(), msg)`; a security/validation fix gets one test per rejection path (wrong user, expired, wrong state); a query-identity fix asserts the row count / archived count directly.

Flagged repeatedly in review: KYO-145 (missing `Display` prefix-free assertion blocked sign-off until added), KYO-143 (new status-mapping branch shipped untested), KYO-140 (exemplary — the archival fix shipped with a test running the real query template against an in-memory pool asserting `tables_archived == 0`).

### Prove the test fails without the fix — then prove you restored the tree

A green test proves nothing on its own. It may assert a condition that holds either way: an `assert_ne!(None, None)` on a value that is `None` on both code paths, a filter that empties the collection before the assertion runs, an enum variant reachable by an early return that never touches the changed logic. Every one of those passes against the buggy code too, which means the regression it claims to prevent will ship silently.

**Rule:** Before claiming a test locks in a behavior change, revert the fix (or mutate the exact line the assertion depends on), re-run *that* test, and confirm it fails with the failure the ticket describes. Then restore from a pre-mutation copy and confirm `git diff --cached` is byte-identical to what you staged — a mutation left behind in the working tree is worse than no test. Quote the mutation and its failure output in the PR; "the tests pass" is not the claim being made. Prefer assertions that can only pass for the right reason: assert the *whole* captured log is empty rather than filtering to one level, and put an `assert!(x.is_some())` ahead of any `assert_ne!` whose `None == None` case would pass vacuously.

```bash
# 1. Break the exact line the assertion depends on
$ # edit auth_service.rs: "signup" -> "email_verification" (the pre-fix value)
$ cargo test -p kyomi-auth --locked --lib passkey_signup_verify_only_accepts_its_own_token_type
#   → MUST fail, with the cross-flow acceptance the ticket describes

# 2. Restore and prove no drift from your own testing
$ cp .backup/auth_service.rs crates/kyomi-auth/src/auth_service.rs
$ git diff --cached --stat        # identical to before the mutation
$ cargo test -p kyomi-auth --locked --lib passkey_signup_verify_only_accepts_its_own_token_type
```

Applied in almost every review in the 2026-08-01 → 2026-08-07 window, and load-bearing in several: KYO-256 mutated both auto-heal branches to echo the bogus session id back, and confirmed each `assert_ne!` duly failed with `Some(x)` on both sides rather than passing vacuously; KYO-263 mutated both guard conditions to show the fail-closed branch was covered by a real assertion rather than only by prose; KYO-222 cycle 2 re-ran the implementer's `#[serde(rename)]` mutation rather than trusting the claim; KYO-281 and KYO-282 each reverted the shipped fix to reproduce the exact panic; KYO-259 broke a matrix expression to prove `actionlint` catches it. Each of those reviews also re-confirmed the staged diff byte-for-byte after restoring.

### A `!contains(...)` assertion after an `assert_eq!` on the same value is dead

If a test already asserts `assert_eq!(actual, "the exact expected string")`, every following `assert!(!actual.contains(X))` on that same value is unreachable as a failure. For any mutation that changes `actual`, the equality fires first; for any mutation that leaves `actual` unchanged, the `!contains` cannot fire either — the expected literal is fixed, so whether it contains `X` is decided at authoring time, not at run time. The line reads like a second guard and carries no weight.

**Rule:** Don't stack a substring guard behind an exact-equality assertion on the same value. Either assert equality alone, or — if the substring is the real invariant and the exact string is incidental — assert the substring against something the equality does not already pin, such as a *different* value (`assert!(!other_field.contains(X))`) or a compile-time property of the constant itself (`assert!(!WRAP_UP_FAILED_MESSAGE.contains("cancelled"))`, which pins a constant a future reword could silently break). When reviewing, evaluate the pair, not each line in isolation.

Flagged in the KYO-344 cycle-2 review (2026-08-10): the pair at `crates/kyomi-agent/src/agent.rs:1920` was mutation-tested three ways and the `!contains` guard was proven dead both before and after the change under review — reasoning alone had concluded the opposite.

### A test that never ran is not a passing test — make the skip fail where it matters

Two mechanisms remove a test from a run while the run still reports green. A test that skips at runtime (`let Some(pool) = connect().await else { eprintln!("SKIP: ..."); return; }`) *passes*, and the default harness captures and discards stderr for passing tests — the `SKIP:` line is invisible unless someone passes `--nocapture`/`--show-output`, which CI does not. A test behind a feature gate (`#[cfg(all(test, feature = "ssr"))]`) is not compiled at all without that feature, and the suite still exits `ok` with a healthy-looking count made up entirely of other tests. In both cases the reported total is true and the conclusion drawn from it is false, and nothing will ever surface the gap.

**Rule:** A test that can decline to run must fail loudly in the environment that is supposed to run it. Gate the skip on an explicit env var CI sets — panic naming the variable, the test, and the underlying error when it is set; skip when it is unset, so a local run without the container still works. When reporting a test count for a feature-gated suite, name the feature and confirm the specific new test names appear in the output (grep the run for them) rather than quoting a total. Never call a skip "visible" until you have watched it under the exact command CI runs.

```bash
# WRONG — passes, prints nothing, proves nothing
$ cargo test --locked --workspace --lib --bins --tests   # SKIP: line captured and discarded
test result: ok. 672 passed; 0 failed

# RIGHT — CI sets the var, so a Postgres-arm test that cannot connect fails the job
$ KYOMI_REQUIRE_POSTGRES_TESTS=1 cargo test -p kyomi-auth --locked --lib -- postgres_
#   → panics naming KYOMI_REQUIRE_POSTGRES_TESTS, the test, and the connection error
$ cargo test -p kyomi-auth --locked --lib -- postgres_   # var unset, local dev
#   → SKIP: lines, run still green
```

Flagged as 🟡 in KYO-292 (2026-08-09): `crates/kyomi-auth/src/test_pg.rs`'s module doc claimed the skip was made "visible … rather than silent, because a Postgres-arm test that always reports success without ever running the Postgres arm is worse than no test" — the intent was right and the mechanism did not deliver it, since `ci.yml`'s `cargo test --locked --workspace --lib --bins --tests …` passes neither capture flag. Fixed by having `postgres_test_pool_or_skip` panic under `KYOMI_REQUIRE_POSTGRES_TESTS=1`, which the CI job now sets alongside its `pgvector` service; both branches were then reproduced against a dead `DATABASE_URL`. The feature-gate half was flagged in KYO-278 (2026-08-08): the new `#[cfg(all(test, feature = "ssr"))]` regression tests are invisible without `--features ssr` while `kyomi-ui` still runs 161 other tests green — and the reviewer's own first pass mis-read a truncated log as "the crate compiles zero tests without ssr," a stronger claim than the evidence supported.

### Deleting a file means accounting for every test in its `#[cfg(test)] mod tests`

When a duplicate implementation is deleted, its functions get ported because callers need them; its test module goes down with the file because nothing references it. Some of those tests are not about the layer being deleted at all — they pin security properties or data-compatibility guarantees of code that survives, and they have no equivalent anywhere else. Nothing fails when they vanish: the suite gets smaller and stays green, and the loss is discoverable only by reading a file that no longer exists.

**Rule:** Before deleting a file, enumerate every `fn` in its test module and give each one an explicit disposition — ported (say where), or already covered (name the specific test and confirm it actually asserts the same property). "Covered elsewhere" is a claim to verify, not an assumption. Establish that the module is exhausted by reading it end to end rather than spot-checking, and mutation-test each ported test in its new home: a test that changed crates may have picked up a different fixture or a differently-flavoured runtime.

Flagged across all three cycles of KYO-286 (2026-08-09), which deleted `apps/server/src/routes/auth_passkeys.rs` — 1645 lines whose `mod tests` ran from line 1427 to EOF and held 9 tests. Cycle 1 🔴: the two KYO-215 tests on `lookup_recovery_user` (a DB failure logs `error!` without the requester's email; an absent user logs nothing, so the two enumeration-resistant outcomes stay distinguishable to operators but not to callers) were dropped even though the function itself was ported verbatim into `crates/kyomi-auth/src/auth_service.rs`. Cycle 2 🟡: `deserialize_migrated_passkey_json`, guarding that Python-migration passkey JSON still deserializes under `webauthn-rs`, was orphaned with no equivalent anywhere in the tree. The six `require_purpose_*` tests were the one honest "covered elsewhere" case — `webauthn_challenge_purpose.rs` tests `has_purpose` directly and `auth_service.rs` exercises purpose binding end to end — but that took checking, not assuming. Only a line-by-line accounting in cycle 3 established the module was exhausted.

### A test that reads back only its own seeded rows cannot catch a widened filter

A test that seeds two sessions, calls `fetch_session_counts(&[a, b])`, and asserts `counts[a] == 3` proves the rows it seeded are found. It says nothing about what else came back. Widen the query's `WHERE session_id = ANY($1)` to `... OR pinned = true` and every assertion still passes — the seeded ids already matched the id-list clause — while the caller now receives every other tenant's rows. That is exactly the shape of a scoping bug, and it is the one shape this kind of test structurally cannot see.

**Rule:** Any test over a query whose correctness depends on a filter must assert the *size* of the result, not merely the presence of the rows it seeded, with a message naming the leak the assertion exists to catch. Mutate the filter to the *widened* form (`AND` → `OR`) rather than to a broken one — a mutation that empties the result is killed by any assertion at all and proves nothing about scoping.

```rust
// WRONG — survives WHERE session_id = ANY($1) OR pinned = true
assert_eq!(counts.get("session-a"), Some(&3));

// RIGHT — the widened filter now fails on the count
assert_eq!(counts.len(), 2, "query must return only the requested sessions; an AND→OR widening would leak other workspaces' rows");
assert_eq!(counts.get("session-a"), Some(&3));
```

Flagged as 🟡 in KYO-292 (2026-08-09): `postgres_fetch_session_counts_counts_messages_and_pinned` (`crates/kyomi-auth/src/chat_service.rs`) and `postgres_fetch_member_counts_counts_only_active_members` (`crates/kyomi-auth/src/workspace_service.rs`) both looked up only their own seeded ids and both survived the `AND`→`OR` mutation traced above, while the two sibling tests in the same diff that did assert `.len()` did not. This is the test-side counterpart of *`workspace_id` is not an authorization boundary* above — the query returns more than the requester may see, and every assertion still passes.

## Version Control & Working Tree

*Standards for reasoning about repo state before drawing conclusions from it.*

### Never draw conclusions about repo state from a working tree you have not verified is current

A stale checkout is indistinguishable from a real defect when you only look at the working tree. A file mode that reads `100644` instead of `100755`, a file that appears missing, a config block that isn't there, a hook that "was never wired up" — every one of these reads identically whether the repo genuinely lacks the thing or your clone is simply behind. The working tree cannot tell you which case you're in; only comparing it against the remote can.

**Rule:** before filing a bug, writing a ticket, or asserting that something is missing, unenforced, or unimplemented based on what you see in a checkout, run `git fetch origin main` and `git rev-list --left-right --count main...origin/main`. A non-zero right-hand number means the checkout is behind, and every conclusion drawn from it is provisional until you either update the tree or account for the drift. State the divergence in the ticket, or say explicitly that the tree was verified current.

This compounds with the repo-layout hazard already documented in `CLAUDE.md`: a `grep` limited to `~/repos/kyomi` misses code that lives in `kyomi-connect`, `chartml`, or the other sibling repos. It's the same failure shape — concluding *absence* from an incomplete view, whether the gap is a stale local branch or a repo boundary you didn't search across.

Real precedent: KYO-372 was filed at priority 2 on 2026-08-12, claiming `.githooks/pre-commit` was committed non-executable so the mandatory review-signature gate had never run in any clone. It was cancelled: `origin/main` had the file at `100755` since PR #329 (KYO-358) merged 2026-08-10. The reporting tree was 10 commits behind. Cost: a wrong `agent-ready` ticket, which costs a whole subsequent agent run to triage and cancel. Tracked as KYO-373.
