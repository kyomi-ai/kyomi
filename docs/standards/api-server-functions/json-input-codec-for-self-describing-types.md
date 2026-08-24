# A server fn carrying `serde_json::Value` must declare `input = server_fn::codec::Json`

`#[server]`'s default input encoding is `PostUrl`, which decodes the request body with
`serde_qs`. A query string carries no type information: every leaf is text, and the
deserializer recovers the intended type from the *target* type. That works for
`String`/`u16`/`bool` fields and fails silently for a self-describing target. Asked to fill a
`serde_json::Value`, `serde_qs` has nothing to reconstruct from, so it produces
`Value::String("5432")` and `Value::String("true")` where the caller sent `Value::Number` and
`Value::Bool`.

Nothing errors. The struct deserializes, the handler runs, the row is written. The damage
appears one layer further on, in whatever reads the leaf back: `.as_u64()`, `.as_bool()` and
`.is_number()` all answer `None`/`false` for a string, and the call site takes its
`.unwrap_or(DEFAULT)` branch. A user's port, their TLS posture, or a column's entire numeric
classification is discarded, with no log line and no failed test.

This is not a hypothetical about future callers. It has landed twice, in two different files,
and once it had already corrupted persisted data before anyone noticed.

**Rule:** If any parameter of a `#[server]` function is `serde_json::Value` — or a `Vec`,
`Option`, `HashMap` or struct that contains one anywhere inside it — declare
`input = server_fn::codec::Json`. Do this even when every leaf the current caller sends
happens to be a string: the asymmetry is invisible at the call site, and the next field added
to that map is what breaks. Pin it with a test that inspects the macro-generated
`ServerFn::Protocol` for the function, because deleting the attribute compiles, passes every
other test, and reintroduces the bug in silence. When one server fn in a file gets the codec,
check its siblings that take the same map from the same caller — a decoded shape that depends
on which of two entry points sent it is a trap even while dormant.

```rust
// WRONG — default PostUrl codec; `connection_config`'s port arrives as
// Value::String("5432") and the driver's .as_u64() silently uses its default.
#[server(prefix = "/leptos-api")]
pub async fn create_datasource_modal(
    name: String,
    connection_config: serde_json::Value,
) -> Result<String, ServerFnError> { /* ... */ }

// RIGHT — JSON preserves Value::Number / Value::Bool on the wire.
#[server(prefix = "/leptos-api", input = server_fn::codec::Json)]
pub async fn create_datasource_modal(
    name: String,
    connection_config: serde_json::Value,
) -> Result<String, ServerFnError> { /* ... */ }
```

Real precedent, three tickets and one migration:

- **KYO-428** (review log `2026-08-23`, `05:20`) — `crates/kyomi-ui/src/server_fns/datasources.rs`.
  `build_connection_config` genuinely emits `Value::Number` for `port` and `Value::Bool` for
  `secure`/`encrypt`/`trust_server_certificate`; all four affected server fns now carry the
  codec. The same review filed the sibling instance rather than fixing it inline.
- **KYO-459** (review log `2026-08-23`, `15:57`) — that sibling:
  `generate_chart_from_results` in `crates/kyomi-ui/src/server_fns/sql_editor.rs`, taking
  `sample_rows: Vec<Vec<serde_json::Value>>`. Its caller
  (`pages/sql_editor/results_container.rs`) builds real `Value::Number` leaves via
  `s.parse::<f64>()`, and `let is_numeric = values.iter().all(|v| v.is_number())` was
  therefore `false` for *every* column — every numeric column misclassified, in a function
  whose entire job is picking a chart type. The guard test
  (`generate_chart_from_results_uses_the_json_input_codec`) is what makes the fix durable;
  the reviewer's own mutation confirmed removing the attribute kills it.
- **KYO-460** (review log `2026-08-23`, `14:00`) — the bill for the window before KYO-428:
  `apps/server/migrations/20260823000000_retype_connection_config_scalars.sql`, an
  irreversible repair migration over `datasource_configs.connection_config` for every row the
  modal wrote with flattened scalars. Fixing the cause did not repair anything already
  persisted.

Related: [propagate-predicate-changes-to-every-copy.md](../code-organization/propagate-predicate-changes-to-every-copy.md)
is the general form of the "one call site fixed, its twin left behind" half of this; the codec
question is specifically about the wire format, and applies to a lone server fn with no twin
at all.
