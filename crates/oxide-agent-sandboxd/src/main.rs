use dotenvy::dotenv;
use oxide_agent_core::capabilities::compiled_capability_manifest;
use oxide_agent_core::config::load_module_runtime_settings;
use oxide_agent_core::sandbox::SandboxBrokerServer;
use std::{env, io};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug, Eq, PartialEq)]
enum StartupCommand {
    RunBroker,
    PrintCompiledCapabilitiesJson,
    PrintEnabledCapabilitiesJson { config_path: Option<String> },
    PrintCompiledConfigSchemaJson,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_startup_command(env::args().skip(1))? {
        StartupCommand::RunBroker => {}
        StartupCommand::PrintCompiledCapabilitiesJson => {
            let manifest = compiled_capability_manifest()?;
            println!("{}", manifest.to_json_pretty()?);
            return Ok(());
        }
        StartupCommand::PrintEnabledCapabilitiesJson { config_path } => {
            let manifest = compiled_capability_manifest()?;
            let module_settings = load_module_runtime_settings(config_path.as_deref())?;
            let enabled = module_settings
                .enabled_capability_manifest(&manifest)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;
            println!("{}", enabled.to_json_pretty()?);
            return Ok(());
        }
        StartupCommand::PrintCompiledConfigSchemaJson => {
            let manifest = compiled_capability_manifest()?;
            println!("{}", manifest.config_schema_json_pretty()?);
            return Ok(());
        }
    }

    dotenv().ok();
    init_logging();

    let server = SandboxBrokerServer::bind_default().await?;
    info!(socket_path = %server.socket_path().display(), "Starting sandbox broker");
    server.serve().await?;
    Ok(())
}

fn init_logging() {
    let debug_mode = std::env::var("DEBUG_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let filter = if debug_mode {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new("oxide_agent_core=info,oxide_agent_sandboxd=info,bollard=warn")
        })
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn parse_startup_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    match args.next().as_ref().map(AsRef::as_ref) {
        None => Ok(StartupCommand::RunBroker),
        Some("capabilities") => parse_capabilities_command(args),
        Some(_) => Err(capabilities_usage_error()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapabilityMode {
    Compiled,
    Enabled,
    ConfigSchema,
}

fn parse_capabilities_command<I, S>(args: I) -> io::Result<StartupCommand>
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
                    return Err(capabilities_usage_error());
                }
                config_path = Some(
                    args.next()
                        .ok_or_else(capabilities_usage_error)?
                        .as_ref()
                        .to_string(),
                );
            }
            _ => return Err(capabilities_usage_error()),
        }
    }

    if !json {
        return Err(capabilities_usage_error());
    }

    match mode {
        Some(CapabilityMode::Compiled) if config_path.is_none() => {
            Ok(StartupCommand::PrintCompiledCapabilitiesJson)
        }
        Some(CapabilityMode::Enabled) => {
            Ok(StartupCommand::PrintEnabledCapabilitiesJson { config_path })
        }
        Some(CapabilityMode::ConfigSchema) if config_path.is_none() => {
            Ok(StartupCommand::PrintCompiledConfigSchemaJson)
        }
        _ => Err(capabilities_usage_error()),
    }
}

fn set_capability_mode(
    mode: &mut Option<CapabilityMode>,
    next_mode: CapabilityMode,
) -> io::Result<()> {
    if mode.replace(next_mode).is_some() {
        return Err(capabilities_usage_error());
    }
    Ok(())
}

fn capabilities_usage_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Usage: oxide-agent-sandboxd capabilities (--compiled | --enabled [--config PATH] | --config-schema) --json",
    )
}

#[cfg(test)]
mod tests {
    use super::{StartupCommand, parse_startup_command};
    use std::io;

    #[test]
    fn parses_default_broker_startup() {
        let command = parse_startup_command(std::iter::empty::<&str>())
            .expect("empty args should run broker");
        assert_eq!(command, StartupCommand::RunBroker);
    }

    #[test]
    fn parses_compiled_capabilities_json() {
        let command = parse_startup_command(["capabilities", "--compiled", "--json"])
            .expect("compiled capabilities should parse");
        assert_eq!(command, StartupCommand::PrintCompiledCapabilitiesJson);
    }

    #[test]
    fn parses_enabled_capabilities_json_with_config() {
        let command = parse_startup_command([
            "capabilities",
            "--enabled",
            "--config",
            "config/local.yaml",
            "--json",
        ])
        .expect("enabled capabilities should parse");
        assert_eq!(
            command,
            StartupCommand::PrintEnabledCapabilitiesJson {
                config_path: Some("config/local.yaml".to_string())
            }
        );
    }

    #[test]
    fn parses_config_schema_json() {
        let command = parse_startup_command(["capabilities", "--config-schema", "--json"])
            .expect("config schema should parse");
        assert_eq!(command, StartupCommand::PrintCompiledConfigSchemaJson);
    }

    #[test]
    fn rejects_partial_capabilities_command() {
        let error = parse_startup_command(["capabilities", "--compiled"])
            .expect_err("missing json flag should fail");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
