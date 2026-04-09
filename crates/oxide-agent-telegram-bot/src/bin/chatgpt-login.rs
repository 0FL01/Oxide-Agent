use anyhow::Result;
use dotenvy::dotenv;
use oxide_agent_core::llm::providers::chatgpt::{
    auth_file_host_path_from_container_path, ChatGptProvider,
};
use std::env;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let auth_path = resolve_auth_path();
    let device = ChatGptProvider::begin_headless_login().await?;

    println!(
        "Open {} and enter code: {}",
        device.verification_url, device.user_code
    );
    println!("Auth file: {}", auth_path.display());
    println!("Waiting for authorization...");

    let record = ChatGptProvider::complete_headless_login(auth_path.clone(), &device).await?;

    println!("Saved ChatGPT OAuth auth to {}", auth_path.display());
    println!("Account ID: {}", record.account_id);
    println!("Expires at (ms): {}", record.expires_at_ms);

    Ok(())
}

fn resolve_auth_path() -> PathBuf {
    let cli_arg = env::args()
        .nth(1)
        .unwrap_or_else(|| "config/chatgpt/auth.json".to_string());
    auth_file_host_path_from_container_path(&cli_arg).unwrap_or_else(|_| PathBuf::from(cli_arg))
}
