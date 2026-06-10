use super::types::{SearxngResult, SearxngSearchResponse, TOOL_NAME};
use chrono::Utc;
use serde_json::{Value, json};
use std::fmt::Write;

const MAX_OUTPUT_CHARS: usize = 20_000;

#[must_use]
pub fn format_search_results(
    query: &str,
    response: &SearxngSearchResponse,
    max_results: usize,
) -> (String, Value) {
    let mut output = format!("## SearXNG results for: {}\n\n", query.trim());

    if let Some(total) = response.number_of_results {
        let _ = writeln!(output, "Approximate result count: {:.0}\n", total);
    }

    if !response.answers.is_empty() {
        output.push_str("### Direct answers\n");
        for answer in response.answers.iter().take(3) {
            let _ = writeln!(output, "- {}", crate::utils::clean_html(answer));
        }
        output.push('\n');
    }

    let results = response
        .results
        .iter()
        .take(max_results)
        .collect::<Vec<_>>();
    if results.is_empty() {
        if response.unresponsive_engines.is_empty() {
            output.push_str("Search returned no results for this query.\n");
        } else {
            output.push_str("Partial results: some engines were unavailable for this query.\n");
        }
    } else {
        output.push_str("### Results\n\n");
        for (index, result) in results.iter().enumerate() {
            append_result(&mut output, index + 1, result);
        }
    }

    if !response.suggestions.is_empty() {
        let _ = writeln!(
            output,
            "### Suggestions\n- {}\n",
            response.suggestions.join("\n- ")
        );
    }

    if !response.corrections.is_empty() {
        let _ = writeln!(
            output,
            "### Corrections\n- {}\n",
            response.corrections.join("\n- ")
        );
    }

    let payload_results = results
        .iter()
        .enumerate()
        .map(|(index, result)| result_payload(index + 1, result))
        .collect::<Vec<_>>();

    (
        truncate_output(output),
        json!({
            "provider": TOOL_NAME,
            "kind": "search",
            "query": query.trim(),
            "results": payload_results,
            "answers": response.answers,
            "suggestions": response.suggestions,
            "corrections": response.corrections,
            "number_of_results": response.number_of_results,
            "unresponsive_engines": response.unresponsive_engines,
            "fetched_at": Utc::now().to_rfc3339(),
            "snippet_only": true,
        }),
    )
}

fn append_result(output: &mut String, index: usize, result: &SearxngResult) {
    let title = crate::utils::clean_html(&result.title);
    let snippet = crate::utils::clean_html(&result.content);

    let _ = writeln!(output, "{}. {}", index, title);
    let _ = writeln!(output, "URL: {}", result.url);
    if let Some(engine) = result
        .engine
        .as_deref()
        .filter(|engine| !engine.trim().is_empty())
    {
        let _ = writeln!(output, "Engine: {}", crate::utils::clean_html(engine));
    }
    if !snippet.trim().is_empty() {
        let _ = writeln!(output, "{}", snippet);
    }
    output.push_str("\n---\n\n");
}

fn result_payload(rank: usize, result: &SearxngResult) -> Value {
    let title = crate::utils::clean_html(&result.title);
    let snippet = crate::utils::clean_html(&result.content);

    json!({
        "rank": rank,
        "title": title,
        "url": result.url,
        "snippet": snippet,
        "content": snippet,
        "engine": result.engine.as_deref(),
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
    use super::*;

    #[test]
    fn formats_empty_results() {
        let response = SearxngSearchResponse {
            results: Vec::new(),
            answers: Vec::new(),
            suggestions: Vec::new(),
            corrections: Vec::new(),
            number_of_results: Some(0.0),
            unresponsive_engines: Vec::new(),
        };

        let (formatted, payload) = format_search_results("rust", &response, 5);
        assert!(formatted.contains("Search returned no results"));
        assert_eq!(payload["provider"], TOOL_NAME);
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["query"], "rust");
        assert_eq!(payload["results"], json!([]));
        assert!(payload["fetched_at"].is_string());
    }

    #[test]
    fn formats_partial_results_when_engines_unavailable() {
        let response = SearxngSearchResponse {
            results: Vec::new(),
            answers: Vec::new(),
            suggestions: Vec::new(),
            corrections: Vec::new(),
            number_of_results: Some(0.0),
            unresponsive_engines: vec!["google".to_string(), "bing".to_string()],
        };

        let (formatted, payload) = format_search_results("rust", &response, 5);
        assert!(formatted.contains("Partial results: some engines were unavailable"));
        assert_eq!(payload["unresponsive_engines"], json!(["google", "bing"]));
    }

    #[test]
    fn formats_results_with_ranked_structured_payload() {
        let response = SearxngSearchResponse {
            results: vec![SearxngResult {
                title: "Rust".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                content: "Systems language".to_string(),
                engine: Some("duckduckgo".to_string()),
            }],
            answers: Vec::new(),
            suggestions: Vec::new(),
            corrections: Vec::new(),
            number_of_results: Some(1.0),
            unresponsive_engines: Vec::new(),
        };

        let (markdown, payload) = format_search_results(" rust ", &response, 5);

        assert!(markdown.contains("SearXNG results"));
        assert_eq!(payload["provider"], TOOL_NAME);
        assert_eq!(payload["kind"], "search");
        assert_eq!(payload["query"], "rust");
        assert_eq!(payload["snippet_only"], true);
        assert_eq!(payload["results"][0]["rank"], 1);
        assert_eq!(payload["results"][0]["title"], "Rust");
        assert_eq!(payload["results"][0]["url"], "https://www.rust-lang.org/");
        assert_eq!(payload["results"][0]["snippet"], "Systems language");
    }
}
