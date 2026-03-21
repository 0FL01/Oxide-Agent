use anyhow::Result;
use dotenvy::dotenv;
use oxide_agent_core::llm::providers::MistralProvider;
use oxide_agent_core::llm::{ChatWithToolsRequest, LlmProvider, Message, ToolDefinition};
use serde_json::json;
use std::env;
use tracing::{info, warn};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

fn init_test_env() {
    let _ = dotenv();
    let _ = tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::DEBUG.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

fn should_run_e2e_checks() -> bool {
    matches!(env::var("RUN_LLM_E2E_CHECKS").as_deref(), Ok("1"))
}

fn weather_tool() -> ToolDefinition {
    ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get the current weather for a city".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "city": {"type": "string", "description": "The city name"}
            },
            "required": ["city"]
        }),
    }
}

fn time_tool() -> ToolDefinition {
    ToolDefinition {
        name: "get_time".to_string(),
        description: "Get current time for a timezone".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "timezone": {"type": "string"}
            },
            "required": ["timezone"]
        }),
    }
}

fn weather_and_time_tools() -> Vec<ToolDefinition> {
    vec![weather_tool(), time_tool()]
}

fn build_weather_result() -> &'static str {
    r#"{"temperature": 22, "condition": "sunny"}"#
}

fn build_weather_second_turn(
    first_response: &oxide_agent_core::llm::ChatResponse,
    tool_call: &oxide_agent_core::llm::ToolCall,
) -> Vec<Message> {
    vec![
        Message::user("What's the weather in Tokyo?"),
        Message::assistant_with_tools(
            first_response.content.as_deref().unwrap_or("I'll check the weather for you."),
            first_response.tool_calls.clone(),
        ),
        Message::tool(&tool_call.id, &tool_call.function.name, build_weather_result()),
        Message::user("Is it a nice day?"),
    ]
}


