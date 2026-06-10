use super::error::BraveSearchError;
use super::types::{BraveSearchResponse, BraveWebResult, NormalizedBraveSearchArgs, TOOL_NAME};
use chrono::Utc;
use serde_json::{Value, json};
use std::fmt::Write;

const MAX_OUTPUT_CHARS: usize = 20_000;
const FALLBACK_TOOL: &str = "searxng_search";

#[must_use]
pub fn format_search_results(
    args: &NormalizedBraveSearchArgs,
    response: &BraveSearchResponse,
) -> (String, Value) {
    let results = response
        .web
        .as_ref()
        .map(|web| web.results.as_slice())
        .unwrap_or(&[])
        .iter()
        .take(usize::from(args.max_results))
        .cloned()
        .collect::<Vec<_>>();

    let mut output = format!("## Brave Search results for: {}\n\n", args.query.trim());
    if results.is_empty() {
        output.push_str("Search returned no results for this query.\n");
    } else {
        for (index, result) in results.iter().enumerate() {
            append_result(&mut output, index + 1, result);
        }
    }

    let payload_results = results.iter().map(result_payload).collect::<Vec<Value>>();

    (
        truncate_output(output),
        json!({
            "provider": TOOL_NAME,
            "kind": "search",
            "query": args.query.trim(),
            "country": args.country.as_deref(),
            "search_lang": args.search_lang.as_deref(),
            "freshness": args.freshness.as_deref(),
            "results": payload_results,
            "snippet_only": true,
            "fetched_at": Utc::now().to_rfc3339(),
        }),
    )
}

#[must_use]
pub fn format_search_failure(query: &str, error: &BraveSearchError) -> (String, Value) {
    let query = query.trim();
    let markdown = format!(
        "Brave Search failed: {error}\n\nDo not retry brave_search in this task; use searxng_search as fallback if search is still required.\n"
    );

    (
        markdown,
        json!({
            "provider": TOOL_NAME,
            "kind": "search",
            "query": query,
            "error_kind": error.code(),
            "error": error.to_string(),
            "provider_unavailable": error.provider_unavailable(),
            "retryable": error.is_retryable(),
            "fallback": FALLBACK_TOOL,
            "results": [],
            "snippet_only": true,
            "fetched_at": Utc::now().to_rfc3339(),
        }),
    )
}

fn append_result(output: &mut String, index: usize, result: &BraveWebResult) {
    let title = crate::utils::clean_html(&result.title);
    let description = crate::utils::clean_html(&result.description);

    let _ = writeln!(output, "{index}. **{title}**");
    let _ = writeln!(output, "   URL: {}", result.url);
    if !description.trim().is_empty() {
        let _ = writeln!(output, "   Snippet: {description}");
    }
    if let Some(age) = result.age.as_deref().filter(|age| !age.trim().is_empty()) {
        let _ = writeln!(output, "   Age: {age}");
    }
    for snippet in &result.extra_snippets {
        let snippet = crate::utils::clean_html(snippet);
        if !snippet.trim().is_empty() {
            let _ = writeln!(output, "   Extra snippet: {snippet}");
        }
    }
    output.push('\n');
}

fn result_payload(result: &BraveWebResult) -> Value {
    json!({
        "title": result.title.as_str(),
        "url": result.url.as_str(),
        "description": result.description.as_str(),
        "age": result.age.as_deref(),
        "language": result.language.as_deref(),
        "extra_snippets": &result.extra_snippets,
    })
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
    use super::super::types::BraveWebResults;
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn formats_success_markdown_and_structured_payload() {
        let args = normalized_args();
        let response = BraveSearchResponse {
            web: Some(BraveWebResults {
                results: vec![
                    BraveWebResult {
                        title: "Rust".to_string(),
                        url: "https://www.rust-lang.org/".to_string(),
                        description: "A language empowering everyone".to_string(),
                        age: Some("2 days ago".to_string()),
                        language: Some("en".to_string()),
                        family_friendly: Some(true),
                        extra_snippets: vec!["Memory safety".to_string()],
                    },
                    BraveWebResult {
                        title: "Ignored by max_results".to_string(),
                        url: "https://example.com/ignored".to_string(),
                        description: String::new(),
                        age: None,
                        language: None,
                        family_friendly: None,
                        extra_snippets: Vec::new(),
                    },
                ],
            }),
            query: None,
        };

        let (markdown, payload) = format_search_results(&args, &response);

        assert!(markdown.contains("## Brave Search results for: rust async"));
        assert!(markdown.contains("1. **Rust**"));
        assert!(markdown.contains("   URL: https://www.rust-lang.org/"));
        assert!(markdown.contains("   Snippet: A language empowering everyone"));
        assert!(markdown.contains("   Age: 2 days ago"));
        assert_eq!(payload["provider"], TOOL_NAME);
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["query"], "rust async");
        assert_eq!(payload["country"], "US");
        assert_eq!(payload["search_lang"], "en");
        assert_eq!(payload["freshness"], "pw");
        assert_eq!(payload["results"].as_array().expect("array").len(), 1);
        assert_eq!(payload["results"][0]["url"], "https://www.rust-lang.org/");
        assert_eq!(payload["results"][0]["language"], "en");
        assert_eq!(payload["results"][0]["extra_snippets"][0], "Memory safety");
        assert_eq!(payload["snippet_only"], true);
        assert!(payload["fetched_at"].as_str().is_some());
    }

    #[test]
    fn formats_empty_success_payload() {
        let args = normalized_args();
        let response = BraveSearchResponse {
            web: None,
            query: None,
        };

        let (markdown, payload) = format_search_results(&args, &response);

        assert!(markdown.contains("Search returned no results"));
        assert_eq!(payload["provider"], TOOL_NAME);
        assert!(payload["results"].as_array().expect("array").is_empty());
    }

    #[test]
    fn formats_failure_structured_payload() {
        let error = BraveSearchError::RateLimited;

        let (markdown, payload) = format_search_failure(" rust ", &error);

        assert!(markdown.contains("Brave Search failed"));
        assert_eq!(payload["provider"], TOOL_NAME);
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["query"], "rust");
        assert_eq!(payload["error_kind"], "rate_limited");
        assert_eq!(payload["provider_unavailable"], true);
        assert_eq!(payload["retryable"], false);
        assert_eq!(payload["fallback"], FALLBACK_TOOL);
        assert!(payload["results"].as_array().expect("array").is_empty());
        assert_eq!(payload["snippet_only"], true);
        assert!(payload["fetched_at"].as_str().is_some());
    }

    #[test]
    fn failure_payload_marks_retryable_server_error() {
        let error = BraveSearchError::Server {
            status: StatusCode::BAD_GATEWAY,
            body: "bad gateway".to_string(),
        };

        let (_, payload) = format_search_failure("rust", &error);

        assert_eq!(payload["error_kind"], "server");
        assert_eq!(payload["provider_unavailable"], true);
        assert_eq!(payload["retryable"], true);
    }

    fn normalized_args() -> NormalizedBraveSearchArgs {
        NormalizedBraveSearchArgs {
            query: "rust async".to_string(),
            max_results: 1,
            offset: 0,
            country: Some("US".to_string()),
            search_lang: Some("en".to_string()),
            ui_lang: Some("en-US".to_string()),
            freshness: Some("pw".to_string()),
            safesearch: "moderate".to_string(),
            extra_snippets: true,
        }
    }
}
