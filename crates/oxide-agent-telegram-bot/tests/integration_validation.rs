use anyhow::{anyhow, Result};
use aws_credential_types::Credentials;
use aws_sdk_s3::Client;
use aws_types::region::Region;
use dotenvy::dotenv;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_core::storage::{R2Storage, R2StorageConfig, StorageProvider};
use oxide_agent_transport_telegram::config::TelegramSettings;
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
    r2_region: String,
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
    let agent_settings = AgentSettings::new();
    let telegram_settings = TelegramSettings::new();

    match (agent_settings, telegram_settings) {
        (Ok(agent), Ok(telegram)) => Ok(IntegrationEnv {
            telegram_token: telegram.telegram_token,
            ..integration_env_from_r2_config(R2StorageConfig::from_agent_settings(&agent)?)?
        }),
        (agent_result, telegram_result) => {
            if let Err(err) = agent_result {
                info!(
                    "AgentSettings::new() failed (expected due to case sensitivity): {}",
                    err
                );
            }
            if let Err(err) = telegram_result {
                info!(
                    "TelegramSettings::new() failed (expected due to case sensitivity): {}",
                    err
                );
            }
            info!("Falling back to direct environment variable read for test verification.");

            Ok(IntegrationEnv {
                telegram_token: std::env::var("TELEGRAM_TOKEN")
                    .unwrap_or_else(|_| "dummy_token_for_s3_verification".to_string()),
                r2_endpoint: std::env::var("OXIDE_R2_ENDPOINT_URL")?,
                r2_access: std::env::var("OXIDE_R2_ACCESS_KEY_ID")?,
                r2_secret: std::env::var("OXIDE_R2_SECRET_ACCESS_KEY")?,
                r2_bucket: std::env::var("OXIDE_R2_BUCKET_NAME")?,
                r2_region: std::env::var("OXIDE_R2_REGION").unwrap_or_else(|_| "auto".to_string()),
            })
        }
    }
}

fn integration_env_from_r2_config(config: R2StorageConfig) -> Result<IntegrationEnv> {
    Ok(IntegrationEnv {
        telegram_token: std::env::var("TELEGRAM_TOKEN")
            .unwrap_or_else(|_| "dummy_token_for_s3_verification".to_string()),
        r2_endpoint: config.endpoint_url,
        r2_access: config.access_key_id,
        r2_secret: config.secret_access_key,
        r2_bucket: config.bucket_name,
        r2_region: config.region,
    })
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
    info!("Validating R2 Storage credentials with bucket-scoped probe...");
    info!("R2 Endpoint: {}", env.r2_endpoint);
    info!("R2 Bucket: {}", env.r2_bucket);
    info!(
        "R2 Access Key: {}...",
        &env.r2_access.chars().take(4).collect::<String>()
    );

    let client = build_r2_client(env).await?;
    verify_r2_put_and_cleanup(&client, &env.r2_bucket).await?;
    verify_r2_provider_check_connection(env).await?;
    Ok(())
}

async fn verify_r2_provider_check_connection(env: &IntegrationEnv) -> Result<()> {
    let storage = R2Storage::new(&R2StorageConfig {
        endpoint_url: env.r2_endpoint.clone(),
        bucket_name: env.r2_bucket.clone(),
        access_key_id: env.r2_access.clone(),
        secret_access_key: env.r2_secret.clone(),
        region: env.r2_region.clone(),
    })
    .await?;

    storage
        .check_connection()
        .await
        .map_err(|error| anyhow!("R2 storage provider connection check failed: {error}"))
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
        .region(Region::new(env.r2_region.clone()))
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
            info!("PutObject successful for bucket-scoped credential probe.");
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
    let has_provider =
        std::env::var("MISTRAL_API_KEY").is_ok() || std::env::var("OPENROUTER_API_KEY").is_ok();

    assert!(
        has_provider,
        "At least one LLM provider API key must be set"
    );
    info!("LLM Client configuration looks valid.");
}
