use anyhow::Result;
use dotenvy::dotenv;
use oxide_agent::llm::providers::ZaiProvider;
use oxide_agent::llm::{LlmProvider, Message, ToolDefinition};
use serde_json::json;
use std::env;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn init_test_env() {
    let _ = dotenv();
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

#[tokio::test]
async fn test_zai_tool_calling_integration() -> Result<()> {
    init_test_env();

    let api_key = match env::var("ZAI_API_KEY") {
        Ok(k) if !k.is_empty() && k != "dummy" => k,
        _ => {
            warn!("Skipping ZAI integration test: valid ZAI_API_KEY not set");
            return Ok(());
        }
    };

    info!("Starting ZAI tool calling integration test...");

    // Hardcoded model for regression testing
    let model_id = "glm-4.5-air";

    // We can't easily force the model to call a tool without a user message asking for it,
    // but the critical part is that the API accepts the `tools` parameter without 400ing.
    // So we'll try to trigger it.

    let provider = ZaiProvider::new(api_key, None);

    // Define a simple tool
    let tools = vec![ToolDefinition {
        name: "get_current_weather".to_string(),
        description: "Get the current weather in a given location".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "location": {
                    "type": "string",
                    "description": "The city and state, e.g. San Francisco, CA"
                },
                "unit": {
                    "type": "string",
                    "enum": ["celsius", "fahrenheit"]
                }
            },
            "required": ["location"]
        }),
    }];

    let system_prompt = "You are a helpful assistant.";

    let messages = vec![Message::user("What's the weather like in Tokyo?")];

    info!("Sending request to ZAI (model: {})...", model_id);
    let result = provider
        .chat_with_tools(system_prompt, &messages, &tools, model_id, 1024, false)
        .await;

    match result {
        Ok(response) => {
            info!("ZAI response received successfully!");
            if !response.tool_calls.is_empty() {
                info!("ZAI decided to call tools: {:?}", response.tool_calls);

                // Verify tool call structure is correct (regression check)
                for tool_call in &response.tool_calls {
                    assert!(!tool_call.id.is_empty(), "Tool call ID must not be empty");
                    assert!(
                        !tool_call.function.name.is_empty(),
                        "Tool call function name must not be empty"
                    );

                    // Parse arguments to ensure they're valid JSON (regression check)
                    let _: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
                        .map_err(|e| anyhow::anyhow!("Invalid JSON in tool arguments: {}", e))?;

                    info!(
                        "Tool call validation passed for: {}",
                        tool_call.function.name
                    );
                }
            } else {
                info!("ZAI responded with text: {:?}", response.content);
            }

            // Verify response structure (regression check)
            assert!(
                !response.finish_reason.is_empty(),
                "Finish reason must not be empty"
            );
        }
        Err(e) => {
            // If we get a 400 Bad Request, that's exactly what we want to fail the test on.
            panic!("ZAI API request failed: {}", e);
        }
    }

    Ok(())
}
