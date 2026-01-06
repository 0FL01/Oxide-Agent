//! Utility functions for text processing, HTML cleaning, and message formatting.
//!
//! This module uses the `lazy-regex` crate for efficient and safe regular expression handling.
//! Key benefits include:
//! - **Compile-time validation**: Regex patterns are checked during compilation, preventing runtime panics due to syntax errors.
//! - **Lazy initialization**: Regex objects are initialized only when first used, improving startup performance.
//! - **Static integration**: Patterns are stored in static variables using the `lazy_regex!` macro.

// Allow non_std_lazy_statics because we use lazy_regex! macro which uses once_cell internally
// This is intentional and safe - lazy_regex! validates regex at compile time
#![allow(clippy::non_std_lazy_statics)]

use anyhow::Result;
use lazy_regex::lazy_regex;
use std::time::Duration;
use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::Retry;
use tracing::warn;
use unicode_segmentation::UnicodeSegmentation;
use uuid::Uuid;

/// Match code blocks: ```...```
static RE_CODE_BLOCK: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"```[\s\S]*?```");

/// Match code blocks with optional language: ```language\ncode```
static RE_CODE_BLOCK_FENCE: lazy_regex::Lazy<regex::Regex> =
    lazy_regex!(r"```(\w+)?\n([\s\S]*?)```");

/// Match bullet points at start of line: *
static RE_BULLET: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"(?m)^\* ");

/// Match bold text: **text**
static RE_BOLD: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"\*\*(.*?)\*\*");

/// Match italic text: *text*
static RE_ITALIC: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"\*(.*?)\*");

/// Match inline code: `code`
static RE_INLINE_CODE: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"`(.*?)`");

/// Match 3+ consecutive newlines
static RE_MULTI_NEWLINE: lazy_regex::Lazy<regex::Regex> = lazy_regex!(r"\n{3,}");

/// Replace naked angle brackets with HTML entities, preserving Telegram-allowed HTML tags.
///
/// This function uses an iterator-based approach (instead of allocating Vec<char>)
/// for better performance on long texts.
fn escape_angle_brackets(text: &str) -> String {
    // Whitelist of HTML tags supported by Telegram
    const TELEGRAM_ALLOWED_TAGS: &[&str] = &[
        "b", "i", "u", "s", "code", "pre", "a", "/b", "/i", "/u", "/s", "/code", "/pre", "/a",
    ];

    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_tag = false;

    while let Some(c) = chars.next() {
        match c {
            '<' => {
                // Look ahead to see if this starts an HTML tag like </a or <b
                let mut lookahead = String::new();
                let mut peeked_chars = Vec::new();

                // Check for closing tag: </
                if chars.peek() == Some(&'/') {
                    peeked_chars.push(chars.next().unwrap());
                    lookahead.push('/');
                }

                // Extract tag name (alphanumeric only, stops at space or >)
                while let Some(&next_char) = chars.peek() {
                    if next_char.is_ascii_alphanumeric() {
                        peeked_chars.push(chars.next().unwrap());
                        lookahead.push(next_char);
                    } else {
                        break;
                    }
                }

                // Check if it's a whitelisted tag
                if !lookahead.is_empty() && TELEGRAM_ALLOWED_TAGS.contains(&lookahead.as_str()) {
                    result.push('<');
                    result.push_str(&lookahead);
                    in_tag = true;
                } else {
                    // Not a whitelisted tag, escape the <
                    result.push_str("&lt;");
                    // Put back the peeked characters
                    for peeked in peeked_chars {
                        result.push(peeked);
                    }
                }
            }
            '>' => {
                if in_tag {
                    result.push(c);
                    in_tag = false;
                } else {
                    result.push_str("&gt;");
                }
            }
            _ => {
                result.push(c);
            }
        }
    }
    result
}

