# Never feed a reactive prop with `.get_untracked()` at the callsite

A component whose prop is `MaybeProp<T>` / `Signal<T>` reads that prop inside a
closure, so it re-reads on every render. Passing `some_signal.get_untracked()` as
the argument defeats that entirely: the call evaluates **once, at parent-render
time**, and hands the component a plain `T`. The `MaybeProp` then holds that one
frozen value for the lifetime of the view, and every later `.set()` on the signal
is dead code. The component is still reactive; it has simply been given nothing to
react to.

This is worse than an ordinary stale-render bug because it is invisible at the
component. `ConfirmDialog`'s own doc comment correctly promised *"All text props
accept `Signal<String>` … so they re-read reactively when the dialog opens — no
stale-render bugs"*, and its body genuinely did read them in closures. Three
callsites still rendered an empty dialog, because the defect was one level up the
call stack. Reviewing the component tells you nothing; only the callsite does.

**Rule:** When a prop is declared reactive (`MaybeProp<T>`, `Signal<T>`, or any
`#[prop(into)]` that accepts a signal), pass the signal itself. Never pass
`signal.get_untracked()` — or `signal.get()` — as the prop argument.

```rust
// WRONG — collapses to a plain String at parent-render time. The signals are
// initialised empty and set later by a click handler, so the dialog renders an
// empty <h3> and empty <p> forever.
<ConfirmDialog
    open=Signal::from(dialog_open)
    title=dialog_title.get_untracked()
    message=dialog_message.get_untracked()
    destructive=dialog_destructive.get_untracked()
    ...
/>

// RIGHT — the component re-reads each one when it opens.
<ConfirmDialog
    open=Signal::from(dialog_open)
    title=dialog_title
    message=dialog_message
    destructive=dialog_destructive
    ...
/>
```

If the `into` conversion needs help, use `Signal::from(dialog_title)` or
`Signal::derive(move || dialog_title.get())` — both preserve reactivity. Reach for
a snapshot only when the prop is genuinely a plain `T` and the value genuinely
cannot change.

## This does not contradict `resolve-signal-values-at-click-time.md`

Those two rules point in opposite directions on purpose, and the distinction is
*where the call sits*, not which method it names:

| Position | Correct call | Why |
|---|---|---|
| Inside an `on:click` / event handler | `.get_untracked()` | You want the value **now**, at interaction time, and you must not create a reactive dependency inside a handler. |
| As a **prop argument** in a `view!` | pass the signal | You want the child to re-read it **later**, every time it renders. |

A handler runs once per interaction, so evaluating it there is exactly right. A
prop argument is evaluated once per *view construction*, which for a dialog that is
populated on click means "before the value exists". Same method, opposite verdicts.

## Symptom to recognise

Signals initialised to `String::new()` / `false`, set inside a click handler, and
read with `.get_untracked()` in the `view!`. The rendered result is always the
**initial** value — an empty title, or a confirm button styled from the wrong
`destructive` default — never the value the handler set. If a dialog renders blank,
check the callsite before the component.

## Widening a non-reactive prop is the right fix, not a workaround

If the prop is a plain `bool` (or `String`) and a caller genuinely needs it to
change, widen the prop rather than snapshotting at the callsite:
`#[prop(optional, into)] destructive: MaybeProp<bool>`, read as
`destructive.get().unwrap_or(true)`, keeping the prior default. Plain literals
still work via `Into`, so existing callsites are unaffected. `Switch`'s `disabled`
prop was widened this way for the same reason.

Motivated by KYO-726 (`ConfirmDialog` — Team, Billing and Passkeys all rendered
titleless, messageless confirmation dialogs for destructive actions) and KYO-487
(`Switch`'s `disabled`, the same class one prop over).

**Not** KYO-441, despite KYO-726's description citing it as "the second class of
stale-render bug in `ConfirmDialog`". KYO-441 was a *stacking* defect — the dialog's
backdrop sat at `z-50`, below `Modal`'s `z-[1000]`, so a dialog opened from inside a
modal painted underneath it. Its content was correct and it had no reactivity
component at all. The two share only a symptom ("the user cannot see what they are
confirming") and a file, not a mechanism. Checked against KYO-441's own ticket and
the `backdrop_clears_modal_elevated_layer` test it left behind.
