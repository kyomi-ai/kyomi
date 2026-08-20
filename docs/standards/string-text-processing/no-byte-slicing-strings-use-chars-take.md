# Never byte-slice strings for truncation — use `chars().take(N)`

Rust `&str[..N]` is a byte slice. If `N` falls in the middle of a multi-byte UTF-8 character, the program panics at runtime. This is easy to miss because it works fine with ASCII test data and only fails in production when users enter non-ASCII characters (accented names, emoji, CJK text).

**Rule:** Always use `.chars().take(N).collect::<String>()` for truncation. If the pattern already exists in the same file, use it consistently.

```rust
// WRONG — panics on non-ASCII content at a multi-byte boundary
let preview = if content.len() > 200 { &content[..200] } else { content };

// RIGHT — safe on any UTF-8 content
let preview: String = content.chars().take(200).collect();
```

Flagged in KYO-85 review — two call sites in `dashboard_service.rs` used byte-slicing while a third site in the same file already used the safe `chars().take()` pattern.
