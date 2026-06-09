use dotenvy::dotenv;
use oxide_agent_core::capabilities::{compiled_capability_manifest, compiled_profile_name};
use oxide_agent_core::config::{AgentSettings, load_module_runtime_settings};
use oxide_agent_core::llm::LlmClient;
use oxide_agent_runtime::SessionRegistry;
use oxide_agent_transport_web::session::WebSessionManager;
use oxide_agent_transport_web::{AppState, serve};
use std::env;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Clone, Debug, Eq, PartialEq)]
enum StartupCommand {
    RunServer,
    PrintCompiledCapabilitiesJson,
    PrintEnabledCapabilitiesJson { config_path: Option<String> },
    PrintCompiledConfigSchemaJson,
    PrintConfigExample { profile: String, json: bool },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_startup_command(env::args().skip(1))? {
        StartupCommand::RunServer => {}
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
        StartupCommand::PrintConfigExample { profile, json } => {
            validate_requested_profile_matches_compiled(&profile)?;
            let manifest = compiled_capability_manifest()?;
            if json {
                println!("{}", manifest.config_example_json_pretty(&profile)?);
            } else {
                print!("{}", manifest.config_example_yaml(&profile)?);
            }
            return Ok(());
        }
    }

    dotenv().ok();
    init_logging();

    let addr = env::var("OXIDE_WEB_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:3010".to_string())
        .parse::<SocketAddr>()?;

    let agent_settings = Arc::new(AgentSettings::new()?);
    let llm = Arc::new(LlmClient::new(agent_settings.as_ref()));
    let provider_names = llm.configured_provider_names();
    if provider_names.is_empty() {
        warn!(
            "No LLM providers are configured; web tasks will fail until provider credentials are set"
        );
    } else {
        info!(providers = ?provider_names, "Configured LLM providers");
    }

    let state = build_app_state(SessionRegistry::new(), llm, agent_settings).await?;
    info!("Starting Oxide Agent web console at http://{addr}");
    serve(state, addr).await;
    Ok(())
}

fn parse_startup_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(StartupCommand::RunServer);
    };

    match first.as_ref() {
        "capabilities" => parse_capabilities_command(args),
        "config" => parse_config_command(args),
        _ => Ok(StartupCommand::RunServer),
    }
}

fn parse_capabilities_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut mode = None;
    let mut config_path = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_ref() {
            "--compiled" => set_capability_mode(&mut mode, CapabilityMode::Compiled)?,
            "--enabled" => set_capability_mode(&mut mode, CapabilityMode::Enabled)?,
            "--config-schema" => set_capability_mode(&mut mode, CapabilityMode::ConfigSchema)?,
            "--config" => {
                let Some(path) = args.next() else {
                    return Err(capabilities_usage_error());
                };
                config_path = Some(path.as_ref().to_string());
            }
            "--json" => {}
            _ => return Err(capabilities_usage_error()),
        }
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

fn parse_config_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(subcommand) = args.next() else {
        return Err(config_usage_error());
    };

    match subcommand.as_ref() {
        "schema" => parse_config_schema_command(args),
        "example" => parse_config_example_command(args),
        _ => Err(config_usage_error()),
    }
}

fn parse_config_schema_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut compiled = false;
    let mut json = false;
    for arg in args {
        match arg.as_ref() {
            "--compiled" => compiled = true,
            "--json" => json = true,
            _ => return Err(config_usage_error()),
        }
    }

    if compiled && json {
        Ok(StartupCommand::PrintCompiledConfigSchemaJson)
    } else {
        Err(config_usage_error())
    }
}

fn parse_config_example_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let mut profile = None;
    let mut json = false;
    while let Some(arg) = args.next() {
        match arg.as_ref() {
            "--profile" => {
                let Some(value) = args.next() else {
                    return Err(config_usage_error());
                };
                profile = Some(value.as_ref().to_string());
            }
            "--json" => json = true,
            _ => return Err(config_usage_error()),
        }
    }

    let Some(profile) = profile else {
        return Err(config_usage_error());
    };

    Ok(StartupCommand::PrintConfigExample { profile, json })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapabilityMode {
    Compiled,
    Enabled,
    ConfigSchema,
}

