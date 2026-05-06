// SPDX-License-Identifier: LicenseRef-Alytic-Enterprise

//! Slack formatting helpers — markdown conversion, emoji translation, channel reference parsing.
//!
//! Wire-compatible with the Python helpers in `slack_message_processor.py`
//! and the inline helpers in `slack_integration.py`.

use std::sync::LazyLock;

// ---------------------------------------------------------------------------
// Markdown to Slack mrkdwn conversion
// ---------------------------------------------------------------------------

/// Cached regexes for markdown-to-Slack conversion.
static RE_BOLD: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\*\*(.+?)\*\*").expect("valid regex"));
static RE_BOLD_UNDERSCORE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"__(.+?)__").expect("valid regex"));
static RE_HEADERS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^#{1,6}\s+(.+?)$").expect("valid regex"));
static RE_BULLETS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^(\s*)-\s+").expect("valid regex"));
static RE_LINKS: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\[(.+?)\]\((.+?)\)").expect("valid regex"));
static RE_CHANNEL_REF: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<#([A-Z0-9]+)\|([^>]*)>").expect("valid regex"));

/// Convert markdown to Slack mrkdwn format.
///
/// Conversions:
/// - `**bold**` or `__bold__` -> `*bold*`
/// - `# Header` -> `*Header*`
/// - `- item` -> `\u{2022} item` (bullet)
/// - `[text](url)` -> `<url|text>`
/// - Inline code (`code`) and code blocks are left unchanged (Slack supports backtick-code)
/// - Italic (`*text*`) is left unchanged (Slack uses the same syntax)
pub fn markdown_to_slack(markdown: &str) -> String {
    let mut text = markdown.to_string();

    // Convert bold: **text** or __text__ -> *text*
    text = RE_BOLD.replace_all(&text, "*$1*").into_owned();
    text = RE_BOLD_UNDERSCORE
        .replace_all(&text, "*$1*")
        .into_owned();

    // Convert headers: # Header -> *Header*
    text = RE_HEADERS.replace_all(&text, "*$1*").into_owned();

    // Convert bullet points: - item -> bullet item
    text = RE_BULLETS
        .replace_all(&text, "${1}\u{2022} ")
        .into_owned();

    // Convert links: [text](url) -> <url|text>
    text = RE_LINKS.replace_all(&text, "<$2|$1>").into_owned();

    text
}

// ---------------------------------------------------------------------------
// Emoji translation
// ---------------------------------------------------------------------------

