use super::types::{
    DuckDuckGoNewsResult, DuckDuckGoResultKind, DuckDuckGoSearchResult, DuckDuckGoStructuredPayload,
};
use serde_json::{Value, json};
use std::fmt::Write;

const MAX_OUTPUT_CHARS: usize = 20_000;

#[must_use]
pub fn format_search_results(
    query: &str,
    region: &str,
    results: &[DuckDuckGoSearchResult],
    max_results: usize,
) -> (String, Value) {
    let results = results
        .iter()
        .take(max_results)
        .cloned()
        .collect::<Vec<_>>();
    let mut output = format!("## DuckDuckGo search results for: {}\n\n", query.trim());

    if results.is_empty() {
        output.push_str("Search returned no results for this query.\n");
    } else {
        output.push_str("### Results\n\n");
        for (index, result) in results.iter().enumerate() {
            append_search_result(&mut output, index + 1, result);
        }
    }

    let payload = DuckDuckGoStructuredPayload {
        provider: "duckduckgo",
        kind: DuckDuckGoResultKind::Search,
        query: query.trim().to_string(),
        region: region.to_string(),
        results,
    };

    (
        truncate_output(output),
        serde_json::to_value(payload).unwrap_or_else(|_| json!({"provider": "duckduckgo"})),
    )
}

#[must_use]
pub fn format_news_results(
    query: &str,
    region: &str,
    results: &[DuckDuckGoNewsResult],
    max_results: usize,
) -> (String, Value) {
    let results = results
        .iter()
        .take(max_results)
        .cloned()
        .collect::<Vec<_>>();
    let mut output = format!("## DuckDuckGo news results for: {}\n\n", query.trim());

    if results.is_empty() {
        output.push_str("News search returned no results for this query.\n");
    } else {
        output.push_str("### Results\n\n");
        for (index, result) in results.iter().enumerate() {
            append_news_result(&mut output, index + 1, result);
        }
    }

    let payload = DuckDuckGoStructuredPayload {
        provider: "duckduckgo",
        kind: DuckDuckGoResultKind::News,
        query: query.trim().to_string(),
        region: region.to_string(),
        results,
    };

    (
        truncate_output(output),
        serde_json::to_value(payload).unwrap_or_else(|_| json!({"provider": "duckduckgo"})),
    )
}

fn append_search_result(output: &mut String, index: usize, result: &DuckDuckGoSearchResult) {
    let title = crate::utils::clean_html(&result.title);
    let snippet = crate::utils::clean_html(&result.snippet);

    let _ = writeln!(output, "{}. **{}**", index, title);
    let _ = writeln!(output, "URL: {}", result.url);
    if !snippet.trim().is_empty() {
        let _ = writeln!(output, "Snippet: {}", snippet);
    }
    output.push_str("\n---\n\n");
}

fn append_news_result(output: &mut String, index: usize, result: &DuckDuckGoNewsResult) {
    let title = crate::utils::clean_html(&result.title);
    let source = crate::utils::clean_html(&result.source);
    let snippet = crate::utils::clean_html(&result.snippet);

    let _ = writeln!(output, "{}. **{}**", index, title);
    if !source.trim().is_empty() {
        let _ = writeln!(output, "Source: {}", source);
    }
    if !result.date.trim().is_empty() {
        let _ = writeln!(output, "Date: {}", result.date);
    }
    let _ = writeln!(output, "URL: {}", result.url);
    if !snippet.trim().is_empty() {
        let _ = writeln!(output, "Summary: {}", snippet);
    }
    output.push_str("\n---\n\n");
}

fn truncate_output(mut output: String) -> String {
    if output.chars().count() <= MAX_OUTPUT_CHARS {
        return output;
    }

    let truncated = output.chars().take(MAX_OUTPUT_CHARS).collect::<String>();
    output.clear();
    output.push_str(&truncated);
    output.push_str("\n\n[truncated]\n");
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_search_results_and_structured_payload() {
        let (markdown, payload) = format_search_results(
            "rust async",
            "wt-wt",
            &[DuckDuckGoSearchResult {
                title: "Tokio".to_string(),
                url: "https://tokio.rs/".to_string(),
                snippet: "Async runtime".to_string(),
            }],
            5,
        );

        assert!(markdown.contains("DuckDuckGo search results"));
        assert_eq!(payload["provider"], "duckduckgo");
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["results"][0]["url"], "https://tokio.rs/");
    }

    #[test]
    fn formats_news_results_and_structured_payload() {
        let (markdown, payload) = format_news_results(
            "rust",
            "wt-wt",
            &[DuckDuckGoNewsResult {
                date: "2026-05-29T00:00:00Z".to_string(),
                title: "Rust news".to_string(),
                source: "Example".to_string(),
                url: "https://example.com/rust".to_string(),
                snippet: "Summary".to_string(),
                image: Some("https://example.com/image.jpg".to_string()),
            }],
            5,
        );

        assert!(markdown.contains("DuckDuckGo news results"));
        assert_eq!(payload["kind"], "news");
        assert_eq!(payload["results"][0]["source"], "Example");
    }
}
