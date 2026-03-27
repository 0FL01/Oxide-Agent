use super::error::SearxngError;
use super::types::{SearxngSearchArgs, SearxngSearchResponse};
use reqwest::header::ACCEPT;
use std::time::Duration;

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

    pub async fn search(
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
    use super::join_csv;

    #[test]
    fn joins_csv_without_empty_values() {
        let values = vec![" general ".to_string(), "".to_string(), "news".to_string()];
        assert_eq!(join_csv(Some(&values)), Some("general,news".to_string()));
    }
}
