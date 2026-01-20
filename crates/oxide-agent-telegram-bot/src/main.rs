use dotenvy::dotenv;
use oxide_agent_core::config::AgentSettings;
use oxide_agent_transport_telegram::config::{BotSettings, TelegramSettings};
use oxide_agent_transport_telegram::runner::run_bot;
use regex::Regex;
use std::io::{self, Write};
use std::sync::Arc;
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

    run_bot(settings).await;

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
