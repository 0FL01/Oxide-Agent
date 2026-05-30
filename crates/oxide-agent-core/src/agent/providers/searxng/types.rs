use serde::{Deserialize, Deserializer, Serialize};

pub const TOOL_NAME: &str = "searxng_search";
pub const DEFAULT_MAX_RESULTS: u8 = 5;
pub const MAX_RESULTS_LIMIT: u8 = 10;
pub const DEFAULT_PAGE: u16 = 1;

#[derive(Debug, Deserialize, Clone)]
pub struct SearxngSearchArgs {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: u8,
    pub language: Option<String>,
    pub time_range: Option<String>,
    pub safe_search: Option<u8>,
    pub categories: Option<Vec<String>>,
    pub engines: Option<Vec<String>>,
    #[serde(default = "default_page")]
    pub page: u16,
}

impl SearxngSearchArgs {
    #[must_use]
    pub fn normalized_max_results(&self) -> usize {
        usize::from(self.max_results.clamp(1, MAX_RESULTS_LIMIT))
    }

    #[must_use]
    pub fn normalized_page(&self) -> u16 {
        self.page.max(1)
    }

    #[must_use]
    pub fn normalized_safe_search(&self) -> Option<u8> {
        self.safe_search.map(|value| value.min(2))
    }
}

const fn default_max_results() -> u8 {
    DEFAULT_MAX_RESULTS
}

const fn default_page() -> u16 {
    DEFAULT_PAGE
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearxngSearchResponse {
    #[serde(default)]
    pub results: Vec<SearxngResult>,
    #[serde(default)]
    pub answers: Vec<String>,
    #[serde(default)]
    pub suggestions: Vec<String>,
    #[serde(default)]
    pub corrections: Vec<String>,
    pub number_of_results: Option<f64>,
    /// Engines that timed out or failed during search.
    /// Used internally for automatic engine rotation on retry.
    #[serde(
        default,
        rename = "unresponsive_engines",
        deserialize_with = "deserialize_unresponsive_engines"
    )]
    pub unresponsive_engines: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearxngResult {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub engine: Option<String>,
}

fn deserialize_unresponsive_engines<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };

    let mut engines = Vec::new();

    if let serde_json::Value::Array(items) = raw {
        for item in items {
            match item {
                serde_json::Value::String(name) => {
                    let name = name.trim();
                    if !name.is_empty() {
                        engines.push(name.to_string());
                    }
                }
                serde_json::Value::Array(parts) => {
                    if let Some(serde_json::Value::String(name)) = parts.first() {
                        let name = name.trim();
                        if !name.is_empty() {
                            engines.push(name.to_string());
                        }
                    }
                }
                serde_json::Value::Object(map) => {
                    for key in ["engine", "name"] {
                        if let Some(serde_json::Value::String(name)) = map.get(key) {
                            let name = name.trim();
                            if !name.is_empty() {
                                engines.push(name.to_string());
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    engines.sort();
    engines.dedup();
    Ok(engines)
}

#[cfg(test)]
mod tests {
    use super::SearxngSearchResponse;
    use serde_json::json;

    #[test]
    fn parses_unresponsive_engines_as_pairs() {
        let value = json!({
            "results": [],
            "answers": [],
            "suggestions": [],
            "corrections": [],
            "number_of_results": 0,
            "unresponsive_engines": [["brave", "TooManyRequests"], ["qwant", "timeout"]]
        });

        let parsed: SearxngSearchResponse =
            serde_json::from_value(value).expect("response should deserialize");

        assert_eq!(parsed.unresponsive_engines, vec!["brave", "qwant"]);
    }

    #[test]
    fn parses_unresponsive_engines_as_strings() {
        let value = json!({
            "results": [],
            "answers": [],
            "suggestions": [],
            "corrections": [],
            "number_of_results": 0,
            "unresponsive_engines": ["brave", "bing"]
        });

        let parsed: SearxngSearchResponse =
            serde_json::from_value(value).expect("response should deserialize");

        assert_eq!(parsed.unresponsive_engines, vec!["bing", "brave"]);
    }
}
