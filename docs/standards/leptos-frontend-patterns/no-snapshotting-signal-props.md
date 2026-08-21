# Never snapshot a `Signal` prop into a local variable — derive from it

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
