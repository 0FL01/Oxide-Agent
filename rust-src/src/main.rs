mod config;
mod storage;
pub mod llm;
mod utils;
mod bot;

use config::Settings;
use dotenvy::dotenv;
use tracing::{info, error};
use tracing_subscriber::{prelude::*, EnvFilter};
use regex::Regex;
use lazy_static::lazy_static;
use std::io::{self, Write};

lazy_static! {
    static ref RE_TOKEN1: Regex = Regex::new(r"(https?://[^/]+/bot)([0-9]+:[A-Za-z0-9_-]+)(/[^'\s]*)").unwrap();
    static ref RE_TOKEN2: Regex = Regex::new(r"([0-9]{8,10}:[A-Za-z0-9_-]{35})").unwrap();
    static ref RE_TOKEN3: Regex = Regex::new(r"(bot[0-9]{8,10}:)[A-Za-z0-9_-]+").unwrap();
    static ref RE_R2_1: Regex = Regex::new(r"R2_ACCESS_KEY_ID=[^\s&]+").unwrap();
    static ref RE_R2_2: Regex = Regex::new(r"R2_SECRET_ACCESS_KEY=[^\s&]+").unwrap();
    static ref RE_R2_3: Regex = Regex::new(r"'aws_access_key_id': '[^']*'").unwrap();
    static ref RE_R2_4: Regex = Regex::new(r"'aws_secret_access_key': '[^']*'").unwrap();
}

fn redact(input: &str) -> String {
    let mut output = input.to_string();
    output = RE_TOKEN1.replace_all(&output, "$1[TELEGRAM_TOKEN]$3").to_string();
    output = RE_TOKEN2.replace_all(&output, "[TELEGRAM_TOKEN]").to_string();
    output = RE_TOKEN3.replace_all(&output, "$1[TELEGRAM_TOKEN]").to_string();
    output = RE_R2_1.replace_all(&output, "R2_ACCESS_KEY_ID=[MASKED]").to_string();
    output = RE_R2_2.replace_all(&output, "R2_SECRET_ACCESS_KEY=[MASKED]").to_string();
    output = RE_R2_3.replace_all(&output, "'aws_access_key_id': '[MASKED]'").to_string();
    output = RE_R2_4.replace_all(&output, "'aws_secret_access_key': '[MASKED]'").to_string();
    output
}

struct RedactingWriter<W: Write> {
    inner: W,
}

impl<W: Write> RedactingWriter<W> {
    fn new(inner: W) -> Self {
        Self { inner }
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let s = String::from_utf8_lossy(buf);
        let redacted = redact(&s);
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
}

impl<F> RedactingMakeWriter<F> {
    fn new(make_inner: F) -> Self {
        Self { make_inner }
    }
}

impl<'a, F, W> tracing_subscriber::fmt::MakeWriter<'a> for RedactingMakeWriter<F>
where
    F: Fn() -> W + 'static,
    W: Write,
{
    type Writer = RedactingWriter<W>;

    fn make_writer(&'a self) -> Self::Writer {
        RedactingWriter::new((self.make_inner)())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Load .env file
    dotenv().ok();

    // Setup logging with redaction
    let make_writer = RedactingMakeWriter::new(io::stderr);
    
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(make_writer))
        .init();

    info!("Starting Another Chat TG Bot (Rust port)...");

    // Load settings
    match Settings::new() {
        Ok(settings) => {
            info!("Configuration loaded successfully.");
            info!("Allowed users count: {}", settings.allowed_users().len());
            
            // Test redaction
            info!("Testing token redaction: bot12345678:ABC-DEF1234ghIkl-zyx57W2v1u123ew11");

            // Initialize storage
            match storage::R2Storage::new(&settings).await {
                Ok(storage) => {
                    info!("R2 Storage initialized.");
                    if storage.check_connection().await {
                        info!("R2 Storage connection verified.");
                    } else {
                        error!("R2 Storage connection failed.");
                    }
                }
                Err(e) => {
                    error!("Failed to initialize R2 Storage: {}", e);
                }
            }
        }
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    }

    Ok(())
}
