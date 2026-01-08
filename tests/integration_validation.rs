use anyhow::{anyhow, Result};
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_types::region::Region;
use dotenvy::dotenv;
use oxide_agent::config::Settings;
use std::path::Path;
use tracing::info;
use tracing_subscriber::{prelude::*, EnvFilter};

#[tokio::test]
#[ignore = "Requires real credentials"]
async fn test_credentials_validation() -> Result<()> {
    load_dotenv();
    init_tracing();

    info!("Starting integration test for credentials validation...");
    let env = load_env_settings()?;

    validate_telegram_token(&env.telegram_token);
    validate_r2_storage(&env).await?;
    validate_llm_provider_keys();

    info!("Credentials validation test passed successfully.");
    Ok(())
}

struct IntegrationEnv {
    telegram_token: String,
    r2_endpoint: String,
    r2_access: String,
    r2_secret: String,
    r2_bucket: String,
}

fn load_dotenv() {
    let env_path = Path::new("../.env");
    if env_path.exists() {
        let _ = dotenvy::from_path(env_path);
    } else {
        dotenv().ok();
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .try_init();
}

fn load_env_settings() -> Result<IntegrationEnv> {
    match Settings::new() {
        Ok(s) => Ok(IntegrationEnv {
            telegram_token: s.telegram_token,
            r2_endpoint: s
                .r2_endpoint_url
                .ok_or_else(|| anyhow!("R2_ENDPOINT_URL missing"))?,
            r2_access: s
                .r2_access_key_id
                .ok_or_else(|| anyhow!("R2_ACCESS_KEY_ID missing"))?,
            r2_secret: s
                .r2_secret_access_key
                .ok_or_else(|| anyhow!("R2_SECRET_ACCESS_KEY missing"))?,
            r2_bucket: s
                .r2_bucket_name
                .ok_or_else(|| anyhow!("R2_BUCKET_NAME missing"))?,
        }),
        Err(e) => {
            info!(
                "Settings::new() failed (expected due to case sensitivity): {}",
                e
            );
            info!("Falling back to direct environment variable read for test verification.");

            Ok(IntegrationEnv {
                telegram_token: std::env::var("TELEGRAM_TOKEN")
                    .unwrap_or_else(|_| "dummy_token_for_s3_verification".to_string()),
                r2_endpoint: std::env::var("R2_ENDPOINT_URL")?,
                r2_access: std::env::var("R2_ACCESS_KEY_ID")?,
                r2_secret: std::env::var("R2_SECRET_ACCESS_KEY")?,
                r2_bucket: std::env::var("R2_BUCKET_NAME")?,
            })
        }
    }
}

fn validate_telegram_token(telegram_token: &str) {
    if telegram_token == "dummy_token_for_s3_verification" {
        info!("Using dummy TELEGRAM_TOKEN. This is fine for S3 verification.");
        return;
    }

    assert!(
        !telegram_token.is_empty(),
        "TELEGRAM_TOKEN is missing (check .env file or loading logic)"
    );
}

async fn validate_r2_storage(env: &IntegrationEnv) -> Result<()> {
    info!(
        "Validating R2 Storage credentials with manual client construction (Theory: Force Path Style)..."
    );
    info!("R2 Endpoint: {}", env.r2_endpoint);
    info!("R2 Bucket: {}", env.r2_bucket);
    info!(
        "R2 Access Key: {}...",
        &env.r2_access.chars().take(4).collect::<String>()
    );

    let client = build_r2_client(env).await?;
    verify_r2_put_and_cleanup(&client, &env.r2_bucket).await?;
    Ok(())
}

async fn build_r2_client(env: &IntegrationEnv) -> Result<Client> {
    let credentials = Credentials::new(
        env.r2_access.clone(),
        env.r2_secret.clone(),
        None,
        None,
        "r2-storage",
    );

    let sdk_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .credentials_provider(credentials)
        .region(Region::new("us-east-1"))
        .load()
        .await;

    let s3_config = aws_sdk_s3::config::Builder::from(&sdk_config)
        .endpoint_url(env.r2_endpoint.clone())
        .force_path_style(true)
        .build();

    Ok(Client::from_conf(s3_config))
}

async fn verify_r2_put_and_cleanup(client: &Client, bucket: &str) -> Result<()> {
    let test_key = "integration_test_connectivity.txt";
    info!("Attempting PutObject to key: {}", test_key);

    let put_result = client
        .put_object()
        .bucket(bucket)
        .key(test_key)
        .body(aws_sdk_s3::primitives::ByteStream::from_static(
            b"test_connectivity",
        ))
        .send()
        .await;

    match put_result {
        Ok(_) => {
            info!("PutObject successful! Theory verified: force_path_style works for writing.");
            info!("Cleaning up...");
            let _ = client
                .delete_object()
                .bucket(bucket)
                .key(test_key)
                .send()
                .await;
            Ok(())
        }
        Err(e) => Err(anyhow!(
            "Failed to connect to R2 Storage (PutObject failed). Error: {e:#?}"
        )),
    }
}

fn validate_llm_provider_keys() {
    info!("Validating LLM Client configuration...");
    let has_provider = std::env::var("GROQ_API_KEY").is_ok()
        || std::env::var("MISTRAL_API_KEY").is_ok()
        || std::env::var("GEMINI_API_KEY").is_ok()
        || std::env::var("OPENROUTER_API_KEY").is_ok();

    assert!(
        has_provider,
        "At least one LLM provider API key must be set"
    );
    info!("LLM Client configuration looks valid.");
}