/// Cleans HTML content by escaping naked angle brackets while preserving code blocks and valid HTML tags.
///
/// This function uses `RE_CODE_BLOCK` (a compile-time validated regex via `lazy_regex!`)
/// to identify and protect code blocks from escaping.
///
/// # Examples
///
/// ```
/// use another_chat_rs::utils::clean_html;
/// let input = "Check this: 1 < 2 but <b>bold</b> works";
/// let cleaned = clean_html(input);
/// assert_eq!(cleaned, "Check this: 1 &lt; 2 but <b>bold</b> works");
/// ```
pub fn clean_html(text: &str) -> String {
    // Use UUID-based placeholders to prevent collision attacks
    // (user text containing "__CODE_BLOCK_0__" won't be replaced)
    let mut code_blocks: Vec<(String, String)> = Vec::new();
    let mut text_owned = text.to_string();

    // Replace code blocks with UUID-based placeholders
    let mut result = String::new();
    let mut last_end = 0;
    for mat in RE_CODE_BLOCK.find_iter(&text_owned) {
        result.push_str(&text_owned[last_end..mat.start()]);
        let uuid = Uuid::new_v4().as_simple().to_string();
        let placeholder = format!("__CODE_BLOCK_{uuid}__");
        code_blocks.push((placeholder.clone(), mat.as_str().to_string()));
        result.push_str(&placeholder);
        last_end = mat.end();
    }
    result.push_str(&text_owned[last_end..]);
    text_owned = result;

    // Replace naked < and > using our custom function
    text_owned = escape_angle_brackets(&text_owned);

    // Restore code blocks
    for (placeholder, block) in code_blocks {
        text_owned = text_owned.replace(&placeholder, &block);
    }

    text_owned
}

/// Formats markdown-like text into Telegram-compatible HTML.
///
/// Supported formatting:
/// - Code blocks: ` ```language\ncode``` ` -> `<pre><code class="language">code</code></pre>`
/// - Bullets: `* ` at the start of a line -> `• `
/// - Bold: `**text**` -> `<b>text</b>`
/// - Italic: `*text*` -> `<i>text</i>`
/// - Inline code: `` `code` `` -> `<code>code</code>`
/// - Multiple newlines (3+) are collapsed into two.
///
/// All regex patterns used here are compile-time validated using the `lazy_regex!` macro
/// to ensure safety and performance.
///
/// # Examples
///
/// ```
/// use another_chat_rs::utils::format_text;
/// let input = "**Bold** and *italic* with `code`";
/// let formatted = format_text(input);
/// assert_eq!(formatted, "<b>Bold</b> and <i>italic</i> with <code>code</code>");
/// ```
pub fn format_text(text: &str) -> String {
    let mut text_owned = clean_html(text);

    // Replace blocks: ```language\ncode``` -> <pre><code class="language">code</code></pre>
    text_owned = RE_CODE_BLOCK_FENCE
        .replace_all(&text_owned, |caps: &regex::Captures| {
            let lang = caps.get(1).map_or("", |m| m.as_str());
            let code = caps.get(2).map_or("", |m| m.as_str()).trim();
            let escaped_code = html_escape::encode_text(code);
            format!("<pre><code class=\"{lang}\">{escaped_code}</code></pre>")
        })
        .to_string();

    // Replace bullets: * -> •
    text_owned = RE_BULLET.replace_all(&text_owned, "• ").to_string();

    // Replace bold: **text** -> <b>text</b>
    text_owned = RE_BOLD.replace_all(&text_owned, "<b>$1</b>").to_string();

    // Replace italic: *text* -> <i>text</i>
    text_owned = RE_ITALIC.replace_all(&text_owned, "<i>$1</i>").to_string();

    // Replace inline code: `code` -> <code>code</code>
    text_owned = RE_INLINE_CODE
        .replace_all(&text_owned, |caps: &regex::Captures| {
            let code = caps.get(1).map_or("", |m| m.as_str());
            let escaped_code = html_escape::encode_text(code);
            format!("<code>{escaped_code}</code>")
        })
        .to_string();

    // Replace 3+ newlines with 2
    text_owned = RE_MULTI_NEWLINE
        .replace_all(&text_owned, "\n\n")
        .to_string();

    text_owned.trim().to_string()
}

