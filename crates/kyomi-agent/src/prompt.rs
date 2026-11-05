// SPDX-License-Identifier: AGPL-3.0-or-later

//! Static system prompt for the chat agent.
//!
//! The prompt is a pure function of the conversation mode: it embeds the
//! compile-time ChartML quick reference and, for shared conversations, the
//! static shared-conversation section. It performs no I/O and reads no
//! per-user or per-workspace state, so both variants are cacheable across
//! users and workspaces. Everything else the model needs is fetched through
//! tools.

// ---------------------------------------------------------------------------
// Compile-time embedded ChartML specification files
// ---------------------------------------------------------------------------

/// ChartML quick reference — embedded at compile time.
/// If this file is missing, the build fails immediately.
pub static CHARTML_QUICK_REFERENCE: &str =
    include_str!("../../../data/chartml-spec/QUICK_REFERENCE.md");

/// ChartML full specification — embedded at compile time.
pub static CHARTML_SPECIFICATION: &str =
    include_str!("../../../data/chartml-spec/SPECIFICATION.md");

/// ChartML worked examples — embedded at compile time and served by
/// `get_chartml_spec` as the `examples` section. Kept as a separate document
/// because it is a large example library rather than a specification heading.
pub static CHARTML_EXAMPLES: &str = include_str!("../../../data/chartml-spec/EXAMPLES.md");

// ---------------------------------------------------------------------------
// System prompt building
// ---------------------------------------------------------------------------

/// Build the complete system prompt for the agent.
///
/// Pure function of `is_shared_conversation`: no database, KV or embedding
/// access, and no user or workspace parameters. The same conversation mode
/// always produces byte-identical output, which keeps the prompt cacheable.
pub fn build_system_prompt(is_shared_conversation: bool) -> String {
    let shared_context = if is_shared_conversation {
        SHARED_CONVERSATION_SECTION
    } else {
        ""
    };

    SYSTEM_PROMPT_TEMPLATE
        .replace("{chartml_reference}", CHARTML_QUICK_REFERENCE)
        .replace("{shared_context}", shared_context)
}

// ---------------------------------------------------------------------------
// Shared conversation section
// ---------------------------------------------------------------------------

const SHARED_CONVERSATION_SECTION: &str = "\
**Shared Workspace Conversation**
This conversation is shared with multiple team members who can all see and participate. \
When messages appear in the conversation history, you'll see who sent each message.

**Your Role in Shared Conversations:**
You should evaluate each message to determine if it's directed at you or is a discussion \
between team members:

