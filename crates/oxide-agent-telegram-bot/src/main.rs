use dotenvy::dotenv;
use oxide_agent_core::agent::providers::cleanup_stale_private_key_tempfiles;
use oxide_agent_core::capabilities::compiled_capability_manifest;
use oxide_agent_core::config::{load_module_runtime_settings, AgentSettings};
use oxide_agent_transport_telegram::config::{BotSettings, TelegramSettings};
use oxide_agent_transport_telegram::runner::run_bot;
use regex::Regex;
use std::env;
use std::io::{self, Write};
use std::sync::Arc;
use tracing::{error, info, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

/// Regex patterns for redacting sensitive data
struct RedactionPatterns {
    token1: Regex,
    token2: Regex,
    token3: Regex,
    r2_1: Regex,
    r2_2: Regex,
    r2_3: Regex,
    r2_4: Regex,
}

impl RedactionPatterns {
    /// Initialize all regex patterns
    ///
    /// # Errors
    ///
    /// Returns an error if any regex pattern is invalid
    fn new() -> Result<Self, regex::Error> {
        Ok(Self {
            token1: Regex::new(r"(https?://[^/]+/bot)([0-9]+:[A-Za-z0-9_-]+)(/['\s]*)")?,
            token2: Regex::new(r"([0-9]{8,10}:[A-Za-z0-9_-]{35})")?,
            token3: Regex::new(r"(bot[0-9]{8,10}:)[A-Za-z0-9_-]+")?,
            r2_1: Regex::new(r"R2_ACCESS_KEY_ID=[^\s&]+")?,
            r2_2: Regex::new(r"R2_SECRET_ACCESS_KEY=[^\s&]+")?,
            r2_3: Regex::new(r"'aws_access_key_id': '[^']*'")?,
            r2_4: Regex::new(r"'aws_secret_access_key': '[^']*'")?,
        })
    }

    fn redact(&self, input: &str) -> String {
        let mut output = input.to_string();
        output = self
            .token1
            .replace_all(&output, "$1[TELEGRAM_TOKEN]$3")
            .to_string();
        output = self
            .token2
            .replace_all(&output, "[TELEGRAM_TOKEN]")
            .to_string();
        output = self
            .token3
            .replace_all(&output, "$1[TELEGRAM_TOKEN]")
            .to_string();
        output = self
            .r2_1
            .replace_all(&output, "R2_ACCESS_KEY_ID=[MASKED]")
            .to_string();
        output = self
            .r2_2
            .replace_all(&output, "R2_SECRET_ACCESS_KEY=[MASKED]")
            .to_string();
        output = self
            .r2_3
            .replace_all(&output, "'aws_access_key_id': '[MASKED]'")
            .to_string();
        output = self
            .r2_4
            .replace_all(&output, "'aws_secret_access_key': '[MASKED]'")
            .to_string();
        output
    }
}

struct RedactingWriter<W: Write> {
    inner: W,
    patterns: Arc<RedactionPatterns>,
}

impl<W: Write> RedactingWriter<W> {
    const fn new(inner: W, patterns: Arc<RedactionPatterns>) -> Self {
        Self { inner, patterns }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        let redacted = self.patterns.redact(&s);
        self.inner.write_all(redacted.as_bytes())?;
        // We return the original buffer length to satisfy the contract,
        // even if the redacted string length differs.
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct RedactingMakeWriter<F> {
    make_inner: F,
    patterns: Arc<RedactionPatterns>,
}

impl<F> RedactingMakeWriter<F> {
    const fn new(make_inner: F, patterns: Arc<RedactionPatterns>) -> Self {
        Self {
            make_inner,
            patterns,
        }
    }
}

impl<'a, F, W> tracing_subscriber::fmt::MakeWriter<'a> for RedactingMakeWriter<F>
where
    F: Fn() -> W + 'static,
    W: Write,
{
    type Writer = RedactingWriter<W>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new((self.make_inner)(), self.patterns.clone())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StartupCommand {
    RunBot,
    PrintCompiledCapabilitiesJson,
    PrintEnabledCapabilitiesJson { config_path: Option<String> },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    match parse_startup_command(env::args().skip(1))? {
        StartupCommand::RunBot => {}
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
    }

    // Load .env file
    dotenv().ok();

    // Initialize redaction patterns early (before logging)
    let patterns = Arc::new(RedactionPatterns::new().map_err(|e| {
        eprintln!("Failed to compile regex patterns: {e}");
        e
    })?);

    // Setup logging with redaction
    init_logging(patterns);

    match cleanup_stale_private_key_tempfiles() {
        Ok(removed) if removed > 0 => {
            info!(removed, "Removed stale SSH private key temp files");
        }
        Ok(_) => {}
        Err(error) => {
            warn!(%error, "Failed to clean up stale SSH private key temp files");
        }
    }

    info!("Starting Oxide Agent TG Bot...");

    // Load settings
    let settings = init_settings();

    run_bot(settings).await;

    Ok(())
}

fn parse_startup_command<I, S>(args: I) -> io::Result<StartupCommand>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(StartupCommand::RunBot);
    };
    if first.as_ref() != "capabilities" {
        return Ok(StartupCommand::RunBot);
    }

    let mut mode = None;
    let mut config_path = None;
    let mut json = false;
    while let Some(arg) = args.next() {
        match arg.as_ref() {
            "--compiled" => set_capability_mode(&mut mode, CapabilityMode::Compiled)?,
            "--enabled" => set_capability_mode(&mut mode, CapabilityMode::Enabled)?,
            "--config" => {
                let Some(path) = args.next() else {
                    return Err(capabilities_usage_error());
                };
                config_path = Some(path.as_ref().to_string());
            }
            "--json" => {
                json = true;
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
        _ => Err(capabilities_usage_error()),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CapabilityMode {
    Compiled,
    Enabled,
}

fn set_capability_mode(
    mode: &mut Option<CapabilityMode>,
    requested: CapabilityMode,
) -> io::Result<()> {
    match mode {
        None => {
            *mode = Some(requested);
            Ok(())
        }
        Some(existing) if *existing == requested => Ok(()),
        Some(_) => Err(capabilities_usage_error()),
    }
}

fn capabilities_usage_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "Usage: oxide-agent-telegram-bot capabilities (--compiled | --enabled [--config PATH]) --json",
    )
}

fn init_logging(patterns: Arc<RedactionPatterns>) {
    let make_writer = RedactingMakeWriter::new(io::stderr, patterns);

    // Проверка переменной DEBUG_MODE для verbose режима
    let debug_mode = std::env::var("DEBUG_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let filter = if debug_mode {
        // Verbose режим: всё на уровне debug
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"))
    } else {
        // Production режим: фильтрованные настройки
        EnvFilter::try_from_default_env().unwrap_or_else(|_| {
            EnvFilter::new(
                "oxide_agent_core=info,oxide_agent_transport_telegram=info,zai_rs=debug,hyper=warn,h2=error,reqwest=warn,tokio=warn,tower=warn,async_openai=warn",
            )
        })
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(make_writer))
        .init();
}

fn init_settings() -> Arc<BotSettings> {
    let agent_settings = match AgentSettings::new() {
        Ok(settings) => settings,
        Err(e) => {
            error!("Failed to load agent configuration: {}", e);
            std::process::exit(1);
        }
    };
    let telegram_settings = match TelegramSettings::new() {
        Ok(settings) => settings,
        Err(e) => {
            error!("Failed to load telegram configuration: {}", e);
            std::process::exit(1);
        }
    };

    info!("Configuration loaded successfully.");
    Arc::new(BotSettings::new(agent_settings, telegram_settings))
}

#[cfg(test)]
mod tests {
    use super::{parse_startup_command, StartupCommand};
    use std::io;

    #[test]
    fn startup_command_defaults_to_bot() {
        let command =
            parse_startup_command(std::iter::empty::<&str>()).expect("empty args should run bot");

        assert_eq!(command, StartupCommand::RunBot);
    }

    #[test]
    fn startup_command_parses_compiled_capabilities_json() {
        let command = parse_startup_command(["capabilities", "--compiled", "--json"])
            .expect("capabilities command should parse");

        assert_eq!(command, StartupCommand::PrintCompiledCapabilitiesJson);
    }

    #[test]
    fn startup_command_parses_enabled_capabilities_json_with_config() {
        let command = parse_startup_command([
            "capabilities",
            "--enabled",
            "--config",
            "config/test-profile.yaml",
            "--json",
        ])
        .expect("enabled capabilities command should parse");

        assert_eq!(
            command,
            StartupCommand::PrintEnabledCapabilitiesJson {
                config_path: Some("config/test-profile.yaml".to_string())
            }
        );
    }

    #[test]
    fn startup_command_rejects_partial_capabilities_command() {
        let error = parse_startup_command(["capabilities", "--compiled"])
            .expect_err("partial capabilities command should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn startup_command_rejects_mixed_capability_modes() {
        let error = parse_startup_command(["capabilities", "--compiled", "--enabled", "--json"])
            .expect_err("mixed capability modes should fail");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
}
