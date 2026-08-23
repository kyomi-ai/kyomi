//! Catalog tab wiring: registry-driven indexing auth modes (KYO-187)
//! and the create/edit-mode `include_public_datasets` read/write
//! agreement (KYO-446).

use super::{extract_between, MOD_TESTS_MARKER, SRC};

// ── KYO-187: indexing auth modes come from the registry ────────────
//
// `get_indexing_auth_modes` used to be a client-side `match ds_type {
// "bigquery" => ..., "databricks" => ..., ..., _ => &[] }` hardcoded in
// this file — a fifth provider silently got the empty `_` arm (no
// indexing credentials UI at all) until someone remembered to add a case
// here too. KYO-187 deleted that function and made
// `EditModeCatalogTab` source its auth-mode list from the registry via
// `get_datasource_types()` / `DatasourceTypeInfo::indexing_auth_modes`.
// The registry-level exhaustiveness check lives in
// `indexing_auth_modes_match_legacy_client_hardcoded_table`
// (kyomi-core); this test covers the UI wiring specifically — that the
// component still reads from the server payload and that nobody has
// reintroduced a local hardcoded mapping.

#[test]
fn edit_mode_catalog_tab_sources_indexing_auth_modes_from_the_registry() {
    // `MOD_TESTS_MARKER` (defined in `tests/mod.rs`) is the actual
    // `#[cfg(...)]\nmod tests;` code text that ends `datasources.rs` (a
    // real newline, unescaped quotes), so it matches only that
    // declaration — not this const's own string literal, which is
    // textually different (escaped `\"` / `\n`). `EditModeCatalogTab` is
    // the last item before that declaration, so this slice is exactly
    // the function body.
    let f = extract_between(SRC, "fn EditModeCatalogTab(", MOD_TESTS_MARKER);

    assert!(
        f.contains("get_datasource_types()"),
        "EditModeCatalogTab must fetch its auth-mode options via get_datasource_types() \
         (the registry-backed query) — without this call it has no server-derived source \
         for indexing_auth_modes at all"
    );
    assert!(
        f.contains(".indexing_auth_modes"),
        "EditModeCatalogTab must read the per-type mode list off \
         DatasourceTypeInfo::indexing_auth_modes returned by get_datasource_types() — \
         this is the registry-derived replacement for the deleted \
         get_indexing_auth_modes(ds_type) client-side match"
    );
    assert!(
        !f.contains("fn get_indexing_auth_modes"),
        "get_indexing_auth_modes must not be reintroduced — KYO-187 deleted it because its \
         hardcoded match ds_type {{ \"bigquery\" => ..., \"databricks\" => ..., _ => &[] }} \
         silently gave any new datasource type an empty indexing-auth-modes list until a \
         human remembered to add a case here; the registry is the single source of truth now"
    );

    // Scoped to just the auth-modes-resolution closure, not the whole
    // component: EditModeCatalogTab legitimately contains other
    // `"bigquery" =>` / `"databricks" =>` arms elsewhere (e.g. field
    // labels like "Datasets" / "Catalogs to Index") that have nothing to
    // do with indexing auth modes. Widening this check to the whole
    // component would false-positive on those.
    let resolve_modes = extract_between(
        f,
        "let auth_modes: Vec<AuthModeOption> = all_types",
        "let current_type = indexing_creds_type.get();",
    );
    for type_id_arm in ["\"bigquery\" =>", "\"databricks\" =>", "\"synapse\" =>"] {
        assert!(
            !resolve_modes.contains(type_id_arm),
            "the indexing-auth-modes resolution in EditModeCatalogTab must not contain a \
             `{type_id_arm}` match arm — that shape is the KYO-187 regression pattern: a \
             client-side match on datasource type string hardcoding which auth modes are \
             offered, silently drifting out of sync with the registry (and defaulting new \
             types to no indexing credentials at all via a `_ => &[]` arm) instead of \
             reading DatasourceTypeInfo::indexing_auth_modes"
        );
    }
}

