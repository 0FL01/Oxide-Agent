use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref RE_CODE_BLOCK: Regex = Regex::new(r"```[\s\S]*?```").unwrap();
    static ref RE_CODE_BLOCK_FENCE: Regex = Regex::new(r"```(\w+)?\n([\s\S]*?)```").unwrap();
    static ref RE_BULLET: Regex = Regex::new(r"(?m)^\* ").unwrap();
    static ref RE_BOLD: Regex = Regex::new(r"\*\*(.*?)\*\*").unwrap();
    static ref RE_ITALIC: Regex = Regex::new(r"\*(.*?)\*").unwrap();
    static ref RE_INLINE_CODE: Regex = Regex::new(r"`(.*?)`").unwrap();
    static ref RE_MULTI_NEWLINE: Regex = Regex::new(r"\n{3,}").unwrap();
}

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
        let placeholder = format!("__CODE_BLOCK_{}__", i);
        text_owned = text_owned.replace(&placeholder, block);
    }

    text_owned
}

pub fn format_text(text: &str) -> String {
    let mut text_owned = clean_html(text);

    // Replace blocks: ```language\ncode``` -> <pre><code class="language">code</code></pre>
    text_owned = RE_CODE_BLOCK_FENCE
        .replace_all(&text_owned, |caps: &regex::Captures| {
            let lang = caps.get(1).map_or("", |m| m.as_str());
            let code = caps.get(2).map_or("", |m| m.as_str()).trim();
            let escaped_code = html_escape::encode_text(code);
            format!(
                "<pre><code class=\"{}\">{}</code></pre>",
                lang, escaped_code
            )
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
            format!("<code>{}</code>", escaped_code)
        })
        .to_string();

    // Replace 3+ newlines with 2
    text_owned = RE_MULTI_NEWLINE
        .replace_all(&text_owned, "\n\n")
        .to_string();

    text_owned.trim().to_string()
}

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
/// This is UTF-8 safe and will not panic on multi-byte characters.
pub fn truncate_str(s: impl AsRef<str>, max_chars: usize) -> String {
    let s = s.as_ref();
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    s.char_indices()
        .nth(max_chars)
        .map_or(s.to_string(), |(pos, _)| s[..pos].to_string())
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
}