/// Common Slack emoji shortcode to Unicode mappings.
///
/// This covers the most frequently used Slack emoji codes. Custom workspace
/// emojis (e.g. `:company-logo:`) are left as-is.
static EMOJI_MAP: LazyLock<Vec<(&'static str, &'static str)>> = LazyLock::new(|| {
    vec![
        (":wave:", "\u{1F44B}"),
        (":thumbsup:", "\u{1F44D}"),
        (":thumbsdown:", "\u{1F44E}"),
        (":clap:", "\u{1F44F}"),
        (":heart:", "\u{2764}\u{FE0F}"),
        (":fire:", "\u{1F525}"),
        (":rocket:", "\u{1F680}"),
        (":star:", "\u{2B50}"),
        (":star2:", "\u{1F31F}"),
        (":check:", "\u{2705}"),
        (":white_check_mark:", "\u{2705}"),
        (":x:", "\u{274C}"),
        (":warning:", "\u{26A0}\u{FE0F}"),
        (":bulb:", "\u{1F4A1}"),
        (":chart_with_upwards_trend:", "\u{1F4C8}"),
        (":chart_with_downwards_trend:", "\u{1F4C9}"),
        (":bar_chart:", "\u{1F4CA}"),
        (":bell:", "\u{1F514}"),
        (":no_bell:", "\u{1F515}"),
        (":eyes:", "\u{1F440}"),
        (":thinking_face:", "\u{1F914}"),
        (":thinking:", "\u{1F914}"),
        (":tada:", "\u{1F389}"),
        (":party_popper:", "\u{1F389}"),
        (":point_right:", "\u{1F449}"),
        (":point_left:", "\u{1F448}"),
        (":point_up:", "\u{261D}\u{FE0F}"),
        (":point_down:", "\u{1F447}"),
        (":ok_hand:", "\u{1F44C}"),
        (":muscle:", "\u{1F4AA}"),
        (":pray:", "\u{1F64F}"),
        (":raised_hands:", "\u{1F64C}"),
        (":100:", "\u{1F4AF}"),
        (":boom:", "\u{1F4A5}"),
        (":zap:", "\u{26A1}"),
        (":sparkles:", "\u{2728}"),
        (":gear:", "\u{2699}\u{FE0F}"),
        (":wrench:", "\u{1F527}"),
        (":hammer:", "\u{1F528}"),
        (":lock:", "\u{1F512}"),
        (":unlock:", "\u{1F513}"),
        (":key:", "\u{1F511}"),
        (":mag:", "\u{1F50D}"),
        (":mag_right:", "\u{1F50E}"),
        (":memo:", "\u{1F4DD}"),
        (":pencil:", "\u{270F}\u{FE0F}"),
        (":clipboard:", "\u{1F4CB}"),
        (":calendar:", "\u{1F4C5}"),
        (":clock:", "\u{1F550}"),
        (":hourglass:", "\u{231B}"),
        (":email:", "\u{1F4E7}"),
        (":envelope:", "\u{2709}\u{FE0F}"),
        (":package:", "\u{1F4E6}"),
        (":link:", "\u{1F517}"),
        (":globe_with_meridians:", "\u{1F310}"),
        (":earth_americas:", "\u{1F30E}"),
        (":sun_with_face:", "\u{1F31E}"),
        (":cloud:", "\u{2601}\u{FE0F}"),
        (":umbrella:", "\u{2602}\u{FE0F}"),
        (":coffee:", "\u{2615}"),
        (":pizza:", "\u{1F355}"),
        (":hamburger:", "\u{1F354}"),
        (":beer:", "\u{1F37A}"),
        (":trophy:", "\u{1F3C6}"),
        (":medal:", "\u{1F3C5}"),
        (":crown:", "\u{1F451}"),
        (":gem:", "\u{1F48E}"),
        (":money_with_wings:", "\u{1F4B8}"),
        (":moneybag:", "\u{1F4B0}"),
        (":dollar:", "\u{1F4B5}"),
        (":chart:", "\u{1F4B9}"),
        (":heavy_check_mark:", "\u{2714}\u{FE0F}"),
        (":negative_squared_cross_mark:", "\u{274E}"),
        (":question:", "\u{2753}"),
        (":exclamation:", "\u{2757}"),
        (":no_entry:", "\u{26D4}"),
        (":rotating_light:", "\u{1F6A8}"),
        (":red_circle:", "\u{1F534}"),
        (":large_blue_circle:", "\u{1F535}"),
        (":large_green_circle:", "\u{1F7E2}"),
        (":yellow_circle:", "\u{1F7E1}"),
        (":orange_circle:", "\u{1F7E0}"),
        (":white_circle:", "\u{26AA}"),
        (":black_circle:", "\u{26AB}"),
        (":arrow_up:", "\u{2B06}\u{FE0F}"),
        (":arrow_down:", "\u{2B07}\u{FE0F}"),
        (":arrow_right:", "\u{27A1}\u{FE0F}"),
        (":arrow_left:", "\u{2B05}\u{FE0F}"),
        (":smile:", "\u{1F604}"),
        (":grinning:", "\u{1F600}"),
        (":wink:", "\u{1F609}"),
        (":blush:", "\u{1F60A}"),
        (":sweat_smile:", "\u{1F605}"),
        (":joy:", "\u{1F602}"),
        (":cry:", "\u{1F622}"),
        (":sob:", "\u{1F62D}"),
        (":angry:", "\u{1F620}"),
        (":confused:", "\u{1F615}"),
        (":disappointed:", "\u{1F61E}"),
        (":scream:", "\u{1F631}"),
        (":sunglasses:", "\u{1F60E}"),
        (":nerd_face:", "\u{1F913}"),
        (":robot_face:", "\u{1F916}"),
        (":ghost:", "\u{1F47B}"),
        (":skull:", "\u{1F480}"),
        (":poop:", "\u{1F4A9}"),
    ]
});

/// Translate common Slack emoji shortcodes to Unicode characters.
///
/// Custom workspace emojis (e.g. `:company-logo:`) are left unchanged.
///
/// Examples:
/// - `:wave:` -> `\u{1F44B}`
/// - `:thumbsup:` -> `\u{1F44D}`
/// - `:custom-emoji:` -> `:custom-emoji:` (unchanged)
pub fn translate_slack_emojis(text: &str) -> String {
    let mut result = text.to_string();

    for &(code, emoji) in EMOJI_MAP.iter() {
        if result.contains(code) {
            result = result.replace(code, emoji);
        }
    }

    result
}

// ---------------------------------------------------------------------------
// Channel reference parsing
// ---------------------------------------------------------------------------