// ── KYO-446: "Include public datasets" write/read must agree ───────
//
// Two independent bugs composed to leak BigQuery public datasets past
// an unchecked toggle: (1) `build_connection_config` only wrote
// `include_public_datasets` when the signal was `true`, so unchecking
// the box on save *omitted* the key rather than persisting `false` —
// and `datasource_service::update_datasource_settings` replaces
// `connection_config` wholesale, so omission is silent deletion of the
// prior value; (2) every consumer of that key but this file's own
// load-back defaulted an absent key to `true`, so the leftover/omitted
// key kept behaving as "enabled" while the UI toggle rendered "off".
// These tests pin the write side (both branches must write
// unconditionally) and that the read side goes through the one shared
// helper the fix introduced, so the two can't drift apart again.

/// Bounds the extraction to just the catalog-scope block of
/// `build_connection_config` (create-mode and edit-mode branches),
/// stopping before the unrelated "Indexing credentials" section that
/// follows in the same closure.
fn catalog_scope_write_block(src: &str) -> &str {
    extract_between(
        src,
        "let in_create_mode = datasource_id.get_untracked().is_none();",
        "// Indexing credentials",
    )
}

#[test]
fn create_mode_include_public_datasets_write_is_unconditional() {
    let block = catalog_scope_write_block(SRC);

    assert!(
        !block.contains("if create_include_public_datasets.get_untracked() {"),
        "create-mode must not gate the include_public_datasets write behind \
         `if create_include_public_datasets.get_untracked()` — that shape omits the \
         key entirely when the toggle is off, and datasource_service replaces \
         connection_config wholesale, so an omitted key silently drops any prior \
         value (KYO-446)"
    );
    assert!(
        block.contains(
            "map.insert(\n                    \"include_public_datasets\".to_string(),\n                    \
             serde_json::json!(create_include_public_datasets.get_untracked()),\n                );"
        ),
        "create-mode must write include_public_datasets unconditionally with the \
         signal's actual value (true AND false), not only when it is true: {block}"
    );
}

#[test]
fn edit_mode_include_public_datasets_write_is_unconditional() {
    let block = catalog_scope_write_block(SRC);

    assert!(
        !block.contains("if bq_include_public.get_untracked() {"),
        "edit-mode must not gate the include_public_datasets write behind \
         `if bq_include_public.get_untracked()` — that shape omits the key entirely \
         when the toggle is off, and datasource_service replaces connection_config \
         wholesale, so an omitted key silently drops any prior value (KYO-446)"
    );
    assert!(
        block.contains(
            "map.insert(\n                    \"include_public_datasets\".to_string(),\n                    \
             serde_json::json!(bq_include_public.get_untracked()),\n                );"
        ),
        "edit-mode must write include_public_datasets unconditionally with the \
         signal's actual value (true AND false), not only when it is true: {block}"
    );
}

#[test]
fn create_and_edit_mode_only_write_include_public_datasets_for_bigquery() {
    let block = catalog_scope_write_block(SRC);

    // `include_public_datasets` is BigQuery-specific (the toggle only
    // renders under `<Show when=move || datasource_type.get() == "bigquery">`
    // in both CreateModeCatalogPicker and EditModeCatalogTab) — the
    // catalog-scope block above it is shared by every provider type via
    // `catalog_config_key_for_type`, so the include_public_datasets
    // write must stay scoped to `t == "bigquery"` rather than writing an
    // irrelevant key into every other provider's connection_config on
    // every save.
    let occurrences = block.matches("if t == \"bigquery\" {").count();
    assert_eq!(
        occurrences, 2,
        "expected exactly one `if t == \\\"bigquery\\\"` guard around the \
         include_public_datasets write in each of create-mode and edit-mode, found \
         {occurrences}: {block}"
    );
}

#[test]
fn catalog_load_back_reads_include_public_datasets_via_the_shared_helper() {
    let snippet = extract_between(
        SRC,
        "set_catalog_selected.try_set(selected_items);",
        "set_bq_include_public.try_set(include_public);",
    );

    assert!(
        snippet.contains("bigquery_include_public(cfg)"),
        "the edit-modal load-back must read include_public_datasets through \
         crate::utils::json::bigquery_include_public — the single place that decides \
         what an absent key means — rather than reimplementing the default inline: \
         {snippet}"
    );
    assert!(
        !snippet.contains("config_bool("),
        "the load-back must not call config_bool directly for include_public_datasets \
         — that was the KYO-446 bug shape: this file's own default (false) disagreed \
         with every other reader, which all defaulted absent to true. Routing through \
         bigquery_include_public is what keeps the default from drifting again: {snippet}"
    );
}

