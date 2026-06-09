//! Reddit Atom RSS fallback: URL detection, feed parsing, and Markdown rendering.

use super::response::html_to_markdown;
use super::types::{CrawlResult, RedditAtomEntry};
use anyhow::{Result, bail};
use reqwest::Url;

/// Check if a URL is a Reddit thread and return the corresponding `.rss` Atom feed URL.
pub(crate) fn reddit_thread_rss_url(url: &Url) -> Option<Url> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if !matches!(
        host.as_str(),
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "new.reddit.com" | "sh.reddit.com"
    ) {
        return None;
    }

    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 4 || segments[0] != "r" || segments[2] != "comments" {
        return None;
    }

    let mut rss_url = url.clone();
    rss_url.set_host(Some("www.reddit.com")).ok()?;
    rss_url.set_query(None);
    rss_url.set_fragment(None);

    let mut path = rss_url.path().trim_end_matches('/').to_string();
    if !path.ends_with(".rss") {
        path.push_str("/.rss");
    }
    rss_url.set_path(&path);
    Some(rss_url)
}

/// Convert a Reddit Atom feed into a `CrawlResult` with rendered Markdown.
pub(crate) fn reddit_atom_to_crawl_result(
    target_url: &Url,
    rss_url: &Url,
    status_code: u16,
    atom: &str,
) -> Result<CrawlResult> {
    let feed_title = xml_tag_text(atom, "title").unwrap_or_else(|| "Reddit thread".to_string());
    let entries = parse_reddit_atom_entries(atom)?;
    if entries.is_empty() {
        bail!("reddit rss parse error: empty Atom entries");
    }

    let markdown = render_reddit_atom_markdown(target_url, &feed_title, &entries);
    let selected_chars = markdown.chars().count();
    Ok(CrawlResult {
        final_url: Some(rss_url.clone()),
        status_code: Some(status_code),
        markdown_kind: "reddit_rss_fallback",
        content_mode: "reddit_rss_fallback",
        source_kind: "reddit_thread",
        markdown,
        raw_chars: atom.chars().count(),
        selected_chars,
        elapsed_ms: None,
        entries_count: Some(entries.len()),
        noise_filtered: true,
    })
}

/// Parse `<entry>` blocks from a Reddit Atom feed into typed structs.
fn parse_reddit_atom_entries(atom: &str) -> Result<Vec<RedditAtomEntry>> {
    let mut entries = Vec::new();
    let mut rest = atom;
    while let Some(start) = rest.find("<entry") {
        let after_start = &rest[start..];
        let Some(open_end) = after_start.find('>') else {
            break;
        };
        let entry_body_start = start + open_end + 1;
        let after_body_start = &rest[entry_body_start..];
        let Some(end) = after_body_start.find("</entry>") else {
            break;
        };
        let block = &after_body_start[..end];
        rest = &after_body_start[end + "</entry>".len()..];

        let title =
            xml_tag_text(block, "title").unwrap_or_else(|| "Untitled Reddit entry".to_string());
        let author = xml_tag_block(block, "author").and_then(|author| xml_tag_text(author, "name"));
        let content_html = xml_tag_text(block, "content").unwrap_or_default();
        let markdown = html_to_markdown(&content_html)
            .unwrap_or_else(|_| content_html.clone())
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        entries.push(RedditAtomEntry {
            title,
            author,
            markdown,
        });
    }
    Ok(entries)
}

/// Render parsed Reddit entries into compact Markdown.
fn render_reddit_atom_markdown(
    target_url: &Url,
    feed_title: &str,
    entries: &[RedditAtomEntry],
) -> String {
    let mut output = String::new();
    output.push_str("# ");
    output.push_str(feed_title.trim());
    output.push_str("\n\n");
    output.push_str("Source: ");
    output.push_str(target_url.as_str());
    output.push_str("\nMode: reddit_rss_fallback\nEntries: ");
    output.push_str(&entries.len().to_string());
    output.push_str("\n\n");

    for (index, entry) in entries.iter().enumerate() {
        if index == 0 {
            output.push_str("## Original post\n\n");
        } else if index == 1 {
            output.push_str("## Comments\n\n");
        }

        if index > 0 {
            output.push_str("### ");
            output.push_str(&index.to_string());
            output.push_str(". ");
        } else {
            output.push_str("**");
        }
        output.push_str(entry.title.trim());
        if index == 0 {
            output.push_str("**");
        }
        output.push_str("\n\n");

        if let Some(author) = entry
            .author
            .as_deref()
            .filter(|author| !author.trim().is_empty())
        {
            output.push_str("Author: ");
            output.push_str(author.trim());
            output.push_str("\n\n");
        }
        if !entry.markdown.trim().is_empty() {
            output.push_str(entry.markdown.trim());
            output.push_str("\n\n");
        }
    }

    output.trim().to_string()
}

// -- Minimal XML helpers (no dependency on full XML parser) --

/// Extract the decoded text content of the first `<tag>...</tag>` occurrence.
pub(crate) fn xml_tag_text(input: &str, tag: &str) -> Option<String> {
    xml_tag_block(input, tag).map(|text| html_escape::decode_html_entities(text).trim().to_string())
}

/// Extract the raw inner content of the first `<tag>...</tag>` occurrence.
pub(crate) fn xml_tag_block<'a>(input: &'a str, tag: &str) -> Option<&'a str> {
    let start_marker = format!("<{tag}");
    let start = input.find(&start_marker)?;
    let after_start = &input[start..];
    let open_end = after_start.find('>')?;
    let body_start = start + open_end + 1;
    let end_marker = format!("</{tag}>");
    let end = input[body_start..].find(&end_marker)?;
    Some(&input[body_start..body_start + end])
}
