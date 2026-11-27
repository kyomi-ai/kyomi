// SPDX-License-Identifier: AGPL-3.0-or-later

//! Minimal CSS reading shared by the CSS parity guards in this directory
//! (`css_theme_parity.rs`, `chart_app_css_parity.rs`): comment stripping,
//! top-level block splitting and flat declaration parsing. Not a general CSS
//! parser; it handles exactly the shapes the two `main.css` files use.

use std::collections::BTreeMap;

/// Strips `/* ... */` comments. CSS comments don't nest, so a single pass is
/// sufficient. Operates on `char`s (not bytes) so it's safe on any UTF-8
/// content, even though this file's comments are ASCII today.
pub fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            chars.next(); // consume '*'
            let mut prev = '\0';
            for cc in chars.by_ref() {
                if prev == '*' && cc == '/' {
                    break;
                }
                prev = cc;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Splits `css` into `(selector, body)` pairs for every *top-level* `{ ... }`
/// block (depth 0 -> 1 -> 0), via brace-depth counting. Nested blocks (e.g.
/// a selector inside `@media { ... }`) are included verbatim in their
/// parent's body rather than returned separately — this test only ever
/// looks up bodies for known top-level selectors (`:root`, `.dark`,
/// `@theme`), so that's sufficient.
pub fn extract_top_level_blocks(css: &str) -> Vec<(String, String)> {
    let mut blocks = Vec::new();
    let mut depth: i32 = 0;
    let mut selector_start = 0usize;
    let mut body_start = 0usize;
    let mut current_selector = String::new();

    for (idx, ch) in css.char_indices() {
        match ch {
            '{' => {
                if depth == 0 {
                    current_selector = css[selector_start..idx].trim().to_string();
                    body_start = idx + ch.len_utf8();
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                assert!(
                    depth >= 0,
                    "main.css: unbalanced `}}` while scanning for top-level blocks"
                );
                if depth == 0 {
                    blocks.push((current_selector.clone(), css[body_start..idx].to_string()));
                    selector_start = idx + ch.len_utf8();
                }
            }
            _ => {}
        }
    }
    assert_eq!(
        depth, 0,
        "main.css: unbalanced braces (unterminated block) while scanning for top-level blocks"
    );
    blocks
}

/// Parses `name: value;` declarations out of a flat block body (no nested
/// selectors). Keys retain their leading `--`.
pub fn parse_declarations(body: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for decl in body.split(';') {
        let decl = decl.trim();
        if decl.is_empty() {
            continue;
        }
        let Some((name, value)) = decl.split_once(':') else {
            continue;
        };
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        let prev = map.insert(name.clone(), value);
        assert!(
            prev.is_none(),
            "main.css: duplicate declaration of `{name}` within the same block"
        );
    }
    map
}
