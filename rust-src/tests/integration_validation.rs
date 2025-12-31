use another_chat_rs::config::Settings;
use another_chat_rs::llm::LlmClient;
use another_chat_rs::storage::R2Storage;
use dotenvy::dotenv;
use tracing::info;
use tracing_subscriber::{prelude::*, EnvFilter};

#[tokio::test]
#[ignore] // Ignored by default as it requires real credentials
async fn test_credentials_validation() {
    // Load .env file
    dotenv().ok();

    // Setup logging
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .init();

    info!("Starting integration test for credentials validation...");

    // 1. Validate Settings Load
    let settings = Settings::new().expect("Failed to load settings from environment");

    // Check if critical keys are present
    assert!(
        settings.telegram_token.len() > 0,
        "TELEGRAM_TOKEN is missing"
    );

    // 2. Validate R2 Storage
    info!("Validating R2 Storage credentials...");
    let storage = R2Storage::new(&settings)
        .await
        .expect("Failed to initialize R2 Storage struct");

    let connected = storage.check_connection().await;
    assert!(
        connected,
        "Failed to connect to R2 Storage with provided credentials"
    );
    info!("R2 Storage connection successful!");

    // 3. Validate LLM Client (Static check of API keys presence)
    info!("Validating LLM Client configuration...");
    let _llm_client = LlmClient::new(&settings);

    // We could try a simple completion if we want to be thorough, but that costs money/tokens.
    // For now, we check if at least one provider is configured.
    let has_provider = settings.groq_api_key.is_some()
        || settings.mistral_api_key.is_some()
        || settings.gemini_api_key.is_some()
        || settings.openrouter_api_key.is_some();

    assert!(
        has_provider,
        "At least one LLM provider API key must be set"
    );
    info!("LLM Client configuration looks valid (providers configured).");

    info!("Credentials validation test passed successfully.");
}
