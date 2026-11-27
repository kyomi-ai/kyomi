// SPDX-License-Identifier: AGPL-3.0-or-later

//! KYO-1001: parity guard between the MCP chart app's stylesheet
//! (`apps/mcp-chart-app-wasm/style/main.css`) and the main app's
//! (`crates/kyomi-ui/style/main.css`).
//!
//! The chart app restates theme values the main app also defines: the design
//! tokens in `@theme` and `.dark`, and the `--chartml-*` typography in the
//! light `:root` block. It has to restate them (it is a separate Tailwind build
//! that cannot import the main app's file), and nothing failed when the two
//! drifted. This test does: every such declaration in the chart app must equal
//! the main app's, unless it is listed in [`EXPECTED_DIFFERENCES`] with the
//! reason it differs.
//!
//! The dark card and secondary colours and the dark `--chartml-separator` follow
//! the host's styles when the host provides them (see `main.css`), so they are
//! compared by the fallback they keep: the colour the main app uses.
//!
//! Deliberately not compared: the chart app's dark `--chartml-*` overrides
//! (the `:root.dark, :root[data-theme="dark"]` block). They give the chart app a
//! neutral charcoal dark palette (`#262626` surface, `#e5e7eb` text) where the
//! main app's `.dark` block uses Kyomi's warm near-black, because the chart app
//! sits inside another product's dark surface rather than on Kyomi's page. The
//! one value in that block that must follow the surface, `--chartml-separator`,
//! has its own test below.
//!
//! `css_theme_parity.rs` guards the other direction of drift, between
//! `kyomi_theme()` and the main app's `--chartml-*` block.

use std::collections::BTreeMap;
use std::path::PathBuf;

mod support;
use support::css::{extract_top_level_blocks, parse_declarations, strip_css_comments};

/// How a chart app declaration may legitimately differ from the main app's.
enum Difference {
    /// The value is intentionally different for the stated reason.
    Intentional(&'static str),
    /// The value is `var(<host variable>, <the main app's value>)`: it follows
    /// the host's style when the host publishes it and falls back to the colour
    /// the main app uses.
    FollowsHost(&'static str),
}

/// `(block selector, property, how it differs)`. Anything not listed must be
/// equal. An entry that stops being true fails the test, so the list cannot go
/// stale.
const EXPECTED_DIFFERENCES: &[(&str, &str, Difference)] = &[
    (
        "@theme",
        "--font-sans",
        Difference::Intentional("the chart app loads no web fonts, so it keeps the system stack"),
    ),
    (
        "@theme",
        "--font-mono",
        Difference::Intentional("the chart app loads no web fonts, so it keeps the system stack"),
    ),
    (
        "@theme",
        "--container-xs",
        Difference::Intentional(
            "rem values are chosen for each app's root font size: the main app's html is 15px, \
             the chart app's is the browser's 16px; both come to 320px",
        ),
    ),
    (
        "@theme",
        "--container-md",
        Difference::Intentional(
            "rem values are chosen for each app's root font size: the main app's html is 15px, \
             the chart app's is the browser's 16px; both come to 448px",
        ),
    ),
    (".dark", "--color-card", Difference::FollowsHost("--color-background-secondary")),
    (".dark", "--color-secondary", Difference::FollowsHost("--color-background-secondary")),
    (
        ":root",
        "--chartml-bg",
        Difference::Intentional(
            "the chart app draws on the host's surface, not on Kyomi's warm page, so its light \
             separator colour is plain white",
        ),
    ),
    (
        ":root",
        "--chartml-table-surface",
        Difference::Intentional(
            "the chart app's table surface is the host's --color-background-primary (KYO-995), \
             not the main app's card colour",
        ),
    ),
];

fn read_css(repo_relative: &str) -> Vec<(String, String)> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(repo_relative);
    let css = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "KYO-1001 parity guard: could not read {path:?} ({error}). This test enforces that \
             the chart app's stylesheet and the main app's do not drift; if a file moved, update \
             this test rather than skipping it."
        )
    });
    extract_top_level_blocks(&strip_css_comments(&css))
}

/// The declarations of the one top-level block with exactly this selector.
fn unique_block(blocks: &[(String, String)], selector: &str) -> BTreeMap<String, String> {
    let mut matching = blocks.iter().filter(|(candidate, _)| candidate == selector);
    let block = matching
        .next()
        .unwrap_or_else(|| panic!("no top-level `{selector}` block"));
    assert!(matching.next().is_none(), "more than one top-level `{selector}` block");
    parse_declarations(&block.1)
}

/// Every `--chartml-*` declaration across all bare `:root` blocks, with each
/// value it is given (a name given twice shows up twice).
fn root_chartml_declarations(blocks: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut declarations: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (selector, body) in blocks {
        if selector != ":root" && !selector.ends_with("\n:root") {
            continue;
        }
        for (name, value) in parse_declarations(body) {
            if name.starts_with("--chartml-") {
                declarations.entry(name).or_default().push(value);
            }
        }
    }
    declarations
}

