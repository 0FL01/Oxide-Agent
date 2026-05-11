use anyhow::{bail, Result};
use dotenvy::dotenv;
use oxide_agent_core::llm::providers::chatgpt::{
    resolve_auth_file_path, ChatGptAuthFlow, ChatGptAuthStatus,
};
use std::env;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Login,
    Status,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Cli {
    command: Command,
    auth_path: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    let cli = parse_cli()?;

    match cli.command {
        Command::Login => run_login(cli.auth_path).await,
        Command::Status => run_status(cli.auth_path).await,
    }
}

async fn run_login(auth_path: PathBuf) -> Result<()> {
    let flow = ChatGptAuthFlow::new(auth_path.clone());
    let device = flow.start().await?;

    println!(
        "Open {} and enter code: {}",
        device.verification_url, device.user_code
    );
    println!("Auth file: {}", auth_path.display());
    println!("Waiting for authorization...");

    let record = flow.wait_for_completion(&device).await?;

    println!("Saved ChatGPT OAuth auth to {}", auth_path.display());
    println!("Account ID: {}", record.account_id);
    println!("Expires at (ms): {}", record.expires_at_ms);

    Ok(())
}

async fn run_status(auth_path: PathBuf) -> Result<()> {
    let flow = ChatGptAuthFlow::new(auth_path.clone());
    match flow.status().await? {
        ChatGptAuthStatus::Missing { auth_path } => {
            println!("Auth file: {}", auth_path.display());
            println!("Status: missing");
        }
        ChatGptAuthStatus::Available {
            auth_path,
            record,
            expired,
        } => {
            println!("Auth file: {}", auth_path.display());
            println!("Status: {}", if expired { "expired" } else { "valid" });
            println!("Account ID: {}", record.account_id);
            println!("Expires at (ms): {}", record.expires_at_ms);
        }
    }

    Ok(())
}

fn parse_cli() -> Result<Cli> {
    let mut args = env::args().skip(1);
    let first = args.next();
    let second = args.next();
    if args.next().is_some() {
        bail!("Usage: chatgpt-login [login|status] [auth_path]");
    }

    let env_auth_path = env::var("CHATGPT_AUTH_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let (command, path_arg) = match (first.as_deref(), second.as_deref()) {
        (Some("login"), path_arg) => (Command::Login, path_arg),
        (Some("status"), path_arg) => (Command::Status, path_arg),
        (Some(path_arg), None) => (Command::Login, Some(path_arg)),
        (None, None) => (Command::Login, None),
        _ => bail!("Usage: chatgpt-login [login|status] [auth_path]"),
    };

    let auth_path = resolve_auth_file_path(path_arg.or(env_auth_path.as_deref()))?;

    Ok(Cli { command, auth_path })
}
