//! Integration tests that require real API keys.
//!
//! These tests are ignored by default and must be run explicitly with:
//!
//!     cargo test e2e_connection_pool_latency -- --ignored --nocapture
//!
//! Requires one of: `OPENROUTER_API_KEY`, `MISTRAL_API_KEY`, or `ZAI_API_KEY`.

use oxide_agent_core::llm::{LlmClient, Message};
use std::sync::Arc;
use std::time::Instant;

/// Test: Measure HTTP connection pool latency improvement.
///
/// Without connection pool: both requests ~200-400ms (TCP+TLS each time).
/// With connection pool: first ~200-400ms, second ~30-80ms (connection reuse).
///
/// Run with: cargo test e2e_connection_pool_latency -- --ignored --nocapture
#[tokio::test]
#[ignore = "Requires OPENROUTER_API_KEY, MISTRAL_API_KEY, or ZAI_API_KEY environment variable"]
async fn e2e_connection_pool_latency() {
    use oxide_agent_core::config::AgentSettings;

    let (provider_name, model_id) = if std::env::var("OPENROUTER_API_KEY").is_ok() {
        ("openrouter", "openrouter/free")
    } else if std::env::var("MISTRAL_API_KEY").is_ok() {
        ("mistral", "labs-devstral-small-2512")
    } else if std::env::var("ZAI_API_KEY").is_ok() {
        ("zai", "glm-4.7-flash")
    } else {
        panic!("Neither OPENROUTER_API_KEY nor MISTRAL_API_KEY nor ZAI_API_KEY is set");
    };

    eprintln!("Testing connection pool with provider: {}", provider_name);

    let agent_settings = Arc::new({
        let mut s = AgentSettings::default();
        s.agent_model_id = Some(model_id.to_string());
        s.agent_model_provider = Some(provider_name.to_string());
        s.agent_timeout_secs = Some(30);
        match provider_name {
            "openrouter" => {
                s.openrouter_api_key = std::env::var("OPENROUTER_API_KEY").ok();
            }
            "mistral" => {
                s.mistral_api_key = std::env::var("MISTRAL_API_KEY").ok();
            }
            "zai" => {
                s.zai_api_key = std::env::var("ZAI_API_KEY").ok();
                if let Ok(base) = std::env::var("ZAI_API_BASE") {
                    s.zai_api_base = base;
                }
            }
            _ => {}
        }
        s
    });

    let llm = Arc::new(LlmClient::new(&agent_settings));

    let messages = vec![Message::user("Say 'pong' and nothing else")];
    let system_prompt = "You are a helpful assistant.";

    let t1 = Instant::now();
    let resp1 = llm
        .chat_with_tools(system_prompt, &messages, &[], model_id, false)
        .await;
    let time1 = t1.elapsed();
    assert!(resp1.is_ok(), "First request failed: {:?}", resp1.err());
    eprintln!(
        "[CONNECTION-POOL] First request (cold): {}ms",
        time1.as_millis()
    );

    let t2 = Instant::now();
    let resp2 = llm
        .chat_with_tools(system_prompt, &messages, &[], model_id, false)
        .await;
    let time2 = t2.elapsed();
    assert!(resp2.is_ok(), "Second request failed: {:?}", resp2.err());
    eprintln!(
        "[CONNECTION-POOL] Second request (warm): {}ms",
        time2.as_millis()
    );

    let improvement = if time1.as_millis() > 0 {
        ((time1.as_millis() - time2.as_millis()) as f64 / time1.as_millis() as f64) * 100.0
    } else {
        0.0
    };

    eprintln!("[CONNECTION-POOL] Improvement: {:.1}%", improvement);
    eprintln!(
        "[CONNECTION-POOL] Baseline: first={}ms, second={}ms",
        time1.as_millis(),
        time2.as_millis()
    );

    assert!(
        time2.as_millis() < time1.as_millis(),
        "Second request should be faster than first with connection pooling. \
         First: {}ms, Second: {}ms. If similar, connection pool may not be working.",
        time1.as_millis(),
        time2.as_millis()
    );

    assert!(
        improvement > 30.0,
        "Connection pool should provide >30% improvement on second request. \
         Current improvement: {:.1}%. First: {}ms, Second: {}ms",
        improvement,
        time1.as_millis(),
        time2.as_millis()
    );
}