fn normalised(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn difference(selector: &str, property: &str) -> Option<&'static Difference> {
    EXPECTED_DIFFERENCES
        .iter()
        .find(|(block, name, _)| *block == selector && *name == property)
        .map(|(_, _, difference)| difference)
}

/// Compare one chart app declaration with the main app's and describe any
/// failure.
fn check(
    selector: &str,
    property: &str,
    chart_app_value: &str,
    main_values: &[String],
    problems: &mut Vec<String>,
) {
    let chart_app_value = normalised(chart_app_value);
    let [main_value] = main_values else {
        problems.push(format!(
            "{selector} {property}: the chart app declares it but the main app gives it {} values \
             ({main_values:?}), so there is nothing single to compare with",
            main_values.len()
        ));
        return;
    };
    let main_value = normalised(main_value);

    match difference(selector, property) {
        None => {
            if chart_app_value != main_value {
                problems.push(format!(
                    "{selector} {property}: chart app has `{chart_app_value}`, main app has \
                     `{main_value}`. Make them equal, or list the difference with its reason in \
                     EXPECTED_DIFFERENCES."
                ));
            }
        }
        Some(Difference::Intentional(reason)) => {
            if chart_app_value == main_value {
                problems.push(format!(
                    "{selector} {property}: listed as intentionally different ({reason}) but the \
                     two now match; remove the entry from EXPECTED_DIFFERENCES."
                ));
            }
        }
        Some(Difference::FollowsHost(host_variable)) => {
            let expected = format!("var({host_variable}, {main_value})");
            if chart_app_value != expected {
                problems.push(format!(
                    "{selector} {property}: chart app has `{chart_app_value}`, expected \
                     `{expected}` — the host's colour when it provides one, the main app's \
                     colour otherwise."
                ));
            }
        }
    }
}

#[test]
fn the_chart_app_theme_values_match_the_main_apps() {
    let chart_app = read_css("apps/mcp-chart-app-wasm/style/main.css");
    let main_app = read_css("crates/kyomi-ui/style/main.css");
    let mut problems = Vec::new();
    let mut compared = 0;

    for selector in ["@theme", ".dark"] {
        let ours = unique_block(&chart_app, selector);
        let theirs = unique_block(&main_app, selector);
        assert!(!ours.is_empty(), "the chart app's `{selector}` block is empty");
        for (property, value) in &ours {
            match theirs.get(property) {
                Some(main_value) => check(selector, property, value, std::slice::from_ref(main_value), &mut problems),
                None => problems.push(format!(
                    "{selector} {property}: the chart app declares it but the main app does not"
                )),
            }
            compared += 1;
        }
    }

    let ours = root_chartml_declarations(&chart_app);
    let theirs = root_chartml_declarations(&main_app);
    assert!(!ours.is_empty(), "the chart app's `:root` block declares no --chartml-* values");
    for (property, values) in &ours {
        let [value] = values.as_slice() else {
            problems.push(format!(":root {property}: the chart app declares it twice"));
            continue;
        };
        match theirs.get(property) {
            Some(main_values) => check(":root", property, value, main_values, &mut problems),
            None => problems.push(format!(
                ":root {property}: the chart app declares it but the main app does not"
            )),
        }
        compared += 1;
    }

    assert!(compared > 30, "only {compared} declarations were compared; the parsing is broken");
    assert!(
        problems.is_empty(),
        "the chart app's stylesheet has drifted from the main app's:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn every_listed_difference_still_names_a_chart_app_declaration() {
    let chart_app = read_css("apps/mcp-chart-app-wasm/style/main.css");
    let theme = unique_block(&chart_app, "@theme");
    let dark = unique_block(&chart_app, ".dark");
    let root = root_chartml_declarations(&chart_app);

    for (selector, property, _) in EXPECTED_DIFFERENCES {
        let present = match *selector {
            "@theme" => theme.contains_key(*property),
            ".dark" => dark.contains_key(*property),
            ":root" => root.contains_key(*property),
            other => panic!("EXPECTED_DIFFERENCES names an unsupported block `{other}`"),
        };
        assert!(
            present,
            "EXPECTED_DIFFERENCES lists {selector} {property}, which the chart app no longer declares"
        );
    }
}

/// The dark separator colour is the colour of the surface the chart sits on, so
/// dot outlines and slice gaps disappear into it. The chart app's surface is the
/// host's (`--chartml-table-surface`), so the separator has to be the same
/// variable rather than a hex copy of its fallback.
#[test]
fn the_dark_separator_follows_the_surface_the_chart_sits_on() {
    let chart_app = read_css("apps/mcp-chart-app-wasm/style/main.css");
    let dark_overrides = chart_app
        .iter()
        .find(|(selector, body)| selector.contains(":root.dark") && body.contains("--chartml-separator"))
        .unwrap_or_else(|| panic!("no dark `--chartml-separator` override in the chart app's stylesheet"));
    let declarations = parse_declarations(&dark_overrides.1);

    assert_eq!(
        declarations.get("--chartml-separator").map(|value| normalised(value)),
        Some("var(--chartml-table-surface)".to_string()),
        "a hex separator stays dark grey on a host whose background is something else"
    );
}
