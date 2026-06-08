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

pub(super) struct TruncatedOutput {
    pub text: String,
    pub was_truncated: bool,
}

pub(super) fn truncate_chars(input: String, max_chars: usize) -> TruncatedOutput {
    if input.chars().count() <= max_chars {
        return TruncatedOutput {
            text: input,
            was_truncated: false,
        };
    }

    let mut text = input.chars().take(max_chars).collect::<String>();
    text.push_str("\n\n... (truncated)");
    TruncatedOutput {
        text,
        was_truncated: true,
    }
}
