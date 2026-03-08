use dotenvy::dotenv;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_runtime::{ObserverAccessRegistry, ObserverAccessRegistryOptions};
use oxide_agent_transport_telegram::config::{BotSettings, TelegramSettings};
use oxide_agent_transport_telegram::runner::TelegramRuntime;
use oxide_agent_transport_web::{
    spawn_web_monitor, WebMonitorServerHandle, WebMonitorServerOptions,
};
use regex::Regex;
use std::fmt::Display;
use std::future::Future;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenv().ok();

    // Initialize redaction patterns early (before logging)
    let patterns = Arc::new(RedactionPatterns::new().map_err(|e| {
        eprintln!("Failed to compile regex patterns: {e}");
        e
    })?);

    // Setup logging with redaction
    init_logging(patterns);

    info!("Starting Oxide Agent TG Bot...");

    // Load settings
    let settings = init_settings();
    let observer_access = build_observer_access_registry(&settings);
    let web_observer_ready = Arc::new(AtomicBool::new(false));

    let runtime = TelegramRuntime::build_with_observer_access(
        Arc::clone(&settings),
        observer_access.clone(),
        Arc::clone(&web_observer_ready),
    )
    .await?;
    let _web_monitor = maybe_spawn_web_monitor(
        Arc::clone(&settings),
        &runtime,
        observer_access,
        &web_observer_ready,
    )
    .await;
    runtime.run().await?;

    Ok(())
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

async fn maybe_spawn_web_monitor(
    settings: Arc<BotSettings>,
    runtime: &TelegramRuntime,
    observer_access: Option<Arc<ObserverAccessRegistry>>,
    web_observer_ready: &AtomicBool,
) -> Option<WebMonitorServerHandle> {
    if !settings.agent.is_web_observer_enabled() {
        return None;
    }

    let bind_addr = match parse_web_bind_addr(settings.agent.web_observer_bind_addr.as_deref()) {
        Ok(value) => value,
        Err(error) => {
            error!(error = %error, "Web observer monitor disabled: invalid bind address");
            return None;
        }
    };
    let observer_access = observer_access?;

    let server = run_optional_web_monitor_startup(bind_addr, web_observer_ready, || async {
        spawn_web_monitor(WebMonitorServerOptions {
            bind_addr,
            storage: Arc::clone(&runtime.services().storage),
            task_events: Arc::clone(&runtime.services().task_events),
            observer_access,
        })
        .await
    })
    .await;

    let server = server?;

    info!(
        addr = %server.local_addr(),
        "Web observer monitor transport enabled"
    );

    Some(server)
}

fn build_observer_access_registry(settings: &BotSettings) -> Option<Arc<ObserverAccessRegistry>> {
    if !settings.agent.is_web_observer_enabled() {
        return None;
    }

    Some(Arc::new(ObserverAccessRegistry::new(
        ObserverAccessRegistryOptions {
            token_ttl: Duration::from_secs(settings.agent.get_web_observer_token_ttl_secs()),
        },
    )))
}

async fn run_optional_web_monitor_startup<T, E, F, Fut>(
    bind_addr: SocketAddr,
    web_observer_ready: &AtomicBool,
    start: F,
) -> Option<T>
where
    E: Display,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    match start().await {
        Ok(server) => {
            web_observer_ready.store(true, Ordering::Relaxed);
            Some(server)
        }
        Err(error) => {
            web_observer_ready.store(false, Ordering::Relaxed);
            error!(
                addr = %bind_addr,
                error = %error,
                "Web observer monitor failed to start; continuing without web monitor"
            );
            None
        }
    }
}

fn parse_web_bind_addr(raw: Option<&str>) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let default_addr = "127.0.0.1:8090";
    let value = raw.unwrap_or(default_addr);
    let parsed = value.parse::<SocketAddr>()?;
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::{parse_web_bind_addr, run_optional_web_monitor_startup};
    use std::io;
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn optional_web_monitor_startup_returns_none_on_start_error() {
        let bind_addr: SocketAddr = match "127.0.0.1:8090".parse() {
            Ok(value) => value,
            Err(error) => panic!("failed to parse test address: {error}"),
        };
        let web_observer_ready = AtomicBool::new(true);

        let result = run_optional_web_monitor_startup::<(), io::Error, _, _>(
            bind_addr,
            &web_observer_ready,
            || async { Err(io::Error::other("bind failed")) },
        )
        .await;

        assert!(result.is_none());
        assert!(!web_observer_ready.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn optional_web_monitor_startup_returns_server_on_success() {
        let bind_addr: SocketAddr = match "127.0.0.1:8091".parse() {
            Ok(value) => value,
            Err(error) => panic!("failed to parse test address: {error}"),
        };
        let web_observer_ready = AtomicBool::new(false);

        let result = run_optional_web_monitor_startup::<u8, io::Error, _, _>(
            bind_addr,
            &web_observer_ready,
            || async { Ok(1) },
        )
        .await;

        assert_eq!(result, Some(1));
        assert!(web_observer_ready.load(Ordering::Relaxed));
    }

    #[test]
    fn parse_web_bind_addr_rejects_invalid_value() {
        let parsed = parse_web_bind_addr(Some("not-an-address"));
        assert!(parsed.is_err());
    }
}
