// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;

#[test]
fn source_markup_is_inert_in_plain_and_highlighted_code() {
    let code =
        "<img src=x onerror=\"void(0)\"><script>void(0)</script>\nSELECT 'javascript:alert';";
    for language in ["", "sql", "yaml", "unregistered-language"] {
        let html = highlight_html(code, language);
        assert!(!html.contains("<img"), "{language}: {html}");
        assert!(!html.contains("<script"), "{language}: {html}");
        assert!(html.contains("&lt;"), "{language}: {html}");
        assert!(
            html.contains("javascript:alert"),
            "code content must remain visible"
        );
    }
    assert_eq!(highlight_html(code, ""), html_escape(code));
    assert_eq!(
        highlight_html(code, "unregistered-language"),
        html_escape(code)
    );
    assert!(
        highlight_html("SELECT 1", "sql").contains("<a-"),
        "syntax token formatting must survive"
    );
}
