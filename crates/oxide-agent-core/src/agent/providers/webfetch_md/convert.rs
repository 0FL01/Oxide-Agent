use anyhow::{Result, anyhow};

pub(super) fn html_to_markdown(html: &str) -> Result<String> {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(vec![
            "script", "style", "noscript", "iframe", "object", "embed", "meta", "link", "nav",
            "footer", "aside", "form", "button", "svg", "canvas",
        ])
        .build()
        .convert(html)
        .map_err(|error| anyhow!("html to markdown conversion failed: {error}"))
}

#[derive(Debug, Clone, Copy)]
pub(in crate::agent::providers::webfetch_md) struct OutputWindow {
    pub max_chars: usize,
    pub offset_chars: usize,
}

pub(in crate::agent::providers::webfetch_md) struct WindowedOutput {
    pub text: String,
    pub markdown_chars: usize,
    pub returned_chars: usize,
    pub remaining_chars: usize,
    pub next_offset_chars: Option<usize>,
    pub was_truncated: bool,
}

pub(super) fn window_chars(input: String, window: OutputWindow) -> WindowedOutput {
    let markdown_chars = input.chars().count();
    if window.offset_chars >= markdown_chars {
        return WindowedOutput {
            text: String::new(),
            markdown_chars,
            returned_chars: 0,
            remaining_chars: 0,
            next_offset_chars: None,
            was_truncated: false,
        };
    }

    let end = (window.offset_chars + window.max_chars).min(markdown_chars);
    let returned_chars = end - window.offset_chars;
    let remaining_chars = markdown_chars - end;
    let was_truncated = remaining_chars > 0;
    let mut text = input
        .chars()
        .skip(window.offset_chars)
        .take(returned_chars)
        .collect::<String>();
    if was_truncated {
        text.push_str("\n\n... (truncated)");
    }

    WindowedOutput {
        text,
        markdown_chars,
        returned_chars,
        remaining_chars,
        next_offset_chars: was_truncated.then_some(end),
        was_truncated,
    }
}