/// Splits a long message into multiple parts that fit within Telegram's message limit.
///
/// This function respects code blocks (triple backticks) and tries to close/reopen them
/// across message boundaries to maintain proper formatting in Telegram.
///
/// **Edge case handling:**
/// - If a single line exceeds `max_length`, it will be split by grapheme clusters (Unicode-safe).
/// - This prevents failures on very long lines without newlines.
///
/// # Arguments
///
/// * `message` - The string to split.
/// * `max_length` - Maximum allowed length for each part.
///
/// # Returns
///
/// A vector of strings, each within the specified length limit.
///
/// # Examples
///
/// ```
/// use another_chat_rs::utils::split_long_message;
/// let long_msg = "A very long message...\n".repeat(300);
/// let parts = split_long_message(&long_msg, 4096);
/// assert!(parts.len() > 1);
/// ```
#[must_use]
pub fn split_long_message(message: &str, max_length: usize) -> Vec<String> {
    if message.is_empty() {
        return Vec::new();
    }

    if message.len() <= max_length {
        return vec![message.to_string()];
    }

    let mut parts = Vec::new();
    let mut current_message = String::new();
    let mut code_block = false;
    let code_fence = "```";

    for line in message.lines() {
        // Handle very long lines without newlines (edge case)
        if line.len() > max_length {
            // If we have content in current_message, flush it first
            if !current_message.is_empty() {
                if code_block {
                    current_message.push_str(code_fence);
                    current_message.push('\n');
                }
                parts.push(current_message.trim_end().to_string());
                current_message.clear();
                if code_block {
                    current_message.push_str(code_fence);
                    current_message.push('\n');
                }
            }

            // Split the long line by grapheme clusters (Unicode-safe)
            let graphemes: Vec<&str> = line.graphemes(true).collect();
            let mut chunk = String::new();
            for grapheme in graphemes {
                if chunk.len() + grapheme.len() > max_length {
                    parts.push(chunk.trim_end().to_string());
                    chunk.clear();
                }
                chunk.push_str(grapheme);
            }
            if !chunk.is_empty() {
                current_message.push_str(&chunk);
                current_message.push('\n');
            }
            continue;
        }

        if line.starts_with(code_fence) {
            code_block = !code_block;
        }

        let new_length = current_message.len() + line.len() + 1; // +1 for newline

        if new_length > max_length && !current_message.is_empty() {
            if code_block {
                current_message.push_str(code_fence);
                current_message.push('\n');
            }

            parts.push(current_message.trim_end().to_string());
            current_message.clear();

            if code_block {
                current_message.push_str(code_fence);
                current_message.push('\n');
                if !line.starts_with(code_fence) {
                    current_message.push_str(line);
                    current_message.push('\n');
                }
            } else {
                current_message.push_str(line);
                current_message.push('\n');
            }
        } else {
            current_message.push_str(line);
            current_message.push('\n');
        }
    }

    if !current_message.is_empty() {
        if code_block {
            current_message.push_str(code_fence);
            current_message.push('\n');
        }
        parts.push(current_message.trim_end().to_string());
    }

    parts
}

/// Safely truncates a string to a maximum character length (not bytes).
///
/// This is UTF-8 safe and will not panic on multi-byte characters.
///
/// # Examples
///
/// ```
/// use another_chat_rs::utils::truncate_str;
/// let s = "Привет, мир!";
/// assert_eq!(truncate_str(s, 6), "Привет");
/// ```
pub fn truncate_str(s: impl AsRef<str>, max_chars: usize) -> String {
    let s = s.as_ref();
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.char_indices()
        .nth(max_chars)
        .map_or_else(|| s.to_string(), |(pos, _)| s[..pos].to_string())
}

