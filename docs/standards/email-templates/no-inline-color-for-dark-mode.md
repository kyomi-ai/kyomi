# Never put `color:` in inline styles on elements that need dark mode overrides

Per the CSS spec, `!important` rules in `<style>` blocks DO override inline styles. However, for email templates, the practical rule is simpler: keep `color` values out of inline styles entirely.

**Why:** Consistency and maintainability. With `color` only in class/element rules, you have one place to update for light mode and one place for dark mode. Inline `color` is never needed because email clients that strip `<style>` blocks (Gmail) still render readable text with browser defaults. This is unlike `background-color` on containers (see next rule).

**Rule:** For any text element, put `color` values in class-based CSS rules only, not inline. Inline styles should only contain layout properties (`padding`, `margin`, `font-size`, `font-weight`, `vertical-align`, `text-align`).

```html
<!-- WRONG — inline color is unnecessary; Gmail renders readable text without it -->
<td style="color:#1C1917; padding:12px 16px;">Label</td>
<!-- Removing it lets td { color: #A8A29E !important; } apply cleanly in dark mode -->

<!-- RIGHT — color comes from class rule, inline has only layout -->
<td style="padding:12px 16px;">Label</td>
<!-- Light mode: td { color: #1C1917; } applies -->
<!-- Dark mode: td { color: #A8A29E !important; } overrides -->
```

This applies to dynamically-built rows too (e.g. `push_str` loops building `<td>` elements). The pattern was missed twice in KYO-6 — once in `alert.rs` and once in `feedback_service.rs` — because dynamic row construction happened before the `html_body` template and was not updated alongside static rows.
