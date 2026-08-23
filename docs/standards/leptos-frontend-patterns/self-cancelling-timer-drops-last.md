# A self-cancelling `Interval`/`Timeout` must drop itself last

The idiom for a timer that stops itself is to park the handle in an
`Rc<RefCell<Option<_>>>` that the timer's own closure captures, then `.take()` that
slot from inside the closure to cancel further firing. Dropping the `Interval` /
`Timeout` also drops the closure it owns — and with it the heap-boxed environment
holding every other value the closure captured. Any field read *after* the `.take()`
is therefore a read of freed memory.

This is a real use-after-free, not a lint nit. It is also unusually easy to ship:
wasm is single-threaded, nothing else allocates between the free and the subsequent
reads, and the aliasing exists only through a runtime `Rc`/`RefCell`/`Box` chain, so
neither the borrow checker nor LLVM can see it. It compiles, passes clippy, and works
in practice right up until an allocator change makes it not.

**Rule:** Copy or clone everything the closure still needs into true stack locals
*before* the self-referential `.take()`, and make that `.take()` the last statement in
the closure that touches any captured field. Say so in an inline comment — the
ordering is load-bearing and looks arbitrary to the next reader. Independent handles
(a different `Rc` chain from the one being dropped) are not subject to this, but
ordering them before the self-drop too costs nothing and removes the need to prove it.

```rust
// WRONG — the Interval that owns this closure is dropped mid-body, then the
// closure keeps reading its own captured environment
let interval_slot_poll = interval_slot.clone();
Interval::new(200, move || {
    interval_slot_poll.borrow_mut().take();     // 💥 frees this closure
    timeout_slot_poll.borrow_mut().take();      // reads freed environment
    on_outcome_poll.clone()(Outcome::Closed);   // reads freed environment
});

// RIGHT — hoist what is still needed, then self-drop last
let interval_slot_poll = interval_slot.clone();
Interval::new(200, move || {
    let report = on_outcome_poll.clone();       // stack-local, survives the drop
    timeout_slot_poll.borrow_mut().take();      // independent chain, done first
    // Self-drop must be the final touch of a captured field: it frees this
    // closure's environment.
    interval_slot_poll.borrow_mut().take();
    report(Outcome::Closed);
});
```

Flagged in KYO-437 (review log `2026-08-22`, entry `11:55 — KYO-437: OAuth connect
popup recovery`), where a popup monitor took its own `Interval` and `Timeout` slots
and then continued reading `on_outcome` and the sibling slot from the same freed
environment, at three separate sites in one function. KYO-440 adds a second monitor of
the same shape at the datasource-list level, so this is a pattern the codebase will
keep instantiating rather than a one-off.