fn set_capability_mode(
    mode: &mut Option<CapabilityMode>,
    new_mode: CapabilityMode,
) -> io::Result<()> {
    match mode {
        Some(existing) if *existing != new_mode => Err(capabilities_usage_error()),
        _ => {
            *mode = Some(new_mode);
            Ok(())
        }
    }
}

fn capabilities_usage_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Usage: oxide-agent-web-console capabilities --compiled --json | --enabled --config PATH --json | --config-schema --json",
    )
}

fn config_usage_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Usage: oxide-agent-web-console config schema --compiled --json | config example --profile PROFILE [--json]",
    )
}

fn validate_requested_profile_matches_compiled(profile: &str) -> io::Result<()> {
    let compiled_profile = compiled_profile_name().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "config example requires exactly one compiled profile feature",
        )
    })?;

    if compiled_profile != profile {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "requested profile {profile:?} does not match compiled profile {compiled_profile:?}"
            ),
        ));
    }

    Ok(())
}

async fn build_app_state(
    registry: SessionRegistry,
    llm: Arc<LlmClient>,
    agent_settings: Arc<AgentSettings>,
) -> Result<AppState, Box<dyn std::error::Error>> {
    if unsupported_web_store_env() {
        return Err("unsupported OXIDE_WEB_STORE value; use sqlx or postgres".into());
    }
    if use_sqlx_web_store(agent_settings.as_ref()) {
        return build_sqlx_app_state(registry, llm, agent_settings).await;
    }

    let session_manager = WebSessionManager::new(registry, llm, agent_settings);
    Ok(AppState::new(Arc::new(session_manager)))
}

#[cfg(feature = "storage-sqlx")]
async fn build_sqlx_app_state(
    registry: SessionRegistry,
    llm: Arc<LlmClient>,
    agent_settings: Arc<AgentSettings>,
) -> Result<AppState, Box<dyn std::error::Error>> {
    oxide_agent_transport_web::build_sqlx_backed_app_state(registry, llm, agent_settings)
        .await
        .map_err(Into::into)
}

#[cfg(not(feature = "storage-sqlx"))]
async fn build_sqlx_app_state(
    _registry: SessionRegistry,
    _llm: Arc<LlmClient>,
    _agent_settings: Arc<AgentSettings>,
) -> Result<AppState, Box<dyn std::error::Error>> {
    Err("SQLx/Postgres web persistence requires the storage-sqlx feature".into())
}

fn unsupported_web_store_env() -> bool {
    web_store_env().is_some_and(|value| value != "sqlx" && value != "postgres")
}

fn use_sqlx_web_store(agent_settings: &AgentSettings) -> bool {
    web_store_env().is_some_and(|value| value == "sqlx" || value == "postgres")
        || durable_web_store_required()
        || sqlx_web_store_configured(agent_settings)
}

#[cfg(feature = "storage-sqlx")]
fn sqlx_web_store_configured(agent_settings: &AgentSettings) -> bool {
    agent_settings.is_module_enabled("storage/sqlx")
        && oxide_agent_core::storage::SqlxStorageConfig::is_configured(agent_settings)
}

#[cfg(not(feature = "storage-sqlx"))]
fn sqlx_web_store_configured(_agent_settings: &AgentSettings) -> bool {
    false
}

fn durable_web_store_required() -> bool {
    production_run_mode()
        || web_bool_env("OXIDE_WEB_ENABLED")
        || web_bool_env("OXIDE_WEB_REQUIRE_DURABLE_STORAGE")
}

fn production_run_mode() -> bool {
    env::var("RUN_MODE").is_ok_and(|value| {
        let value = value.trim().to_ascii_lowercase();
        value == "prod" || value == "production"
    })
}

fn web_store_env() -> Option<String> {
    env::var("OXIDE_WEB_STORE")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

fn web_bool_env(key: &str) -> bool {
    env::var(key).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

fn init_logging() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(
            "oxide_agent_core=info,oxide_agent_transport_web=info,oxide_agent_runtime=info,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn",
        )
    });
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
