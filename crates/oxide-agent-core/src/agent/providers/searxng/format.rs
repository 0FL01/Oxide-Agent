use super::types::{SearxngResult, SearxngSearchResponse};
use std::fmt::Write;

const MAX_OUTPUT_CHARS: usize = 20_000;

#[must_use]
pub fn format_search_results(
    query: &str,
    response: &SearxngSearchResponse,
    max_results: usize,
) -> String {
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
        output.push_str("Search returned no results for this query.\n");
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

    truncate_output(output)
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
        };

        let formatted = format_search_results("rust", &response, 5);
        assert!(formatted.contains("Search returned no results"));
    }
}