/// Retry a Telegram API operation with exponential backoff.
///
/// This function is specifically designed for Telegram API file operations
/// (e.g., `get_file` + `download_file`) that may fail due to transient network errors.
///
/// The retry strategy uses exponential backoff with jitter to avoid thundering herd:
/// - Initial delay: 500ms
/// - Max delay: 4s
/// - Max attempts: 3 (configurable via constants in `config.rs`)
///
/// # Arguments
///
/// * `operation` - An async closure that performs the operation and returns `Result<T>`
///
/// # Returns
///
/// Returns the result of the operation if successful within max attempts,
/// or the last error if all attempts fail.
///
/// # Examples
///
/// ```no_run
/// use another_chat_rs::utils::retry_telegram_operation;
/// use anyhow::Result;
///
/// async fn download_file() -> Result<Vec<u8>> {
///     // ... your download logic
///     Ok(vec![])
/// }
///
/// # async fn example() -> Result<()> {
/// let buffer = retry_telegram_operation(|| async {
///     download_file().await
/// }).await?;
/// # Ok(())
/// # }
/// ```
pub async fn retry_telegram_operation<F, Fut, T>(operation: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    use crate::config::{
        TELEGRAM_API_INITIAL_BACKOFF_MS, TELEGRAM_API_MAX_BACKOFF_MS, TELEGRAM_API_MAX_RETRIES,
    };

    let retry_strategy = ExponentialBackoff::from_millis(TELEGRAM_API_INITIAL_BACKOFF_MS)
        .max_delay(Duration::from_millis(TELEGRAM_API_MAX_BACKOFF_MS))
        .map(jitter) // Add jitter to prevent thundering herd
        .take(TELEGRAM_API_MAX_RETRIES);

    Retry::spawn(retry_strategy, operation).await.map_err(|e| {
        warn!(
            "Telegram API operation failed after {} attempts: {}",
            TELEGRAM_API_MAX_RETRIES, e
        );
        e
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_str_unicode() {
        let s = "Привет, мир!";
        assert_eq!(truncate_str(s, 6), "Привет");
        assert_eq!(truncate_str(s, 50), "Привет, мир!");
    }

    #[test]
    fn test_clean_html_preserves_code_blocks() {
        // We use `< 3` instead of `<tag>` to ensure it's treated as a naked bracket,
        // not a potential HTML tag (which clean_html preserves).
        let input = "Start\n```rust\nfn main() {\n    println!(\"<hello>\");\n}\n```\nEnd < 3";
        let expected =
            "Start\n```rust\nfn main() {\n    println!(\"<hello>\");\n}\n```\nEnd &lt; 3";
        assert_eq!(clean_html(input), expected);
    }

    #[test]
    fn test_clean_html_escapes_naked_brackets() {
        let input = "1 < 2 and 3 > 1 but <b>bold</b> and <a href=''>link</a>";
        let expected = "1 &lt; 2 and 3 &gt; 1 but <b>bold</b> and <a href=''>link</a>";
        assert_eq!(clean_html(input), expected);
    }

    #[test]
    fn test_format_text_markdown() {
        // Note: Bullets must be at start of line
        let input = "* Bullet\nAnd **bold** text\nAnd *italic*\nAnd `inline code`";
        let expected =
            "• Bullet\nAnd <b>bold</b> text\nAnd <i>italic</i>\nAnd <code>inline code</code>";
        assert_eq!(format_text(input), expected);
    }

    #[test]
    fn test_format_text_code_blocks() {
        let input = "Code:\n```rust\nlet x = 1;\n```";
        let expected = "Code:\n<pre><code class=\"rust\">let x = 1;</code></pre>";
        assert_eq!(format_text(input), expected);
    }

    #[test]
    fn test_format_text_multi_newline() {
        let input = "Line 1\n\n\n\nLine 2";
        let expected = "Line 1\n\nLine 2";
        assert_eq!(format_text(input), expected);
    }

    #[test]
    fn test_split_long_message_simple() {
        let input = "Line 1\nLine 2\nLine 3";
        // Max length 13. "Line 1\n" is 7. "Line 2\n" is 7. 7+7=14 > 13.
        let parts = split_long_message(input, 13);
        assert_eq!(parts, vec!["Line 1", "Line 2", "Line 3"]);
    }

    #[test]
    fn test_split_long_message_with_code_block() {
        let input = "Start\n```\nLine 1\nLine 2\n```\nEnd";
        let parts = split_long_message(input, 15);

        assert!(parts.len() > 1);
        assert!(parts[0].ends_with("```"));
        assert!(parts[1].starts_with("```"));
    }

    #[test]
    fn test_clean_html_unsupported_tags() {
        let input = "Text with <arg_key>value</arg_key> and <custom>tag</custom>";
        let expected =
            "Text with &lt;arg_key&gt;value&lt;/arg_key&gt; and &lt;custom&gt;tag&lt;/custom&gt;";
        assert_eq!(clean_html(input), expected);
    }

    #[test]
    fn test_tavily_result_escaping() {
        let input = "Result: <arg_key>some value</arg_key> in text";
        let cleaned = clean_html(input);
        assert!(!cleaned.contains("<arg_key>"));
        assert!(cleaned.contains("&lt;arg_key&gt;"));
    }

    #[test]
    fn test_clean_html_double_escaping_prevention() {
        // Simulate what would happen if tool_call XML leaked into content
        let input = "Result: <arg_key>some value</arg_key> in text";
        let cleaned = clean_html(input);
        // Should escape unsupported tags once
        assert_eq!(
            cleaned,
            "Result: &lt;arg_key&gt;some value&lt;/arg_key&gt; in text"
        );

        // Should NOT double-escape if run again
        let double_cleaned = clean_html(&cleaned);
        assert_eq!(cleaned, double_cleaned);
    }

    #[test]
    fn test_clean_html_xml_like_tags() {
        // Test various XML-like tags that could leak from LLM tool calls
        let input = "Text with <arg_value>data</arg_value> and <tool_name>search</tool_name>";
        let cleaned = clean_html(input);
        assert!(!cleaned.contains("<arg_value>"));
        assert!(!cleaned.contains("<tool_name>"));
        assert!(cleaned.contains("&lt;arg_value&gt;"));
        assert!(cleaned.contains("&lt;tool_name&gt;"));
    }

    #[test]
    fn test_clean_html_placeholder_injection_attack() {
        // CRITICAL: Test that user text containing placeholder-like strings
        // doesn't get replaced with actual code blocks
        let input = "User says: __CODE_BLOCK_0__\n```rust\nfn evil() {}\n```";
        let cleaned = clean_html(input);

        // The placeholder should NOT be replaced (UUID makes it unique)
        assert!(cleaned.contains("__CODE_BLOCK_"));
        assert!(cleaned.contains("fn evil()"));

        // Original placeholder from user should still be there
        // (it won't match the UUID-based placeholder)
        assert!(cleaned.contains("User says: __CODE_BLOCK_0__"));
    }

    #[test]
    fn test_tag_with_attributes() {
        // Test that tags with attributes are properly handled
        let input = r#"<a href="http://example.com?a=1&b=2">link</a>"#;
        let cleaned = clean_html(input);
        // The tag should be preserved (it's whitelisted)
        assert!(cleaned.contains("<a"));
        assert!(cleaned.contains("</a>"));
    }

    #[test]
    fn test_split_very_long_line() {
        // Test edge case: single line exceeding max_length without newlines
        let input = "a".repeat(10000);
        let parts = split_long_message(&input, 4096);

        // Should split into multiple parts
        assert!(parts.len() >= 3);

        // Each part should be within max_length
        for part in &parts {
            assert!(part.len() <= 4096);
        }

        // All parts concatenated should equal original (minus newlines)
        let concatenated: String = parts.join("");
        assert_eq!(concatenated.len(), input.len());
    }

    #[test]
    fn test_split_unicode_graphemes() {
        // Test that splitting by graphemes works correctly with Unicode
        let input = "🔥".repeat(5000); // Each emoji is ~4 bytes
        let parts = split_long_message(&input, 4096);

        // Should split without breaking emoji clusters
        assert!(parts.len() >= 3);

        for part in &parts {
            assert!(part.len() <= 4096);
            // Each part should still contain valid emojis (not broken)
            assert!(part.chars().all(|c| c != '\u{FFFD}')); // No replacement chars
        }
    }
}