/// Parse Slack channel references and convert to agent-friendly format.
///
/// Slack encodes channel mentions as `<#CHANNEL_ID|channel_name>` or `<#CHANNEL_ID|>`.
/// This function replaces them with readable text that includes the channel ID
/// so the agent can use it when creating watches.
///
/// Examples:
/// - `<#C0A83MRQABE|reporting>` -> `#reporting (slack_channel_id: C0A83MRQABE)`
/// - `<#C0A83MRQABE|>` -> `#C0A83MRQABE (slack_channel_id: C0A83MRQABE)`
pub fn parse_slack_channel_refs(text: &str) -> String {
    RE_CHANNEL_REF
        .replace_all(text, |caps: &regex::Captures| {
            let channel_id = &caps[1];
            let channel_name = &caps[2];
            let display_name = if channel_name.is_empty() {
                channel_id
            } else {
                channel_name
            };
            format!("#{display_name} (slack_channel_id: {channel_id})")
        })
        .into_owned()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- markdown_to_slack --

    #[test]
    fn md_bold_double_star() {
        assert_eq!(markdown_to_slack("This is **bold** text"), "This is *bold* text");
    }

    #[test]
    fn md_bold_double_underscore() {
        assert_eq!(markdown_to_slack("This is __bold__ text"), "This is *bold* text");
    }

    #[test]
    fn md_headers() {
        assert_eq!(markdown_to_slack("# Header One"), "*Header One*");
        assert_eq!(markdown_to_slack("## Header Two"), "*Header Two*");
        assert_eq!(markdown_to_slack("### Header Three"), "*Header Three*");
    }

    #[test]
    fn md_bullets() {
        let input = "- Item one\n- Item two";
        let result = markdown_to_slack(input);
        assert!(result.contains("\u{2022} Item one"));
        assert!(result.contains("\u{2022} Item two"));
    }

    #[test]
    fn md_indented_bullets() {
        let input = "  - Nested item";
        let result = markdown_to_slack(input);
        assert!(result.contains("  \u{2022} Nested item"));
    }

    #[test]
    fn md_links() {
        assert_eq!(
            markdown_to_slack("[Kyomi](https://kyomi.ai)"),
            "<https://kyomi.ai|Kyomi>"
        );
    }

    #[test]
    fn md_code_block_language_preserved() {
        // Python parity: language specifiers are NOT stripped from code blocks
        let input = "```sql\nSELECT * FROM users\n```";
        let result = markdown_to_slack(input);
        assert!(result.contains("```sql\nSELECT * FROM users\n```"));
    }

    #[test]
    fn md_inline_code_unchanged() {
        let input = "Use `SELECT *` here";
        let result = markdown_to_slack(input);
        assert_eq!(result, "Use `SELECT *` here");
    }

    #[test]
    fn md_plain_text_unchanged() {
        let input = "Just plain text";
        assert_eq!(markdown_to_slack(input), "Just plain text");
    }

    #[test]
    fn md_mixed() {
        let input = "## Revenue Report\n\n**Revenue** was $100K.\n- North: $50K\n- South: $50K\n\n[View](https://kyomi.ai)";
        let result = markdown_to_slack(input);
        assert!(result.contains("*Revenue Report*"));
        assert!(result.contains("*Revenue*"));
        assert!(result.contains("\u{2022} North: $50K"));
        assert!(result.contains("<https://kyomi.ai|View>"));
    }

    // -- translate_slack_emojis --

    #[test]
    fn emoji_wave() {
        assert_eq!(translate_slack_emojis(":wave:"), "\u{1F44B}");
    }

    #[test]
    fn emoji_thumbsup() {
        assert_eq!(translate_slack_emojis(":thumbsup:"), "\u{1F44D}");
    }

    #[test]
    fn emoji_multiple() {
        let result = translate_slack_emojis("Hello :wave: and :thumbsup:");
        assert!(result.contains("\u{1F44B}"));
        assert!(result.contains("\u{1F44D}"));
        assert!(!result.contains(":wave:"));
        assert!(!result.contains(":thumbsup:"));
    }

    #[test]
    fn emoji_custom_unchanged() {
        let input = ":company-logo: is custom";
        assert_eq!(translate_slack_emojis(input), input);
    }

    #[test]
    fn emoji_no_emojis() {
        let input = "No emojis here";
        assert_eq!(translate_slack_emojis(input), input);
    }

    #[test]
    fn emoji_chart_related() {
        assert!(translate_slack_emojis(":bar_chart:").contains("\u{1F4CA}"));
        assert!(translate_slack_emojis(":bell:").contains("\u{1F514}"));
        assert!(translate_slack_emojis(":fire:").contains("\u{1F525}"));
    }

    // -- parse_slack_channel_refs --

    #[test]
    fn channel_ref_with_name() {
        assert_eq!(
            parse_slack_channel_refs("<#C0A83MRQABE|reporting>"),
            "#reporting (slack_channel_id: C0A83MRQABE)"
        );
    }

    #[test]
    fn channel_ref_without_name() {
        assert_eq!(
            parse_slack_channel_refs("<#C0A83MRQABE|>"),
            "#C0A83MRQABE (slack_channel_id: C0A83MRQABE)"
        );
    }

    #[test]
    fn channel_ref_multiple() {
        let input = "Post to <#C111|general> or <#C222|alerts>";
        let result = parse_slack_channel_refs(input);
        assert!(result.contains("#general (slack_channel_id: C111)"));
        assert!(result.contains("#alerts (slack_channel_id: C222)"));
    }

    #[test]
    fn channel_ref_no_refs() {
        let input = "No channel refs here";
        assert_eq!(parse_slack_channel_refs(input), input);
    }

    #[test]
    fn channel_ref_mixed_with_text() {
        let input = "Send alerts to <#C0A83|ops> for monitoring";
        let result = parse_slack_channel_refs(input);
        assert_eq!(
            result,
            "Send alerts to #ops (slack_channel_id: C0A83) for monitoring"
        );
    }
}
