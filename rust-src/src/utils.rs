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

use lazy_regex::lazy_regex;

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

/// Replace naked angle brackets with HTML entities, preserving HTML tags.
/// Rust's regex doesn't support lookbehind/lookahead, so we iterate manually.
fn escape_angle_brackets(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut result = String::with_capacity(text.len());
    let mut in_tag = false;

    for (i, &c) in chars.iter().enumerate() {
        match c {
            '<' => {
                // Look ahead to see if this starts an HTML tag like </a or <b
                let starts_tag = if i + 1 < chars.len() {
                    let next1 = chars[i + 1];
                    let next2 = if i + 2 < chars.len() {
                        Some(chars[i + 2])
                    } else {
                        None
                    };
                    match (next1, next2) {
                        ('/', Some(ch)) if ch.is_ascii_alphabetic() => true,
                        (ch, _) if ch.is_ascii_alphabetic() => true,
                        _ => false,
                    }
                } else {
                    false
                };

                if starts_tag {
                    result.push(c);
                    in_tag = true;
                } else {
                    result.push_str("&lt;");
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
    let mut code_blocks = Vec::new();
    let mut text_owned = text.to_string();

    // Replace code blocks with placeholders
    let mut result = String::new();
    let mut last_end = 0;
    for mat in RE_CODE_BLOCK.find_iter(&text_owned) {
        result.push_str(&text_owned[last_end..mat.start()]);
        let placeholder = format!("__CODE_BLOCK_{}__", code_blocks.len());
        code_blocks.push(mat.as_str().to_string());
        result.push_str(&placeholder);
        last_end = mat.end();
    }
    result.push_str(&text_owned[last_end..]);
    text_owned = result;

    // Replace naked < and > using our custom function
    text_owned = escape_angle_brackets(&text_owned);

    // Restore code blocks
    for (i, block) in code_blocks.iter().enumerate() {
        let placeholder = format!("__CODE_BLOCK_{i}__");
        text_owned = text_owned.replace(&placeholder, block);
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
/// This function respects code blocks (```) and tries to close/reopen them
/// across message boundaries to maintain proper formatting in Telegram.
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
        if line.starts_with(code_fence) {
            code_block = !code_block;
        }

        let new_length = current_message.len() + line.len() + 1; // +1 for newline

        if new_length > max_length && !current_message.is_empty() {
            if code_block {
                current_message.push_str(code_fence);
                current_message.push('\n');
                // We don't flip code_block state here because we want the NEXT part to start with code_fence too
            }

            parts.append(&mut vec![current_message.trim_end().to_string()]);
            current_message.clear();

            if code_block {
                // If we WERE in a code block, the new part should start with one
                // Wait, if it *starts* with code_fence, we just toggled it.
                // Let's re-evaluate.
                current_message.push_str(code_fence);
                current_message.push('\n');
                // If the line ITSELF was the fence, we don't want to double it.
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
}