#[tokio::test]
async fn test_mistral_simple_chat() -> Result<()> {
    init_test_env();

    if !should_run_e2e_checks() {
        warn!("Skipping Mistral E2E test: RUN_LLM_E2E_CHECKS != 1");
        return Ok(());
    }

    let api_key = match env::var("MISTRAL_API_KEY") {
        Ok(k) if !k.is_empty() && k != "dummy" => k,
        _ => {
            warn!("Skipping test: valid MISTRAL_API_KEY not set");
            return Ok(());
        }
    };

    info!("=== Test: Simple Chat ===");
    let provider = MistralProvider::new(api_key.clone());

    let messages = vec![Message::user("Say 'hello' in exactly one word.")];

    let result = provider
        .chat_with_tools(ChatWithToolsRequest {
            system_prompt: "You are a helpful assistant. Answer concisely.",
            messages: &messages,
            tools: &[],
            model_id: "mistral-large-latest",
            max_tokens: 50,
            json_mode: false,
        })
        .await;

    match result {
        Ok(response) => {
            info!("Response: {:?}", response.content);
            anyhow::ensure!(
                !response.content.as_ref().expect("content should be present").is_empty(),
                "Expected text content"
            );
            info!("✓ Simple chat test passed");
        }
        Err(e) => {
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_mistral_single_tool_call() -> Result<()> {
    init_test_env();

    if !should_run_e2e_checks() {
        warn!("Skipping Mistral E2E test: RUN_LLM_E2E_CHECKS != 1");
        return Ok(());
    }

    let api_key = match env::var("MISTRAL_API_KEY") {
        Ok(k) if !k.is_empty() && k != "dummy" => k,
        _ => {
            warn!("Skipping test: valid MISTRAL_API_KEY not set");
            return Ok(());
        }
    };

    info!("=== Test: Single Tool Call ===");
    let provider = MistralProvider::new(api_key.clone());

    let tools = vec![ToolDefinition {
        name: "get_weather".to_string(),
        description: "Get the current weather for a city".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "city": {
                    "type": "string",
                    "description": "The city name"
                }
            },
            "required": ["city"]
        }),
    }];

    let messages = vec![Message::user("What's the weather in Tokyo?")];

    let result = provider
        .chat_with_tools(ChatWithToolsRequest {
            system_prompt: "You are a helpful weather assistant. Use the get_weather tool to answer questions about weather.",
            messages: &messages,
            tools: &tools,
            model_id: "mistral-large-latest",
            max_tokens: 1024,
            json_mode: false,
        })
        .await;

    match result {
        Ok(response) => {
            if !response.tool_calls.is_empty() {
                info!("Tool calls: {:?}", response.tool_calls);
                for tc in &response.tool_calls {
                    anyhow::ensure!(!tc.id.is_empty(), "Tool call ID should not be empty");
                    anyhow::ensure!(
                        !tc.function.name.is_empty(),
                        "Function name should not be empty"
                    );
                    info!(
                        "✓ Tool '{}' called with args: {}",
                        tc.function.name, tc.function.arguments
                    );
                }
            } else {
                info!("No tool calls (model chose not to use tools)");
                info!("Text response: {:?}", response.content);
            }
            info!("✓ Single tool call test passed");
        }
        Err(e) => {
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_mistral_tool_call_with_result() -> Result<()> {
    init_test_env();

    if !should_run_e2e_checks() {
        warn!("Skipping Mistral E2E test: RUN_LLM_E2E_CHECKS != 1");
        return Ok(());
    }

    let api_key = match env::var("MISTRAL_API_KEY") {
        Ok(k) if !k.is_empty() && k != "dummy" => k,
        _ => {
            warn!("Skipping test: valid MISTRAL_API_KEY not set");
            return Ok(());
        }
    };

    info!("=== Test: Tool Call WITH Result (Multi-turn) ===");
    let provider = MistralProvider::new(api_key.clone());
    let tools = vec![weather_tool()];

    // First turn: model should call a tool
    let first_request_messages = vec![Message::user("What's the weather in Tokyo?")];

    let first_result = provider
        .chat_with_tools(ChatWithToolsRequest {
            system_prompt: "You are a helpful weather assistant. Use the get_weather tool.",
            messages: &first_request_messages,
            tools: &tools,
            model_id: "mistral-large-latest",
            max_tokens: 1024,
            json_mode: false,
        })
        .await;

    let first_response = match first_result {
        Ok(r) => r,
        Err(e) => {
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    };

    // Make sure we got tool calls
    anyhow::ensure!(
        !first_response.tool_calls.is_empty(),
        "First request should have tool calls"
    );

    let tool_call = &first_response.tool_calls[0];
    info!("First turn: called '{}'", tool_call.function.name);

    // Second turn: add tool result and ask for follow-up
    let second_request_messages = build_weather_second_turn(&first_response, tool_call);
    let second_result = provider.chat_with_tools(ChatWithToolsRequest {
        system_prompt: "You are a helpful weather assistant.",
        messages: &second_request_messages,
        tools: &tools,
        model_id: "mistral-large-latest",
        max_tokens: 1024,
        json_mode: false,
    }).await;

    match second_result {
        Ok(response) => {
            info!("Second response: {:?}", response.content);
            info!("Second response tool_calls: {:?}", response.tool_calls);
            info!("✓ Multi-turn tool call test passed");
        }
        Err(e) => {
            warn!("Second turn FAILED: {}", e);
            // This is the critical test - if it fails here, the bug is confirmed
            if e.to_string().contains("Not the same number") {
                return Err(anyhow::anyhow!(
                    "BUG CONFIRMED: Mistral requires exact match of tool_calls and tool responses"
                ));
            }
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_mistral_multiple_tool_calls_parallel() -> Result<()> {
    init_test_env();

    if !should_run_e2e_checks() {
        warn!("Skipping Mistral E2E test: RUN_LLM_E2E_CHECKS != 1");
        return Ok(());
    }

    let api_key = match env::var("MISTRAL_API_KEY") {
        Ok(k) if !k.is_empty() && k != "dummy" => k,
        _ => {
            warn!("Skipping test: valid MISTRAL_API_KEY not set");
            return Ok(());
        }
    };

    info!("=== Test: Multiple Parallel Tool Calls ===");
    let provider = MistralProvider::new(api_key.clone());

    let tools = vec![
        ToolDefinition {
            name: "get_weather".to_string(),
            description: "Get weather for a city".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "city": { "type": "string" }
                },
                "required": ["city"]
            }),
        },
        ToolDefinition {
            name: "get_time".to_string(),
            description: "Get current time for a timezone".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "timezone": { "type": "string" }
                },
                "required": ["timezone"]
            }),
        },
    ];

    let messages = vec![Message::user(
        "What's the weather in Tokyo and what's the current time in London?",
    )];

    let result = provider
        .chat_with_tools(ChatWithToolsRequest {
            system_prompt: "You are a helpful assistant. Use the available tools.",
            messages: &messages,
            tools: &tools,
            model_id: "mistral-large-latest",
            max_tokens: 1024,
            json_mode: false,
        })
        .await;

    match result {
        Ok(response) => {
            if !response.tool_calls.is_empty() {
                info!("Parallel tool calls: {}", response.tool_calls.len());
                for tc in &response.tool_calls {
                    info!("  - {}: {}", tc.function.name, tc.function.arguments);
                }
            } else {
                info!("No tool calls, text response: {:?}", response.content);
            }
            info!("✓ Parallel tool calls test passed");
        }
        Err(e) => {
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_mistral_parallel_tool_results() -> Result<()> {
    init_test_env();

    if !should_run_e2e_checks() {
        warn!("Skipping Mistral E2E test: RUN_LLM_E2E_CHECKS != 1");
        return Ok(());
    }

    let api_key = match env::var("MISTRAL_API_KEY") {
        Ok(k) if !k.is_empty() && k != "dummy" => k,
        _ => {
            warn!("Skipping test: valid MISTRAL_API_KEY not set");
            return Ok(());
        }
    };

    info!("=== Test: Parallel Tool Calls WITH Results ===");
    let provider = MistralProvider::new(api_key.clone());
    let tools = weather_and_time_tools();

    // First turn: get parallel tool calls
    let first_messages = vec![Message::user(
        "What's the weather in Tokyo and what's the current time in London?",
    )];

    let first_result = provider.chat_with_tools(ChatWithToolsRequest {
        system_prompt: "You are a helpful assistant. Use the available tools.",
        messages: &first_messages,
        tools: &tools,
        model_id: "mistral-large-latest",
        max_tokens: 1024,
        json_mode: false,
    }).await;

    let first_response = match first_result {
        Ok(r) => r,
        Err(e) => {
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    };

    if first_response.tool_calls.is_empty() {
        warn!("Model didn't make parallel tool calls, skipping this test");
        return Ok(());
    }
    info!("First turn: {} tool calls", first_response.tool_calls.len());

    // Second turn: add ALL tool results
    let mut second_messages = vec![Message::user(
        "What's the weather in Tokyo and what's the current time in London?",
    )];
    second_messages.push(Message::assistant_with_tools(
        first_response.content.as_deref().unwrap_or("Let me check both."),
        first_response.tool_calls.clone(),
    ));

    for tc in &first_response.tool_calls {
        let result = match tc.function.name.as_str() {
            "get_weather" => build_weather_result(),
            "get_time" => r#"{"time": "14:30", "timezone": "GMT"}"#,
            _ => r#"{"result": "done"}"#,
        };
        second_messages.push(Message::tool(&tc.id, &tc.function.name, result));
    }

    let second_result = provider.chat_with_tools(ChatWithToolsRequest {
        system_prompt: "You are a helpful assistant. Use the available tools.",
        messages: &second_messages,
        tools: &tools,
        model_id: "mistral-large-latest",
        max_tokens: 1024,
        json_mode: false,
    }).await;

    match second_result {
        Ok(response) => {
            info!("Second response: {:?}", response.content);
            info!("✓ Parallel tool results test passed");
        }
        Err(e) => {
            warn!("Parallel tool results FAILED: {}", e);
            if e.to_string().contains("Not the same number") {
                return Err(anyhow::anyhow!(
                    "BUG CONFIRMED: Parallel tool results not handled correctly"
                ));
            }
            if is_expected_error(&e) {
                warn!("Skipping due to expected error: {}", e);
                return Ok(());
            }
            return Err(anyhow::Error::new(e));
        }
    }

    Ok(())
}

fn is_expected_error(e: &oxide_agent_core::llm::LlmError) -> bool {
    let msg = e.to_string().to_lowercase();
    msg.contains("rate limit") || msg.contains("too many requests") || msg.contains("insufficient")
}