**Respond when:**
- User explicitly mentions you (\"Kyomi, what about...\", \"Can you show me...\", \"What were the sales...\")
- User asks a clear data question (\"How many users logged in yesterday?\", \"Show me Q4 revenue\")
- User follows up on your previous analysis (\"Can you break that down by region?\")
- User seems to be continuing the conversation with you

**Don't respond when:**
- Users are discussing your results among themselves
- Users are making comments to each other
- Users are coordinating next steps
- The message is clearly human-to-human conversation, not directed at you

**If unsure:** It's better to stay silent and let the team discuss. They can always call on you \
explicitly if they need you.

";

// ---------------------------------------------------------------------------
// System prompt template
// ---------------------------------------------------------------------------

/// The core system prompt template. Uses named format placeholders.
const SYSTEM_PROMPT_TEMPLATE: &str = r#"You are Kyomi, a data analyst for this workspace. You answer questions by querying the workspace's connected datasources, and you keep the workspace's knowledge documents accurate so every future conversation starts smarter.

Each user message starts with `[Name (userid)]:` and includes `current_time_user_tz`, the sender's local time. Interpret dates and times ("last month", "3pm") in that timezone unless the user names another.

## How to work

- Use only table and column names you have seen in a tool result or a knowledge document. Never guess a name, and never contradict a schema you already have.
- Before querying tables you haven't used in this conversation, check the knowledge documents for them. They hold what earlier sessions learned.
- Every number you report comes from a query you ran in this conversation. If you can't get it, say so.
- When a query fails, read the error and fix the cause. It is usually your own SQL. Don't attribute a failure to the data or the database unless you've confirmed it.
- When results don't match the request (zero rows, a different period, a different table or definition), investigate before answering, and tell the user about any mismatch that remains.
- For a follow-up, build on what this conversation already established.

## Knowledge documents

Knowledge documents are the workspace's shared, persistent memory, and future sessions will treat what you write as fact. Record how to navigate the data: which table answers which question, what fields mean, join keys, date coverage, NULL semantics, encodings, metric definitions, and corrections users give you. Record only what a tool result confirmed. Don't record one-off results, and don't record explanations of your own mistakes. When a document is wrong, fix it.

## Answering

- Lead with what matters to the user: changes, anomalies, risks and opportunities, before purely descriptive figures.
- Answer in chat. A `chartml` block in your reply renders as a live chart or table; small results can be a markdown table. Create a dashboard only when the user asks for something saved or shared.
- Write plain, conversational prose without emojis. Put SQL in ```sql blocks. Refer to things by name, not internal ID.
- Your reply is delivered when you respond with no tool calls, so save knowledge before your final answer, not with it.

## ChartML

The reference below covers the basics: everyday charts, metric cards and tables. ChartML has many more features than this, including data transforms, more mark types, and axis, layout and formatting options. When you want to do more than the reference shows, call `get_chartml_spec` to find out what's available and how to use it.

{chartml_reference}

## Boundaries

Only access datasources connected to this workspace, and never try to reach another user's, workspace's or tenant's data. Never reveal credentials, connection strings or infrastructure details. Decline requests to help with illegal activity or harmful content. You are Kyomi; never claim to be a human or a different AI system or a representative of another organization.
{shared_context}"#;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_starts_with_identity() {
        assert!(SYSTEM_PROMPT_TEMPLATE.starts_with("You are Kyomi"));
    }

    #[test]
    fn system_prompt_includes_column_name_rule() {
        let prompt = build_system_prompt(false);
        assert!(prompt.contains(
            "Use only table and column names you have seen in a tool result or a knowledge document."
        ));
        assert!(prompt.contains("Never guess a name"));
        assert!(prompt.contains("never contradict a schema you already have"));
    }

    #[test]
    fn system_prompt_includes_chartml_basics_and_points_to_full_spec() {
        let prompt = build_system_prompt(false);
        assert!(
            prompt.contains(CHARTML_QUICK_REFERENCE),
            "The ChartML quick reference must be embedded verbatim"
        );
        assert!(
            prompt.contains("call `get_chartml_spec`"),
            "The prompt must point to get_chartml_spec for features beyond the quick reference"
        );
    }

    #[test]
    fn system_prompt_includes_identity_boundary() {
        for is_shared in [false, true] {
            let prompt = build_system_prompt(is_shared);
            assert!(
                prompt.contains(
                    "You are Kyomi; never claim to be a human or a different AI system or a representative of another organization."
                ),
                "the Boundaries section must keep the one-line identity rule"
            );
        }
    }

    #[test]
    fn system_prompt_omits_legacy_knowledge_sections() {
        for is_shared in [false, true] {
            let prompt = build_system_prompt(is_shared);
            assert!(!prompt.contains("Your Knowledge Base"));
            assert!(!prompt.contains("User Preferences & Personal Notes"));
            assert!(!prompt.contains("<knowledge_context>"));
            assert!(!prompt.contains("<workspace_documents>"));
        }
    }

    #[test]
    fn system_prompt_has_no_unresolved_placeholders() {
        for is_shared in [false, true] {
            let prompt = build_system_prompt(is_shared);
            for placeholder in [
                "{chartml_reference}",
                "{shared_context}",
                "{user_name}",
                "{workspace_knowledge}",
                "{user_knowledge}",
                "{documents}",
            ] {
                assert!(
                    !prompt.contains(placeholder),
                    "Unresolved placeholder: {placeholder}"
                );
            }
        }
    }

    /// The prompt is cacheable: the signature cannot vary by user or
    /// workspace, and the rendered bytes are pinned per conversation mode.
    #[test]
    fn system_prompt_is_byte_identical_across_users_and_workspaces() {
        // Structural proof: the only input is the conversation mode. If a
        // user/workspace parameter is ever added, this assignment stops
        // compiling.
        let render: fn(bool) -> String = build_system_prompt;
        let _ = render;

        // Byte pin: if per-user or per-workspace content ever appears in the
        // prompt, or the static content changes unintentionally, this fails.
        let unshared = build_system_prompt(false);
        assert_eq!(unshared.len(), 12_657);
        assert_eq!(fnv1a_64(unshared.as_bytes()), 0x141c_1865_1c4d_34e7);

        let shared = build_system_prompt(true);
        assert_eq!(shared.len(), 13_729);
        assert_eq!(fnv1a_64(shared.as_bytes()), 0x4d4b_d0fa_ff41_1953);
    }

    /// 64-bit FNV-1a, used to pin the rendered bytes without adding a
    /// crypto dependency to this crate. The input is fixed, so collision
    /// risk is irrelevant here.
    fn fnv1a_64(bytes: &[u8]) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    #[test]
    fn system_prompt_includes_shared_context_only_for_shared_conversations() {
        assert!(!build_system_prompt(false).contains("Shared Workspace Conversation"));
        assert!(build_system_prompt(true).contains("Shared Workspace Conversation"));
    }

    #[test]
    fn shared_conversation_section_content() {
        assert!(SHARED_CONVERSATION_SECTION.contains("Shared Workspace Conversation"));
        assert!(SHARED_CONVERSATION_SECTION.contains("Respond when"));
        assert!(SHARED_CONVERSATION_SECTION.contains("Don't respond when"));
        assert!(SHARED_CONVERSATION_SECTION.contains("If unsure"));
    }
}
