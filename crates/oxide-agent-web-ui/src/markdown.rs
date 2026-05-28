use ammonia::{Builder, UrlRelative};
use comrak::{markdown_to_html, Options};
use std::collections::HashSet;

#[cfg(target_arch = "wasm32")]
use leptos::prelude::*;

#[must_use]
pub fn render_markdown(markdown: &str) -> String {
    let html = markdown_to_html(markdown, &markdown_options());
    sanitize_html(&html)
}

#[must_use]
pub fn sanitize_html(html: &str) -> String {
    let allowed_url_schemes = HashSet::from(["http", "https", "mailto"]);
    Builder::default()
        .url_schemes(allowed_url_schemes)
        .url_relative(UrlRelative::PassThrough)
        .rm_tags(&["img"])
        .clean(html)
        .to_string()
}

fn markdown_options() -> Options<'static> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.tasklist = true;
    options.extension.autolink = true;
    options.extension.footnotes = false;
    options.render.r#unsafe = true;
    options
}

#[cfg(target_arch = "wasm32")]
#[component]
pub fn MarkdownContent(markdown: String) -> impl IntoView {
    let html = render_markdown(&markdown);
    view! { <div class="markdown-content" inner_html=html></div> }
}

#[cfg(test)]
mod tests {
    use super::{render_markdown, sanitize_html};

    #[test]
    fn removes_script_content_and_event_attributes() {
        let html = sanitize_html("<p onclick=\"alert(1)\">ok</p><script>alert(1)</script>");

        assert!(html.contains("<p>ok</p>"));
        assert!(!html.contains("onclick"));
        assert!(!html.contains("script"));
        assert!(!html.contains("alert"));
    }

    #[test]
    fn removes_unsafe_link_protocols() {
        let html = render_markdown("[bad](javascript:alert(1)) [ok](https://example.com)");

        assert!(!html.contains("javascript:"));
        assert!(html.contains("https://example.com"));
    }

    #[test]
    fn supports_tables_and_code_blocks() {
        let html = render_markdown(
            "- [x] done\n\n| name | value |\n| --- | --- |\n| a | `b` |\n\n```rust\nfn main() {}\n```",
        );

        assert!(html.contains("<table>"));
        assert!(html.contains("<code>"));
        assert!(html.contains("fn main"));
    }

    #[test]
    fn drops_images_from_markdown_output() {
        let html = render_markdown("![x](https://example.com/image.png)");

        assert!(!html.contains("<img"));
    }
}
