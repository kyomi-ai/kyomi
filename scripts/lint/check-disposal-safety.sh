#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-disposal-safety.sh — Leptos signal disposal safety
#
# Introduced by PR #34 (commit 9c20d8f0). That commit's message cites
# "KYO-289", an identifier from an earlier issue-numbering scheme that no
# longer resolves — cite the PR, not that ID. Enforcement status of this and
# the four sibling patterns it does NOT cover is documented in
# docs/standards/leptos-frontend-patterns/enforcement-status.md (KYO-199).
#
# Static lint that blocks two patterns known to cause "reactive value already
# disposed" WASM panics in Leptos:
#
# NOTE: the two rules are NOT equally strict, despite reading symmetrically
# below. Rule A FAILS the build; Rule B only WARNS and exits 0 (see the
# `*:WARN*)` case in the reporting loop near the bottom of this file). Rule B
# cannot distinguish a genuinely mixed-lifetime derive from a same-scope one,
# so gating on it would fail every build — it is advisory by design. Do not
# "fix" that asymmetry without addressing the false-positive rate first.
#
#   Rule A — bare .set() / .update() inside spawn_local or deferred callbacks
#     [BLOCKING — sets exit status 1]
#     spawn_local spawns a detached future that outlives the component. If
#     the user navigates away before it completes, .set() on a disposed
#     signal panics. Use .try_set() / .try_update() instead.
#
#     Deferred contexts also include gloo_timers Timeout/Interval callbacks.
#
#   Rule B — bare .get() inside Signal::derive / Memo::new closures
#     [ADVISORY — prints WARN:B, does NOT affect exit status]
#     A derive that subscribes to Layout-scoped signals (SyncStore) AND
#     reads page-scoped signals via .get() will panic when the page is
#     disposed and a sync update re-evaluates the derive. Use .try_get()
#     instead.
#
# Escape hatch (require non-empty justification, ≥5 chars after `=` trimmed):
#   `// lint-allow: disposal-safe=<why>`  on the same line as the violation
#
# Usage:
#   check-disposal-safety.sh                 run against full tree
#   check-disposal-safety.sh <file>...       run against the listed files only
#
# Exit codes: 0 no Rule A violations (Rule B warnings do not affect this),
#             1 Rule A violations found, 2 usage error.
#
# Pure bash + awk. No Rust toolchain required.
#
# ------------------------------------------------------------------------------
# KYO-414 — parsing fixes and their known limits (read before trusting a WARN)
# ------------------------------------------------------------------------------
#
# This lint scans source *text*, not a parsed AST, so it approximates Rust
# grammar with a hand-rolled state machine. KYO-414 fixed two real bugs in
# that approximation and closed one structural gap. None of this makes the
# lint precise — it makes it precise *enough that the noise doesn't drown the
# signal*, which is the actual failure mode KYO-414 was filed against (a wall
# of non-blocking WARN:B lines training every reader to stop reading them).
#
# 1. String and char literal content is now excluded from matching *and* from
#    brace-depth tracking. Previously `.find("...move || x.get()...")` in a
#    test assertion tripped Rule B because the matcher had no concept of a
#    string literal — it saw the token text `move ||` and `.get()` and fired
#    regardless of the quotes around them. Fixed by `strip_comment_and_literals()`,
#    which walks each line, char by char, and blanks out (replaces with a
#    single space) the contents of double-quoted strings, raw strings
#    (`r"..."`, `r#"..."#`, ... up to any hash count), and char literals
#    (`'x'`, `'\n'`, `'\xNN'`, `'\u{...}'`) before anything else runs. This
#    is also where `//` line-comment stripping now happens — previously it
#    ran *before* string handling and would truncate a line at the first `//`
#    even when that `//` was inside a string (e.g. `href="https://..."`),
#    silently dropping everything after it from consideration. Comment
#    detection is now string-aware for the same reason literal-matching is.
#
#    Limitation: this is line-by-line. A string or raw string that spans
#    multiple physical lines is not tracked across the line boundary — the
#    scanner treats each line independently and effectively closes an
#    unterminated literal at end-of-line. Grep the target file for a plain
#    multi-line `"..."` (not `r"..."`) before trusting a WARN near one; none
#    were found in this crate at the time of this fix. Block comments
#    (`/* ... */`) are also not recognized, same as before this fix — a
#    pre-existing limitation, not a regression.
#
# 2. A `Signal::derive(...)` / `Memo::new(...)` (or `spawn_local(...)` /
#    `Timeout::new(...)` / etc.) match used to arm a flag that stayed armed
#    across line boundaries until *any* future `{` was seen — including a
#    `{` that belongs to a completely unrelated construct several hundred
#    lines later, if the matched call was a single-line closure with no
#    block body of its own (e.g. `Signal::derive(move || slug.get())`, which
#    never opens a `{` at all). That stray `{` was then wrongly treated as
#    the start of the derive's scope, and every line until *its* matching
#    `}` was misattributed as "inside a derive" — including ordinary
#    `<Show when=move || ...>` and `{move || ...}` view interpolations with
#    no derive anywhere near them. This was the dominant cause of the
#    117-warning count on `datasources.rs` this ticket was filed against
#    (a handful of real per-derive advisories multiplied across everything
#    between one single-line derive and the next unrelated `{`).
#
#    Fixed: a trigger only arms the multi-line stack if a `{` is found later
#    on *that same line*. If it isn't — a single-line derive/memo/deferred
#    call — the rest of that line is checked directly for the forbidden
#    pattern instead, and the flag is never armed. `entering_spawn` and
#    `entering_derive` are also unconditionally cleared at the end of every
#    line's processing as a second, redundant guard against ever leaking
#    into the next line's state again.
#
#    The identical flaw existed for the `#[cfg(test)]` / `mod tests` test
#    module skip: `mod tests;` (an external submodule declaration — no body,
#    no `{` ever coming) used to arm `in_test_module` and, in a file where
#    that declaration is not the last thing in the file, would have silently
#    skipped Rule A (blocking!) checks on every real line after it, waiting
#    forever for a `{` that was never going to arrive. `datasources.rs`
#    happened to declare `mod tests;` as its last two lines, so this was
#    latent rather than observed — but it is the same bug class and is fixed
#    the same way: `mod tests;` (semicolon form) no longer arms the skip;
#    `#[cfg(test)]` and `mod tests {` (block form) still do.
#
# 3. Structural gap, not a parsing bug: KYO-455 established the convention
#    (docs/standards/code-organization/one-test-topic-per-file-not-one-big-mod-tests.md)
#    of splitting a large file's `#[cfg(test)] mod tests { ... }` into
#    `<file_stem>/tests/mod.rs` plus one file per test topic, specifically so
#    concurrent PRs don't collide on one shared tail. Those topic files
#    (`datasources/tests/oauth.rs` and friends) carry no `#[cfg(test)]` of
#    their own — the attribute lives once, on the `mod tests;` declaration in
#    the *parent* file — so no per-line text scan of a topic file in
#    isolation can ever discover it is test-only code. A text-based lint
#    fundamentally cannot see across that file boundary.
#
#    This lint exists to catch a *runtime rendering* hazard (a disposed
#    signal read during a live re-render). Files that only exist to assert
#    against `include_str!`'d production source — the KYO-455 pattern this
#    whole convention is for — never render anything; they're pure text
#    matching. So the fix here is a path convention, not a parser trick: any
#    `*.rs` file with a `tests` path component (`.../tests/*.rs`) is skipped
#    entirely, matching the KYO-455 layout. This is a real, if narrow,
#    residual gap: a `tests/` directory that ever held something other than
#    KYO-455-style split test modules — e.g. hand-written integration tests
#    that construct and render real components — would be silently exempted
#    too. That doesn't happen anywhere in this crate today (verified via
#    `find crates/kyomi-ui/src -type d -name tests` at the time of this fix,
#    which returns exactly the one KYO-455 directory), but if that convention
#    is ever reused for a different purpose, revisit this exclusion.
#
# 4. A KYO-414 follow-up fix, found during review: a Leptos signal read is
#    ALWAYS `.get()` with empty parens -- `Signal<T>::get(&self) -> T` takes
#    no argument. Both Rule B's trigger and Rule A's `.get_untracked()`
#    trigger used to match `.get(` regardless of what was inside the
#    parens, so `serde_json::Value::get(key)`, `HashMap::get(&k)`, and any
#    slice/Vec `.get(idx)` sitting lexically inside a derive/deferred
#    context all matched too -- real reproduction:
#    `v.get("client_email")?.as_str()` on a serde_json::Value at
#    datasources.rs:7976. Fixed by requiring the parens contain nothing but
#    whitespace. This interacted with the literal-stripping fix in header §1:
#    blanking a stripped string argument to spaces turned
#    `.get("client_email")` into `.get(   )`, which the new empty-parens
#    check could no longer tell apart from a real `.get()` -- fixed by
#    filling stripped literals with a repeated non-whitespace placeholder
#    (see fill_run()) instead of spaces.
#
#    Residual gap, not observed in this crate: any OTHER zero-argument
#    `.get()` method -- `Cell<T>::get()`, `OnceCell<T>::get()`,
#    `OnceLock<T>::get()` -- is indistinguishable from a signal read by
#    this same arity check and would still false-positive if it ever
#    appeared inside a Signal::derive/Memo::new body. Checked at the time of
#    this fix: kyomi-ui does use OnceLock (dashboard_editor.rs, a lazily-
#    compiled regex behind a thread_local!), but not inside any derive, so
#    it does not currently trigger this.
#
# What KYO-414 deliberately did NOT change: Rule B still fires on every bare
# `.get()` inside a real, multi-line `Signal::derive`/`Memo::new` block, even
# when the derive provably reads only same-scope signals. That's not a bug —
# per docs/standards/leptos-frontend-patterns/enforcement-status.md, Rule B
# cannot distinguish a mixed-lifetime derive from a same-scope one without
# AST awareness this script doesn't have, so it warns on all of them by
# design and leaves the judgment call to the reader. KYO-414 removed the
# noise that had nothing to do with that judgment call in the first place.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LINT_DIR="${DISPOSAL_LINT_DIR:-$REPO_ROOT/crates/kyomi-ui/src}"

