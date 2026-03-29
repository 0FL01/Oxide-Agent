use super::backoff::{self, MAX_RETRIES};
use super::error::SearxngError;
use super::types::{SearxngSearchArgs, SearxngSearchResponse};
use reqwest::header::ACCEPT;
use std::time::Duration;
use tracing::{debug, warn};

/// Maximum number of engine rotation attempts when results are empty due to unresponsive engines.
const MAX_ENGINE_ROTATIONS: usize = 2;

#[derive(Debug, Clone)]
pub struct SearxngClient {
    base_url: String,
    http: reqwest::Client,
}

impl SearxngClient {
    pub fn new(base_url: &str, timeout: Duration) -> Result<Self, SearxngError> {
        let http = reqwest::Client::builder().timeout(timeout).build()?;
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        })
    }

    /// Search with automatic retry on transient errors and engine rotation on empty results.
    ///
    /// Makes up to `MAX_RETRIES + 1` attempts with exponential backoff + jitter for transient errors.
    /// If search returns empty results due to unresponsive engines, automatically retries with
    /// those engines excluded (up to `MAX_ENGINE_ROTATIONS` times).
    pub async fn search(
        &self,
        args: &SearxngSearchArgs,
    ) -> Result<SearxngSearchResponse, SearxngError> {
        let mut excluded_engines: Vec<String> = Vec::new();
        let mut last_response: Option<SearxngSearchResponse> = None;

        for rotation in 0..=MAX_ENGINE_ROTATIONS {
            // Build args with excluded engines for this rotation attempt.
            let rotation_args = if rotation == 0 {
                // First attempt — use as-is.
                args.clone()
            } else {
                // Subsequent attempts — exclude unresponsive engines.
                let mut modified = args.clone();
                modified.engines =
                    build_engine_list_excluding(args.engines.as_deref(), &excluded_engines);
                modified
            };

            match self.search_with_retry(&rotation_args).await {
                Ok(response) => {
                    // Check if we got results or need engine rotation.
                    if !response.results.is_empty() {
                        // Got results — return them (success case).
                        return Ok(response);
                    }

                    // Empty results — check for unresponsive engines to rotate.
                    if response.unresponsive_engines.is_empty() {
                        // Truly empty results (no engines to rotate away from).
                        return Ok(response);
                    }

                    // Log unresponsive engines for debugging.
                    debug!(
                        query = %args.query.trim(),
                        rotation = rotation + 1,
                        max_rotations = MAX_ENGINE_ROTATIONS,
                        unresponsive = ?response.unresponsive_engines,
                        "SearXNG returned empty results with unresponsive engines"
                    );

                    // Check if we've already excluded all failed engines.
                    let new_exclusions: Vec<String> = response
                        .unresponsive_engines
                        .iter()
                        .filter(|e| !excluded_engines.contains(e))
                        .cloned()
                        .collect();

                    if new_exclusions.is_empty() {
                        // No new engines to exclude — we're done rotating.
                        // Mark response as having partial engine availability.
                        let mut final_response = response;
                        final_response.unresponsive_engines = excluded_engines;
                        return Ok(final_response);
                    }

                    // Add new unresponsive engines to exclusion list.
                    excluded_engines.extend(new_exclusions);
                    last_response = Some(response);

                    if rotation < MAX_ENGINE_ROTATIONS {
                        warn!(
                            query = %args.query.trim(),
                            excluded = ?excluded_engines,
                            "Retrying search without unresponsive engines"
                        );
                    }
                }
                Err(error) => return Err(error),
            }
        }

        // All rotations exhausted — return last response with partial results note.
        if let Some(mut response) = last_response {
            response.unresponsive_engines = excluded_engines;
            Ok(response)
        } else {
            // Should not happen, but handle gracefully.
            Err(SearxngError::EmptyQuery)
        }
    }

    /// Search with retry logic for transient errors.
    async fn search_with_retry(
        &self,
        args: &SearxngSearchArgs,
    ) -> Result<SearxngSearchResponse, SearxngError> {
        let mut last_error = None;

        for attempt in 0..=MAX_RETRIES {
            match self.search_once(args).await {
                Ok(response) => return Ok(response),
                Err(error) if error.is_retryable() && attempt < MAX_RETRIES => {
                    let delay = backoff::retry_delay(attempt + 1);
                    warn!(
                        query = %args.query.trim(),
                        attempt = attempt + 1,
                        max_retries = MAX_RETRIES,
                        error = %error,
                        retry_after_ms = delay.as_millis() as u64,
                        "SearXNG transient error, retrying"
                    );
                    tokio::time::sleep(delay).await;
                    last_error = Some(error);
                }
                Err(error) => return Err(error),
            }
        }

        Err(last_error.expect("loop ran at least once"))
    }

    /// Single HTTP request without retry logic.
    async fn search_once(
        &self,
        args: &SearxngSearchArgs,
    ) -> Result<SearxngSearchResponse, SearxngError> {
        let query = args.query.trim();
        if query.is_empty() {
            return Err(SearxngError::EmptyQuery);
        }

        let endpoint = format!("{}/search", self.base_url);
        let mut params = vec![
            ("q", query.to_string()),
            ("format", "json".to_string()),
            ("pageno", args.normalized_page().to_string()),
        ];

        if let Some(language) = args
            .language
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            params.push(("language", language.trim().to_string()));
        }

        if let Some(time_range) = args
            .time_range
            .as_deref()
            .filter(|value| !value.trim().is_empty())
        {
            params.push(("time_range", time_range.trim().to_string()));
        }

        if let Some(safe_search) = args.normalized_safe_search() {
            params.push(("safe_search", safe_search.to_string()));
        }

        if let Some(categories) = join_csv(args.categories.as_deref()) {
            params.push(("categories", categories));
        }

        if let Some(engines) = join_csv(args.engines.as_deref()) {
            params.push(("engines", engines));
        }

        let response = self
            .http
            .get(endpoint)
            .header(ACCEPT, "application/json")
            .query(&params)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<failed to read response body>".to_string());
            return Err(SearxngError::HttpStatus {
                status,
                body: truncate_for_error(body),
            });
        }

        Ok(response.json::<SearxngSearchResponse>().await?)
    }
}