// ── KYO-452: catalog-scope copy must never promise an unqualified
// "index all" outcome ───────────────────────────────────────────────
//
// The catalog-scope placeholder/helper text used to promise "leave
// blank/empty to index all" unconditionally. That promise only holds if
// discovery actually succeeds — for a BigQuery account whose IAM lacks
// `resourcemanager.projects.list` (a normal, least-privilege
// `BigQuery Job User` grant), discovery fails and leaving the field
// blank produces an empty catalog, the opposite of what the copy said.
// Six sites carried a variant of this claim (per the ticket's own grep);
// implementing the fix found a seventh — `CreateModeCatalogPicker`'s
// always-rendered header, worded differently ("leave all unchecked to
// index everything"), which is why a grep for "index all" / "leave
// blank" / "leave empty" missed it. All seven are enumerated
// individually below — a single coarse check would not name which site
// regressed, and a hand-typed "sites I remembered" list would not catch
// an eighth site added later with the same shape (see
// docs/standards/testing/enumerate-the-variant-registry.md).

/// Site 1 — `CreateModeCatalogPicker`'s header, rendered unconditionally
/// above both the checkbox-list and text-fallback branches. It used to
/// claim "leave all unchecked to index everything" regardless of
/// whether discovery had actually returned anything to check. The fix
/// drops the outcome clause entirely: the header now only describes the
/// field, and the qualified promise lives in the branch-specific copy
/// below it instead (covered by sites 2–6).
#[test]
fn create_mode_picker_header_drops_the_unconditional_index_all_promise() {
    let f = extract_between(
        SRC,
        "fn CreateModeCatalogPicker(",
        "fn view_service_account_form(",
    );
    let header = extract_between(f, "\"Catalog Scope\"</h4>", "</p>");
    assert!(
        !header.contains("Leave"),
        "CreateModeCatalogPicker's header must not append an outcome-promising \
         'Leave ...' clause — KYO-452 requires it to only describe the field, never \
         promise an outcome discovery cannot guarantee"
    );
    assert!(
        header.contains("to include in the catalog."),
        "CreateModeCatalogPicker's header lost its descriptive text entirely"
    );
}

/// Sites 2–5 — the four per-provider placeholders in
/// `CreateModeCatalogPicker`'s text-fallback branch. Each used to end in
/// the literal suffix `"(leave blank to index all)"` — an unconditional
/// promise baked into the `placeholder` attribute itself. The fix drops
/// that suffix from the placeholder (a placeholder is the wrong home for
/// a qualified, two-part sentence — DESIGN.md's spacing/density
/// conventions favor a concise placeholder plus adjacent helper text
/// over an overflowing one) and moves the qualified promise into the
/// helper text below (site 6).
#[test]
fn create_mode_picker_placeholders_no_longer_carry_the_index_all_suffix() {
    let block = extract_between(
        SRC,
        "let placeholder = match ds_type.as_str() {",
        "let noun = catalog_item_label_for_type(&ds_type);",
    );
    assert!(
        !block.contains("(leave blank to index all)"),
        "a CreateModeCatalogPicker placeholder still promises an unqualified \
         'leave blank to index all' outcome — KYO-452 regressed"
    );
    for (provider, expected) in [
        (
            "bigquery",
            "\"bigquery\" => \"Enter project IDs, comma-separated\",",
        ),
        (
            "clickhouse/mysql/snowflake",
            "\"clickhouse\" | \"mysql\" | \"snowflake\" => \"Enter database names, comma-separated\",",
        ),
        (
            "databricks",
            "\"databricks\" => \"Enter catalog names, comma-separated\",",
        ),
        ("fallback", "_ => \"Enter schema names, comma-separated\","),
    ] {
        assert!(
            block.contains(expected),
            "CreateModeCatalogPicker's {provider} placeholder does not match the \
             expected KYO-452 wording {expected:?}"
        );
    }
}