declare -a TARGETS=()
if [ "$#" -gt 0 ]; then
    for f in "$@"; do
        if [ -f "$f" ]; then
            abs="$(cd "$(dirname "$f")" && pwd)/$(basename "$f")"
        elif [ -f "$REPO_ROOT/$f" ]; then
            abs="$REPO_ROOT/$f"
        else
            continue
        fi
        case "$abs" in
            "$LINT_DIR"/*.rs)
                # KYO-414: KYO-455-style split test modules
                # (<file_stem>/tests/*.rs) render nothing — see header §3.
                case "$abs" in
                    */tests/*.rs) ;;
                    *) TARGETS+=("$abs") ;;
                esac
                ;;
            *) ;;
        esac
    done
else
    while IFS= read -r -d '' f; do
        case "$f" in
            */tests/*.rs) ;;
            *) TARGETS+=("$f") ;;
        esac
    done < <(find "$LINT_DIR" -name '*.rs' -type f -print0 | sort -z)
fi

if [ "${#TARGETS[@]}" -eq 0 ]; then
    exit 0
fi

awk_program='
BEGIN {
    depth = 0
    spawn_stack_size = 0
    derive_stack_size = 0
    entering_spawn = 0
    entering_derive = 0
    in_test_module = 0
    test_module_depth = 0
    # A literal single-quote character, built from its char code rather
    # than written directly: this whole program is embedded in a bash
    # single-quoted string, which has no escape mechanism at all, so an
    # actual quote-mark byte anywhere below would end that bash string
    # early and corrupt everything after it.
    SQ = sprintf("%c", 39)
}

function trim(s,  t) {
    t = s
    sub(/^[[:space:]]+/, "", t)
    sub(/[[:space:]]+$/, "", t)
    return t
}

# Strips `//` line comments AND the contents of string/raw-string/char
# literals from a line, in one string-aware pass — see header §1 for why
# comment-stripping has to be string-aware too (a `//` inside a string, e.g.
# a URL, is not a comment). Literal contents are replaced with a single
# space each so neither pattern matching nor brace-depth counting below can
# be fooled by text or braces that only exist inside quotes.
function strip_comment_and_literals(s,    result, i, n, c, c2, c3, j, hashes,
                                     k, close_delim, start_content, rest,
                                     end_pos, cj, close_b, consumed,
                                     lit_start) {
    result = ""
    n = length(s)
    i = 1
    while (i <= n) {
        c = substr(s, i, 1)

        # Unquoted `//` starts a line comment: everything after is dropped.
        if (c == "/" && substr(s, i + 1, 1) == "/") {
            break
        }

        # Raw string literal: r"...", r#"..."#, r##"..."##, ...
        if (c == "r") {
            j = i + 1
            hashes = 0
            while (substr(s, j, 1) == "#") { hashes++; j++ }
            if (substr(s, j, 1) == "\"") {
                close_delim = "\""
                for (k = 0; k < hashes; k++) close_delim = close_delim "#"
                start_content = j + 1
                rest = substr(s, start_content)
                end_pos = index(rest, close_delim)
                lit_start = i
                if (end_pos > 0) {
                    i = start_content + end_pos - 1 + length(close_delim)
                } else {
                    # Unterminated on this line (likely a multi-line raw
                    # string — see header §1 limitation). Blank the rest.
                    i = n + 1
                }
                result = result fill_run(i - lit_start)
                continue
            }
        }

        # Regular (non-raw) string literal.
        if (c == "\"") {
            lit_start = i
            j = i + 1
            while (j <= n) {
                cj = substr(s, j, 1)
                if (cj == "\\") { j += 2; continue }
                if (cj == "\"") { j++; break }
                j++
            }
            i = j
            result = result fill_run(i - lit_start)
            continue
        }

        # Char literal — best-effort. Must NOT consume a lifetime tick
        # (e.g. a short lifetime name, or the static lifetime): only
        # treated as a literal when a closing quote is found exactly
        # where char-literal grammar requires one.
        if (c == SQ) {
            lit_start = i
            c2 = substr(s, i + 1, 1)
            consumed = 0
            if (c2 == "\\") {
                c3 = substr(s, i + 2, 1)
                if (c3 == "u" && substr(s, i + 3, 1) == "{") {
                    close_b = index(substr(s, i + 4), "}")
                    if (close_b > 0 && substr(s, i + 4 + close_b, 1) == SQ) {
                        i = i + 4 + close_b + 1
                        consumed = 1
                    }
                } else if (c3 == "x" && substr(s, i + 5, 1) == SQ) {
                    i = i + 6
                    consumed = 1
                } else if (c3 != "" && substr(s, i + 3, 1) == SQ) {
                    i = i + 4
                    consumed = 1
                }
            } else if (c2 != "" && substr(s, i + 2, 1) == SQ) {
                i = i + 3
                consumed = 1
            }
            if (consumed) {
                result = result fill_run(i - lit_start)
                continue
            }
            # Not a recognized char literal — a lifetime tick. Keep as-is.
            result = result c
            i++
            continue
        }

        result = result c
        i++
    }
    return result
}

# Fills a stripped literal with `count` repeats of a placeholder character
# instead of blanking it to whitespace. This matters specifically for the
# Rule B "empty parens only" check (KYO-414 follow-up, see
# rule_b_findings()): a bare .get() is a zero-argument Leptos signal read,
# but serde_json::Value::get(key), HashMap::get(&k), and slice.get(idx) all
# take one. Blanking a stripped string argument to nothing but spaces would
# turn `.get("client_email")` into `.get(   )`, which looks like the
# zero-arg form once stripped and would silently reintroduce the exact
# false positive that fix exists to prevent. A non-whitespace, non-brace,
# non-paren, non-slash filler keeps every other stripping guarantee intact
# (no stray `{`/`}` corrupting brace depth, no stray `(`/`)` corrupting the
# paren count of a pending trigger call, no stray `//` forming a fake
# comment) while still reading as non-empty to the parens check. The
# placeholder is repeated for the original length of the literal rather
# than collapsed to one character purely so a maintainer inspecting `code`
# via a debug print is not confused by a huge string collapsing to one
# byte; nothing here depends on the exact count.
function fill_run(count,   out) {
    out = ""
    while (length(out) < count) out = out "~"
    return out
}

function has_escape_hatch(line,  idx, tail, eqidx, just) {
    idx = index(line, "// lint-allow: disposal-safe=")
    if (idx == 0) return 0
    tail = substr(line, idx + length("// lint-allow: disposal-safe="))
    just = trim(tail)
    if (length(just) < 5) {
        printf "%s:%d:WARN empty-or-short escape-hatch justification (need ≥5 chars)\n",
            FILENAME, FNR
        return 0
    }
    return 1
}

function update_stacks(code,  i, c) {
    for (i = 1; i <= length(code); i++) {
        c = substr(code, i, 1)
        if (c == "{") {
            depth++

            if (entering_spawn) {
                spawn_stack_size++
                spawn_stack[spawn_stack_size] = depth
                entering_spawn = 0
            }
            if (entering_derive) {
                derive_stack_size++
                derive_stack[derive_stack_size] = depth
                entering_derive = 0
            }
            if (in_test_module == 1 && test_module_depth == 0) {
                test_module_depth = depth
            }
        } else if (c == "}") {
            if (spawn_stack_size > 0 && depth == spawn_stack[spawn_stack_size]) {
                spawn_stack_size--
            }
            if (derive_stack_size > 0 && depth == derive_stack[derive_stack_size]) {
                derive_stack_size--
            }
            if (in_test_module && depth == test_module_depth) {
                in_test_module = 0
                test_module_depth = 0
            }
            depth--
        }
    }
}

# Resolves whether a Signal::derive(...) / Memo::new(...) / spawn_local(...)
# / Timeout::new(...) / TimeoutFuture::new(...) / set_timeout(...) call opens
# a block body or is a single value-expression with no block at all — see
# header §2. Tracks the calls OWN paren depth (starting at 1, for the `(`
# already consumed by the trigger match) from start_pos to end of code:
#
#   - hits `{` before the parens close  -> block-opening call. Arms
#     entering_spawn/entering_derive so the very next `{` update_stacks()
#     sees (which, per the scan just done, is this exact one) gets pushed
#     onto the right stack at the right depth. This is what correctly
#     allows a call whose block is on a LATER line than the trigger itself
#     (e.g. `set_timeout(\n  move || {\n ...`, used in this crate) to still
#     be recognized as block-form, unlike the naive "look at this one line
#     only" heuristic this replaced.
#   - parens close back to 0 before any `{` -> a single value-expression
#     call with no block. If this resolves on the SAME line the trigger
#     itself matched on (the overwhelmingly common shape in this codebase:
#     `Signal::derive(move || x.get() == "y")`), the consumed span is
#     queued in spawn_remainder/derive_remainder for a same-line rule check
#     — this is what lets a genuine bare `.get()` in a one-line derive keep
#     firing without ever arming the stack (and therefore without being
#     able to leak onto unrelated later code, which was the root cause this
#     ticket was filed against). If it instead resolves on a LATER line
#     than the trigger started on (its own value expression spans more than
#     one line with no block — not observed anywhere in this crate), the
#     call is known NOT to be block-form so nothing leaks, but its content
#     is not rule-checked; that residual gap is intentional and documented
#     rather than engineered away, per header §2.
function advance_pending(kind, code, start_pos,   i, ch, n, seg) {
    n = length(code)
    for (i = start_pos; i <= n; i++) {
        ch = substr(code, i, 1)
        if (ch == "{") {
            if (kind == "spawn") entering_spawn = 1
            else entering_derive = 1
            pending[kind] = 0
            return
        } else if (ch == "(") {
            pending_parens[kind]++
        } else if (ch == ")") {
            pending_parens[kind]--
            if (pending_parens[kind] <= 0) {
                if (pending_start_fnr[kind] == FNR) {
                    seg = substr(code, start_pos, i - start_pos + 1)
                    if (kind == "spawn") spawn_remainder = spawn_remainder seg
                    else derive_remainder = derive_remainder seg
                }
                pending[kind] = 0
                return
            }
        }
    }
    # Still open at end of line -- carry the paren count forward; the next
    # line resumes this same scan from its own position 1.
}

# Rule A: bare .set() / .update() / .get_untracked() in a deferred context.
# Shared by the multi-line (stack-based) and single-line (same-line
# remainder) call sites so the two never drift out of sync — see header §2.
function rule_a_findings(text) {
    if (match(text, /\.[[:space:]]*set[[:space:]]*\(/) &&
        text !~ /\.[[:space:]]*try_set[[:space:]]*\(/ &&
        text !~ /\.[[:space:]]*set_untracked[[:space:]]*\(/) {
        printf "%s:%d:A bare .set() inside deferred context — use .try_set() to avoid disposal panic\n",
            FILENAME, FNR
    }
    if (match(text, /\.[[:space:]]*update[[:space:]]*\(/) &&
        text !~ /\.[[:space:]]*try_update[[:space:]]*\(/ &&
        text !~ /\.[[:space:]]*update_value[[:space:]]*\(/ &&
        text !~ /\.[[:space:]]*update_untracked[[:space:]]*\(/) {
        printf "%s:%d:A bare .update() inside deferred context — use .try_update() to avoid disposal panic\n",
            FILENAME, FNR
    }
    # KYO-414 follow-up: .get_untracked() is a zero-argument Leptos
    # accessor, same reasoning as Rule B just below -- require empty
    # parens (only whitespace permitted) so no other types differently-
    # shaped get_untracked(key) method could ever collide. No real
    # collision found for this one, but the fix costs nothing and keeps
    # the two rules consistent.
    if (match(text, /\.[[:space:]]*get_untracked[[:space:]]*\([[:space:]]*\)/) &&
        text !~ /\.[[:space:]]*try_get_untracked[[:space:]]*\(/) {
        printf "%s:%d:A bare .get_untracked() inside deferred context — use .try_get_untracked() to avoid disposal panic\n",
            FILENAME, FNR
    }
}

# Rule B: bare .get() inside Signal::derive / Memo::new (WARN only). See
# rule_a_findings() above for why this is factored out.
#
# KYO-414 follow-up: a Leptos signal read is ALWAYS `.get()` with empty
# parens — Signal<T>::get(&self) -> T takes no argument. `.get(x)` with
# anything inside the parens is a different method on a different type
# entirely (serde_json::Value::get(key), HashMap::get(&k), a slice/Vec
# .get(idx), ...) and can never be the disposal hazard this rule exists to
# catch. The real reproduction: datasources.rs:7976 is
# `v.get("client_email")?.as_str()...` on a serde_json::Value — nothing to
# do with a signal, but the old `\.get\(` match (no arity check at all)
# could not tell the difference. Requiring empty parens between the `(`
# and `)` (only whitespace permitted) fixes this without touching genuine
# signal reads, which are always `.get()`.
function rule_b_findings(text) {
    if (match(text, /\.[[:space:]]*get[[:space:]]*\([[:space:]]*\)/) &&
        text !~ /\.[[:space:]]*try_get[[:space:]]*\(/ &&
        text !~ /\.[[:space:]]*get_untracked[[:space:]]*\(/ &&
        text !~ /\.[[:space:]]*try_get_untracked[[:space:]]*\(/ &&
        text !~ /\.[[:space:]]*get_value[[:space:]]*\(/) {
        printf "%s:%d:WARN:B bare .get() inside Signal::derive/Memo — consider .try_get() if this derive mixes Layout-scoped and page-scoped signals\n",
            FILENAME, FNR
    }
}

BEGINFILE {
    depth = 0
    spawn_stack_size = 0
    derive_stack_size = 0
    entering_spawn = 0
    entering_derive = 0
    in_test_module = 0
    test_module_depth = 0
    cfg_test_pending = 0
    pending["spawn"] = 0
    pending["derive"] = 0
    pending_parens["spawn"] = 0
    pending_parens["derive"] = 0
    pending_start_fnr["spawn"] = 0
    pending_start_fnr["derive"] = 0
}

{
    raw = $0
    code = strip_comment_and_literals(raw)

    # Skip test modules entirely. `#[cfg(test)]` can precede EITHER a block
    # item (`mod tests { ... }`, `fn helper() { ... }`) OR a bare submodule
    # declaration (`mod tests;`, the KYO-455 split-test-module form — no
    # body, ever). Only the former should arm the skip; arming unconditionally
    # on the attribute line (as before KYO-414) leaves it waiting forever
    # for a `{` that a semicolon declaration will never produce, silently
    # swallowing every real line after it as "test code" — including Rule A
    # (blocking!) violations. So the attribute only marks a one-line lookahead
    # (cfg_test_pending); the decision to actually arm is made once the very
    # next line is seen. This does not handle multiple stacked attributes
    # between `#[cfg(test)]` and the item (not used this way anywhere in
    # this crate today) — see header §2.
    if (cfg_test_pending) {
        cfg_test_pending = 0
        if (code !~ /^[[:space:]]*mod[[:space:]]+tests[[:space:]]*;/) {
            in_test_module = 1
        }
    }
    if (code ~ /^[[:space:]]*#\[cfg\(test\)\]/) {
        cfg_test_pending = 1
    } else if (code ~ /^[[:space:]]*mod tests/ &&
               code !~ /^[[:space:]]*mod[[:space:]]+tests[[:space:]]*;/) {
        in_test_module = 1
    }

    spawn_remainder = ""
    derive_remainder = ""

    # Resume a trigger calls argument-list scan carried over from a
    # previous line (e.g. the block-opening `{` is on the line AFTER
    # `set_timeout(`, a real pattern in this crate) before looking for any
    # NEW trigger on this line — see advance_pending()s own comment and
    # header §2.
    if (pending["spawn"]) advance_pending("spawn", code, 1)
    if (pending["derive"]) advance_pending("derive", code, 1)

    # Detect deferred context entry: spawn_local, Timeout, TimeoutFuture,
    # set_timeout.
    if (!pending["spawn"] &&
        (match(code, /spawn_local[[:space:]]*\(/) ||
         match(code, /Timeout::new[[:space:]]*\(/) ||
         match(code, /TimeoutFuture::new[[:space:]]*\(/) ||
         match(code, /set_timeout[[:space:]]*\(/))) {
        pending["spawn"] = 1
        pending_parens["spawn"] = 1
        pending_start_fnr["spawn"] = FNR
        advance_pending("spawn", code, RSTART + RLENGTH)
    }

    # Detect derive context entry: Signal::derive, Memo::new.
    if (!pending["derive"] &&
        (match(code, /Signal::derive[[:space:]]*\(/) ||
         match(code, /Memo::new[[:space:]]*\(/))) {
        pending["derive"] = 1
        pending_parens["derive"] = 1
        pending_start_fnr["derive"] = FNR
        advance_pending("derive", code, RSTART + RLENGTH)
    }

    # Update brace depth and context stacks. Only ever consumes an
    # entering_* flag that advance_pending() just armed by finding a real
    # `{` belonging to that exact call, so this cannot itself attach a flag
    # to an unrelated later block.
    update_stacks(code)

    # Second, redundant guard against the KYO-414 flag-leak bug: an armed
    # trigger must be consumed by update_stacks() on this exact line (it
    # always is, by construction above) or it is dropped here rather than
    # ever surviving into the next lines state.
    entering_spawn = 0
    entering_derive = 0

    # Skip lines in test modules
    if (in_test_module) next

    # Skip lines with escape hatch
    if (has_escape_hatch(raw)) next

    # Rule A: bare signal access inside spawn_local / deferred context —
    # both the stack-based (multi-line block) and same-line (single-line
    # call, no block) forms.
    if (spawn_stack_size > 0) rule_a_findings(code)
    if (spawn_remainder != "") rule_a_findings(spawn_remainder)

    # Rule B: bare .get() inside Signal::derive / Memo::new — same split.
    if (derive_stack_size > 0) rule_b_findings(code)
    if (derive_remainder != "") rule_b_findings(derive_remainder)
}

ENDFILE {
    if (spawn_stack_size > 0) {
        printf "%s:1:PARSE deferred context did not close before EOF (linter parse error)\n",
            FILENAME
    }
    if (derive_stack_size > 0) {
        printf "%s:1:PARSE derive context did not close before EOF (linter parse error)\n",
            FILENAME
    }
    if (pending["spawn"]) {
        printf "%s:%d:PARSE deferred-context call argument list did not close before EOF (linter parse error)\n",
            FILENAME, pending_start_fnr["spawn"]
    }
    if (pending["derive"]) {
        printf "%s:%d:PARSE derive/memo call argument list did not close before EOF (linter parse error)\n",
            FILENAME, pending_start_fnr["derive"]
    }
}
'

findings="$(awk "$awk_program" "${TARGETS[@]}" | LC_ALL=C sort -t: -k1,1 -k2,2n -k3,3)"

if [ -z "$findings" ]; then
    exit 0
fi

status=0
while IFS= read -r line; do
    printf '%s\n' "$line" >&2
    case "$line" in
        *:WARN*) ;;
        *:PARSE*) ;;
        *)
            status=1
            ;;
    esac
done <<< "$findings"

exit "$status"