/// Build engine list excluding specific engines.
///
/// Logic:
/// - If user explicitly specified engines: return those engines MINUS excluded ones
/// - If user didn't specify engines (use all): return None (let SearXNG decide)
///
/// Note: When engines param is None/empty, SearXNG uses all configured engines.
/// We cannot easily exclude engines without knowing the full list, so we retry
/// hoping SearXNG will route to responsive ones.
fn build_engine_list_excluding(
    user_engines: Option<&[String]>,
    excluded: &[String],
) -> Option<Vec<String>> {
    match user_engines {
        None => {
            // User didn't specify engines — let SearXNG use all (exclude none explicitly).
            // The engine rotation happens by retrying and hoping SearXNG routes better.
            None
        }
        Some(engines) => {
            // User specified engines — filter out excluded ones.
            let filtered: Vec<String> = engines
                .iter()
                .filter(|e| !excluded.contains(e))
                .cloned()
                .collect();

            if filtered.is_empty() {
                // All user-specified engines are excluded — return None to use defaults.
                None
            } else {
                Some(filtered)
            }
        }
    }
}

fn join_csv(values: Option<&[String]>) -> Option<String> {
    let values = values?
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();

    if values.is_empty() {
        None
    } else {
        Some(values.join(","))
    }
}

fn truncate_for_error(body: String) -> String {
    const LIMIT: usize = 500;
    if body.chars().count() <= LIMIT {
        return body;
    }

    let mut truncated = body.chars().take(LIMIT).collect::<String>();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::{build_engine_list_excluding, join_csv};

    #[test]
    fn joins_csv_without_empty_values() {
        let values = vec![" general ".to_string(), "".to_string(), "news".to_string()];
        assert_eq!(join_csv(Some(&values)), Some("general,news".to_string()));
    }

    #[test]
    fn build_engine_list_preserves_user_selection_minus_excluded() {
        let user = vec![
            "google".to_string(),
            "bing".to_string(),
            "duckduckgo".to_string(),
        ];
        let excluded = vec!["bing".to_string()];

        let result = build_engine_list_excluding(Some(&user), &excluded);

        assert_eq!(
            result,
            Some(vec!["google".to_string(), "duckduckgo".to_string()])
        );
    }

    #[test]
    fn build_engine_list_returns_none_when_all_user_engines_excluded() {
        let user = vec!["google".to_string(), "bing".to_string()];
        let excluded = vec!["google".to_string(), "bing".to_string()];

        let result = build_engine_list_excluding(Some(&user), &excluded);

        assert_eq!(result, None);
    }

    #[test]
    fn build_engine_list_returns_none_when_no_user_engines() {
        let excluded = vec!["google".to_string()];

        let result = build_engine_list_excluding(None, &excluded);

        assert_eq!(result, None);
    }
}
