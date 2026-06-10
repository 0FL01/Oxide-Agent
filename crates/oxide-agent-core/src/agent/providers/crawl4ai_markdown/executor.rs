//! Typed tool executor: argument parsing, runtime error mapping, ToolExecutor dispatch.

use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;

use crate::agent::tool_runtime::{
    OutputNormalizer, ToolExecutor, ToolInvocation, ToolName, ToolOutput, ToolRuntimeConfig,
    ToolRuntimeError,
};
use crate::llm::ToolDefinition;

use super::constants::TOOL_CRAWL4AI_MARKDOWN;
use super::errors::{crawl4ai_failure_message, crawl4ai_failure_payload};
use super::types::Crawl4AiMarkdownArgs;

use super::Crawl4AiMarkdownProvider;

pub(super) struct Crawl4AiMarkdownToolExecutor {
    pub provider: Arc<Crawl4AiMarkdownProvider>,
    pub name: ToolName,
    pub spec: ToolDefinition,
}

#[async_trait]
impl ToolExecutor for Crawl4AiMarkdownToolExecutor {
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

        if self.name.as_str() != TOOL_CRAWL4AI_MARKDOWN {
            return Err(ToolRuntimeError::Failure(format!(
                "unknown crawl4ai_markdown tool: {}",
                self.name.as_str()
            )));
        }

        let args = parse_crawl4ai_markdown_args(&invocation.raw_arguments)
            .map_err(crawl4ai_runtime_error)?;

        match self
            .provider
            .crawl_markdown(args.clone(), Some(&invocation.cancellation_token))
            .await
        {
            Ok((markdown, payload)) => {
                let mut output = normalizer.success(&invocation, &markdown, "");
                output.structured_payload = Some(payload);
                Ok(output)
            }
            Err(error) => {
                let mut output = normalizer.failure(
                    &invocation,
                    crawl4ai_failure_message(Some(&args), Some(&self.provider.config), &error),
                );
                output.structured_payload = Some(crawl4ai_failure_payload(
                    Some(&args),
                    &self.provider.config,
                    &error,
                ));
                Ok(output)
            }
        }
    }
}

fn parse_crawl4ai_markdown_args(arguments: &str) -> anyhow::Result<Crawl4AiMarkdownArgs> {
    serde_json::from_str(arguments).context("invalid crawl4ai_markdown arguments")
}

fn crawl4ai_runtime_error(error: anyhow::Error) -> ToolRuntimeError {
    let message = error.to_string();
    if message.contains("invalid crawl4ai_markdown arguments") {
        ToolRuntimeError::InvalidArguments(message)
    } else {
        ToolRuntimeError::Failure(message)
    }
}
