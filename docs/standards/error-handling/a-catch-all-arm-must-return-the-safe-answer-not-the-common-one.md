# A catch-all arm must return the answer that is safe when wrong, not the one that is usually right

A `_ =>` arm that returns a *value* is making a claim about every input it was never written
for. The author almost always picks the value that is right for the inputs they had in mind:
the most common status, the key most providers use, the mode most callers want. That choice
feels safe because it is correct for the cases in front of them. For every case they did not
think of, it is a guess, and whether a wrong guess causes damage depends on which way it
errs.

A catch-all that falls to the common answer fails **open**. An input nobody anticipated
reaches the consumer looking like an ordinary one: no error, no log, nothing for a test to
catch. If the value grants something (access, a paid state, a "nothing to do" signal), the
unanticipated input gets that grant. If it names something (a key, a column, a resource
kind), the consumer reads a slot that is never filled and reports an empty result as the
real answer (the downstream form of
[empty-on-failure-must-not-look-like-a-real-result](empty-on-failure-must-not-look-like-a-real-result.md)).

You cannot always remove the catch-all.
[a-generic-conversion-is-a-leak-site](a-generic-conversion-is-a-leak-site.md) recommends an
exhaustive match with no wildcard, so that a new upstream variant becomes a compile error,
and that is right wherever the compiler allows it. It does not allow it on a foreign enum
marked `#[non_exhaustive]`. Outside the defining crate, a `_` arm is mandatory, so the
compile-error guarantee is not available and a ticket that asks for "no wildcard arm" cannot
be satisfied as written. Many such enums also have an explicit `Unknown(String)` variant for
values the crate version doesn't recognise. Taken together, these types are telling you that
unknown inputs **will** arrive at runtime. At that point the only decision left is what the
catch-all returns.

**Rule:** When a match must have a catch-all that returns a value, return the value whose
*wrong* use is harmless. That means deny rather than grant, "lapsed" rather than "paid",
"error" rather than "empty". Log the raw input at `error!` so an unmapped case shows up in
the logs. Do not make it the variant that is correct most often. List every known variant
explicitly, even ones that map to the same result as the catch-all, so the `_` arm is only
reached by values nobody has classified yet. Where the enum is your own or the compiler
allows it, drop the `_` arm entirely and let exhaustiveness do the work. Before writing a
`_` arm, read the enum's definition at the locked version
([../build-toolchain/read-the-locked-dependency-source-before-resting-on-its-semantics.md](../build-toolchain/read-the-locked-dependency-source-before-resting-on-its-semantics.md))
to learn whether it is `#[non_exhaustive]` and whether it has an `Unknown` variant.

Pair it with one test per known variant, derived from the enum as
[../testing/enumerate-the-variant-registry.md](../testing/enumerate-the-variant-registry.md)
describes, and one test that drives `Unknown` (or the nearest reachable stand-in for the
catch-all) and asserts the safe value.

WRONG: quoted from `crates/kyomi-auth/src/stripe_service.rs` on `main` at `d02406fb`, in
`parse_subscription_data`. Every Stripe status the author didn't list is read as a paid,
active subscription:

```rust
match subscription.status {
    SubscriptionStatus::Trialing => "trialing".to_string(),
    SubscriptionStatus::Active => "active".to_string(),
    SubscriptionStatus::PastDue => "past_due".to_string(),
    SubscriptionStatus::Canceled => "cancelled".to_string(),
    SubscriptionStatus::Unpaid => "cancelled".to_string(),
    _ => "active".to_string(),
}
```

RIGHT: this is the shape, not a verbatim quote. Every known variant is listed, and both
`Unknown` and the mandatory `_` fail closed and log:

```rust
match status {
    SubscriptionStatus::Active => Status::Active,
    SubscriptionStatus::Trialing => Status::Trialing,
    SubscriptionStatus::PastDue | SubscriptionStatus::Incomplete => Status::PastDue,
    SubscriptionStatus::Canceled
    | SubscriptionStatus::Unpaid
    | SubscriptionStatus::Paused
    | SubscriptionStatus::IncompleteExpired => Status::Cancelled,
    SubscriptionStatus::Unknown(raw) => {
        tracing::error!(raw_status = %raw, "unrecognised status; treating as lapsed");
        Status::Cancelled
    }
    // Required: the enum is #[non_exhaustive]. Same answer as Unknown, never Active.
    _ => {
        tracing::error!(raw_status = status.as_str(), "unrecognised status; treating as lapsed");
        Status::Cancelled
    }
}
```

Real precedent:

- **KYO-804** (review log `2026-09-23`, `19:06` entry). The `_ => "active"` above meant that
  `paused`, `incomplete` and `incomplete_expired` subscriptions were all read as paid. The
  ticket asked for "no wildcard arm". The reviewer confirmed from the vendored
  `async-stripe-shared` source at the locked version that `SubscriptionStatus` is
  `#[non_exhaustive]` with an `Unknown(String)` variant, so that criterion was *"genuinely
  unsatisfiable from outside that crate"*. It accepted an explicit arm per known variant
  with `Unknown` and `_` both failing closed to `Cancelled` under `tracing::error!`. When
  this rule was written, the fix was still on an open PR and had not landed on `main`.
- **KYO-544, folded into KYO-474** (review log `2026-08-29`, `15:05` entry). In
  `crates/kyomi-ui/src/pages/settings/datasources.rs`, `discovery_resource_key_for_type`
  ends in `_ => "databases"`, which is the key most providers use. BigQuery fell through to
  it, but BigQuery only ever populates `"projects"`. As a result, a real permission denial
  rendered as "0 projects found": the fallthrough *"silently reads a key BigQuery never
  populates"*. The fix added a separate `catalog_denial_key_for_type` with BigQuery mapped
  explicitly, rather than trusting the common default.

Nearest siblings:
[a-guard-in-one-branch-does-not-cover-the-others](a-guard-in-one-branch-does-not-cover-the-others.md)
is about a *check* that is missing from the catch-all arm. This rule is about the *value*
the catch-all returns.
[a-generic-conversion-is-a-leak-site](a-generic-conversion-is-a-leak-site.md) is about
removing the wildcard where the compiler allows it. This rule covers what to do when it
does not.
