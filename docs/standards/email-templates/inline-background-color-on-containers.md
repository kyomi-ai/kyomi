# Inline `background-color` on containers IS correct for email dark mode

Unlike text `color`, structural elements (`<tr>`, `<td>` containers, content cards) SHOULD have inline `background-color` for light mode. This is the standard email dark mode pattern:

- Inline `background-color` provides the light-mode fallback for clients that strip `<style>` (Gmail)
- `@media (prefers-color-scheme: dark) { tr { background-color: #1A1715 !important; } }` overrides it in clients that support dark mode (Apple Mail, iOS Mail)

Per the CSS Cascading and Inheritance spec, `!important` author stylesheet declarations outrank normal inline `style=""` attributes. This was confirmed in the KYO-8 review dispute (2026-05-17).

```html
<!-- CORRECT — inline bg for Gmail fallback, !important dark override works -->
<tr style="background-color: #FAFAF8;">
  <td style="padding: 12px 16px;">Label</td>
</tr>
<!-- Dark mode: tr { background-color: #1A1715 !important; } wins over inline -->
```

**Do NOT confuse this with the `color:` rule above.** Text `color` should never be inline because it's not needed for Gmail fallback (browser defaults handle text). Container `background-color` is inline because without it, Gmail renders a transparent/white background regardless of your design.
