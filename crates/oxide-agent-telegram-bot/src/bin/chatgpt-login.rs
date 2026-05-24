use anyhow::{bail, Result};
use dotenvy::dotenv;
use oxide_agent_core::capabilities::compiled_capability_manifest;
use oxide_agent_core::config::load_module_runtime_settings;
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

#[derive(Debug, Clone, PartialEq, Eq)]
enum CapabilityCommand {
    CompiledManifest,
    EnabledManifest { config_path: Option<String> },
    ConfigSchema,
}

#[tokio::main]
async fn main() -> Result<()> {
    if let Some(command) = parse_capability_startup_command(env::args().skip(1))? {
        run_capability_command(command)?;
        return Ok(());
    }

    dotenv().ok();

    let cli = parse_cli()?;

    match cli.command {
        Command::Login => run_login(cli.auth_path).await,
        Command::Status => run_status(cli.auth_path).await,
    }
}

fn run_capability_command(command: CapabilityCommand) -> Result<()> {
    match command {
        CapabilityCommand::CompiledManifest => {
            let manifest = compiled_capability_manifest()?;
            println!("{}", manifest.to_json_pretty()?);
        }
        CapabilityCommand::EnabledManifest { config_path } => {
            let manifest = compiled_capability_manifest()?;
            let module_settings = load_module_runtime_settings(config_path.as_deref())?;
            let enabled = module_settings.enabled_capability_manifest(&manifest)?;
            println!("{}", enabled.to_json_pretty()?);
        }
        CapabilityCommand::ConfigSchema => {
            let manifest = compiled_capability_manifest()?;
            println!("{}", manifest.config_schema_json_pretty()?);
        }
    }
    Ok(())
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
    parse_cli_from_parts(env::args().skip(1))
}

fn parse_cli_from_parts<I, S>(args: I) -> Result<Cli>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let first = args.next();
    let second = args.next();
    if args.next().is_some() {
        bail!("Usage: chatgpt-login [login|status] [auth_path]");
    }

    let env_auth_path = env::var("CHATGPT_AUTH_PATH")
        .ok()
        .filter(|value| !value.trim().is_empty());

    let first = first.as_ref().map(AsRef::as_ref);
    let second = second.as_ref().map(AsRef::as_ref);
    let (command, path_arg) = match (first, second) {
        (Some("login"), path_arg) => (Command::Login, path_arg),
        (Some("status"), path_arg) => (Command::Status, path_arg),
        (Some(path_arg), None) => (Command::Login, Some(path_arg)),
        (None, None) => (Command::Login, None),
        _ => bail!("Usage: chatgpt-login [login|status] [auth_path]"),
    };

    let auth_path = resolve_auth_file_path(path_arg.or(env_auth_path.as_deref()))?;

    Ok(Cli { command, auth_path })
}

fn parse_capability_startup_command<I, S>(args: I) -> Result<Option<CapabilityCommand>>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    match args.next().as_ref().map(AsRef::as_ref) {
        Some("capabilities") => parse_capabilities_command(args).map(Some),
        _ => Ok(None),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CapabilityMode {
    Compiled,
    Enabled,
    ConfigSchema,
}

fn parse_capabilities_command<I, S>(args: I) -> Result<CapabilityCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut mode = None;
    let mut json = false;
    let mut config_path = None;
    let mut args = args.into_iter();

    while let Some(arg) = args.next() {
        match arg.as_ref() {
            "--compiled" => set_capability_mode(&mut mode, CapabilityMode::Compiled)?,
            "--enabled" => set_capability_mode(&mut mode, CapabilityMode::Enabled)?,
            "--config-schema" => set_capability_mode(&mut mode, CapabilityMode::ConfigSchema)?,
            "--json" => json = true,
            "--config" => {
                if config_path.is_some() {
                    return capabilities_usage_error();
                }
                config_path = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!(capabilities_usage()))?
                        .as_ref()
                        .to_string(),
                );
            }
            _ => return capabilities_usage_error(),
        }
    }

    if !json {
        return capabilities_usage_error();
    }

    match mode {
        Some(CapabilityMode::Compiled) if config_path.is_none() => {
            Ok(CapabilityCommand::CompiledManifest)
        }
        Some(CapabilityMode::Enabled) => Ok(CapabilityCommand::EnabledManifest { config_path }),
        Some(CapabilityMode::ConfigSchema) if config_path.is_none() => {
            Ok(CapabilityCommand::ConfigSchema)
        }
        _ => capabilities_usage_error(),
    }
}

fn set_capability_mode(mode: &mut Option<CapabilityMode>, next_mode: CapabilityMode) -> Result<()> {
    if mode.replace(next_mode).is_some() {
        return capabilities_usage_error();
    }
    Ok(())
}

fn capabilities_usage_error<T>() -> Result<T> {
    bail!(capabilities_usage())
}

const fn capabilities_usage() -> &'static str {
    "Usage: chatgpt-login capabilities (--compiled | --enabled [--config PATH] | --config-schema) --json"
}

#[cfg(test)]
mod tests {
    use super::{parse_capability_startup_command, CapabilityCommand, Cli, Command};
    use std::path::PathBuf;

    #[test]
    fn parses_default_login_command() {
        let cli = parse_cli_from(["login", "/tmp/auth.json"]).expect("login should parse");
        assert_eq!(
            cli,
            Cli {
                command: Command::Login,
                auth_path: PathBuf::from("/tmp/auth.json")
            }
        );
    }

    #[test]
    fn capability_parser_ignores_login_commands() {
        let command = parse_capability_startup_command(["login", "/tmp/auth.json"])
            .expect("non-capability args should not fail");
        assert_eq!(command, None);
    }

    #[test]
    fn capability_parser_accepts_compiled_json() {
        let command = parse_capability_startup_command(["capabilities", "--compiled", "--json"])
            .expect("compiled capabilities should parse");
        assert_eq!(command, Some(CapabilityCommand::CompiledManifest));
    }

    #[test]
    fn capability_parser_accepts_enabled_json_with_config() {
        let command = parse_capability_startup_command([
            "capabilities",
            "--enabled",
            "--config",
            "config/local.yaml",
            "--json",
        ])
        .expect("enabled capabilities should parse");
        assert_eq!(
            command,
            Some(CapabilityCommand::EnabledManifest {
                config_path: Some("config/local.yaml".to_string())
            })
        );
    }

    #[test]
    fn capability_parser_accepts_schema_json() {
        let command =
            parse_capability_startup_command(["capabilities", "--config-schema", "--json"])
                .expect("config schema should parse");
        assert_eq!(command, Some(CapabilityCommand::ConfigSchema));
    }

    #[test]
    fn capability_parser_rejects_missing_json() {
        let error = parse_capability_startup_command(["capabilities", "--compiled"])
            .expect_err("missing json flag should fail");
        assert!(error.to_string().contains("Usage: chatgpt-login"));
    }

    fn parse_cli_from<I, S>(args: I) -> anyhow::Result<Cli>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect::<Vec<_>>();
        super::parse_cli_from_parts(args)
    }
}
