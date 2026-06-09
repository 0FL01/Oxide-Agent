//! Lightweight web page fetcher.
//!
//! Provides `web_markdown`: one HTTP GET for a known URL plus optional HTML to Markdown
//! conversion. It is intentionally not a crawler, browser, or PDF exporter.

mod convert;
mod error;
mod fetch;
mod reddit;
mod url;

use error::{webfetch_failure_message, webfetch_failure_payload};

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

const TOOL_WEB_MARKDOWN: &str = "web_markdown";
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_TIMEOUT_SECS: u64 = 120;
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;
const MAX_OUTPUT_CHARS: usize = 20_000;
const MAX_REDIRECTS: usize = 5;
const MARKDOWN_ACCEPT_HEADER: &str =
    "text/markdown;q=1.0, text/x-markdown;q=0.9, text/plain;q=0.8, text/html;q=0.7, */*;q=0.1";
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";
const ANTI_BOT_ERROR: &str = "web_markdown blocked by anti-bot protection; this lightweight fetcher cannot solve JS/CAPTCHA challenges";

/// Local provider for fetching a single URL as Markdown.
pub struct WebFetchMdProvider {
    client: reqwest::Client,
}

#[derive(Debug, Deserialize, Clone)]
struct WebMarkdownArgs {
    url: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl WebFetchMdProvider {
    /// Create a new local web markdown provider.
    ///
    #[must_use]
    pub fn new() -> Self {
        let client = match reqwest::Client::builder()
            .timeout(Duration::from_secs(MAX_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    return attempt.stop();
                }

                match url::reject_unsafe_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(error) => attempt.error(format!("unsafe redirect target: {error}")),
                }
            }))
            .build()
        {
            Ok(client) => client,
            Err(_) => reqwest::Client::new(),
        };

        Self { client }
    }

    #[cfg(test)]
    fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    /// Build native typed runtime executors for the web markdown tool.
    #[must_use]
    pub fn tool_runtime_executors(self: &Arc<Self>) -> Vec<Arc<dyn ToolExecutor>> {
        let spec = Self::tool_definition();
        vec![Arc::new(WebFetchMdToolExecutor {
            provider: Arc::clone(self),
            name: ToolName::from(spec.name.clone()),
            spec,
        })]
    }

    fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            name: TOOL_WEB_MARKDOWN.to_string(),
            description: "Fetch one known http/https URL and return Markdown. This tool does not crawl, execute JavaScript, search the web, or export PDFs.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Fully-qualified http/https URL to fetch"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Optional request timeout in seconds, clamped to 1..120"
                    }
                },
                "required": ["url"]
            }),
        }
    }
}

impl Default for WebFetchMdProvider {
    fn default() -> Self {
        Self::new()
    }
}

struct WebFetchMdToolExecutor {
    provider: Arc<WebFetchMdProvider>,
    name: ToolName,
    spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for WebFetchMdToolExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        self.spec.clone()
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
    ) -> std::result::Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });

        if self.name.as_str() != TOOL_WEB_MARKDOWN {
            return Err(ToolRuntimeError::Failure(format!(
                "unknown webfetch_md tool: {}",
                self.name.as_str()
            )));
        }

        let args =
            parse_web_markdown_args(&invocation.raw_arguments).map_err(webfetch_runtime_error)?;

        match self
            .provider
            .fetch_markdown(args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok(output) => Ok(normalizer.success(&invocation, &output, "")),
            Err(error) => {
                let mut output =
                    normalizer.failure(&invocation, webfetch_failure_message(Some(&args), &error));
                output.structured_payload = Some(webfetch_failure_payload(Some(&args), &error));
                Ok(output)
            }
        }
    }
}

fn parse_web_markdown_args(arguments: &str) -> Result<WebMarkdownArgs> {
    serde_json::from_str(arguments).context("invalid web_markdown arguments")
}

fn webfetch_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if message.contains("invalid web_markdown arguments") {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}

#[cfg(test)]
mod tests;
