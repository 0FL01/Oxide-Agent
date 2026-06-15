use super::types::{CrwSearchResponse, CrwSearchResult};
use std::fmt::Write;

const MAX_OUTPUT_CHARS: usize = 20_000;

/// Format CRW search results as markdown for the LLM.
#[must_use]
pub fn format_search_results(query: &str, response: &CrwSearchResponse, max_results: u8) -> String {
    let mut output = format!("## Web search results for: {}\n\n", query.trim());

    let results = response
        .data
        .iter()
        .take(usize::from(max_results))
        .collect::<Vec<_>>();

    if results.is_empty() {
        output.push_str("Search returned no results for this query.\n");
    } else {
        let _ = writeln!(output, "Found {} results.\n", results.len());
        output.push_str("### Results\n\n");
        for (index, result) in results.iter().enumerate() {
            append_result(&mut output, index + 1, result);
        }
    }

    truncate_output(output)
}

fn append_result(output: &mut String, index: usize, result: &CrwSearchResult) {
    let title = crate::utils::clean_html(&result.title);
    let snippet = crate::utils::clean_html(&result.content);

    let _ = writeln!(output, "{}. {}", index, title);
    let _ = writeln!(output, "URL: {}", result.url);
    if !snippet.trim().is_empty() {
        let _ = writeln!(output, "{}", snippet);
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
    fn formats_empty_results() {
        let response = CrwSearchResponse {
            success: true,
            data: Vec::new(),
            error: None,
        };
        let formatted = format_search_results("rust", &response, 5);
        assert!(formatted.contains("Search returned no results"));
    }

    #[test]
    fn formats_multiple_results() {
        let response = CrwSearchResponse {
            success: true,
            data: vec![
                CrwSearchResult {
                    title: "Rust Programming".to_string(),
                    url: "https://rust-lang.org".to_string(),
                    content: "A language empowering everyone.".to_string(),
                },
                CrwSearchResult {
                    title: "Learn Rust".to_string(),
                    url: "https://doc.rust-lang.org".to_string(),
                    content: String::new(),
                },
            ],
            error: None,
        };
        let formatted = format_search_results("rust", &response, 5);
        assert!(formatted.contains("Rust Programming"));
        assert!(formatted.contains("https://rust-lang.org"));
        assert!(formatted.contains("empowering everyone"));
        assert!(formatted.contains("Learn Rust"));
        assert!(formatted.contains("Found 2 results"));
    }

    #[test]
    fn truncates_large_output() {
        let large_content = "x".repeat(30_000);
        let response = CrwSearchResponse {
            success: true,
            data: vec![CrwSearchResult {
                title: "Big".to_string(),
                url: "https://example.com".to_string(),
                content: large_content,
            }],
            error: None,
        };
        let formatted = format_search_results("big", &response, 5);
        assert!(formatted.ends_with("[truncated]\n"));
    }
}