/// Site 6 — the helper text under the text-fallback input. Used to be a
/// single hardcoded "Leave blank to index all available items." for
/// every provider. Now built per-provider from
/// `catalog_item_label_for_type` — the same function that already
/// supplies every other per-provider noun in this file (KYO-300) — and
/// qualified with "this account can list", the load-bearing phrase from
/// the ticket's own suggested wording.
#[test]
fn create_mode_picker_fallback_helper_text_is_qualified_per_provider() {
    let f = extract_between(
        SRC,
        "fn CreateModeCatalogPicker(",
        "fn view_service_account_form(",
    );
    assert!(
        !f.contains("Leave blank to index all available items."),
        "CreateModeCatalogPicker's fallback helper text regressed to the old \
         unqualified 'index all available items' promise — KYO-452 regressed"
    );
    assert!(
        f.contains("let noun = catalog_item_label_for_type(&ds_type);"),
        "the fallback helper text must derive its noun from \
         catalog_item_label_for_type — the single source of truth for the \
         per-provider item noun used throughout this file, not a re-typed match"
    );
    assert!(
        f.contains("format!(\"Leave blank to index all {noun} this account can list.\")"),
        "the fallback helper text must carry the qualifier 'this account can list' — \
         the load-bearing part of the KYO-452 fix; without it the promise is \
         unqualified again"
    );
}

/// Site 7 — `EditModeCatalogTab`'s always-visible Catalog-tab header
/// (the second component with catalog-scope copy — `CreateModeCatalogPicker`
/// above is create-mode only). It used to read "...Leave empty to index
/// all available {item_label}." — the same unqualified promise as sites
/// 2–5, worded with "available" instead of "(leave blank to index
/// all)", which is why the ticket's grep for that exact four-variant
/// phrase missed it.
#[test]
fn edit_mode_catalog_tab_header_is_qualified() {
    let f = extract_between(SRC, "fn EditModeCatalogTab(", MOD_TESTS_MARKER);
    assert!(
        !f.contains("Leave empty to index all available"),
        "EditModeCatalogTab's header still promises an unqualified 'index all \
         available' outcome — KYO-452 regressed"
    );
    assert!(
        f.contains("Leave empty to index all "),
        "EditModeCatalogTab's header lost the index-all guidance entirely"
    );
    assert!(
        f.contains("this account can list."),
        "EditModeCatalogTab's header must carry the qualifier 'this account can \
         list' — without it the promise is unqualified again"
    );
}

/// Safety net beyond the seven enumerated sites above: no *new* site
/// introduced anywhere else in this file may adopt the same unqualified
/// shape either. Checked against the exact fragments that made every
/// prior site false — deliberately NOT the bare words "index all" or
/// "index everything" on their own, both of which also appear in copy
/// that is legitimately unqualified-promise-free: the discovered-items
/// branch's own accurate "all (leave unchecked to index everything)"
/// (conditioned on real discovery results already on screen, so the
/// promise is true there), the BYOK "leave blank to keep the existing
/// key" copy, and the Snowflake role field's "leave empty for default".
///
/// KYO-455: on `origin/main` this scoped to `&SRC[..mod_tests_start]`
/// (found via `MOD_TESTS_MARKER`) because `SRC` was `include_str!`-ed
/// from the same file the inline `mod tests` block lived in, and this
/// very test's own `banned` array quotes several of these fragments
/// verbatim — an unscoped `SRC.contains(banned)` would have matched its
/// own literal and could never pass, regardless of production code. Now
/// that the tests live in `datasources/tests/` rather than inline in
/// `datasources.rs`, `SRC` contains production code only — no test
/// source, from this file or any sibling topic file, is reachable
/// through it — so the slice is checked directly against `SRC` with no
/// scoping needed. The assertion's meaning is unchanged (no banned
/// fragment anywhere in production `datasources.rs`); only the
/// now-unnecessary EOF-slicing mechanism is gone.
#[test]
fn no_new_site_introduces_the_kyo_452_unqualified_promise_shape() {
    for banned in [
        "(leave blank to index all)",
        "Leave blank to index all available",
        "Leave empty to index all available",
        "Leave all unchecked to index everything.",
    ] {
        assert!(
            !SRC.contains(banned),
            "found the KYO-452 unqualified promise fragment {banned:?} somewhere in \
             datasources.rs — every catalog-scope copy site must be qualified with \
             'this account can list' (or reworded to not promise an outcome at all)"
        );
    }
}
