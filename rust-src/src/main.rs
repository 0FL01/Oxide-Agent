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
    let settings = match Settings::new() {
        Ok(s) => {
            info!("Configuration loaded successfully.");
            s
        }
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize storage
    let storage = match storage::R2Storage::new(&settings).await {
        Ok(s) => {
            info!("R2 Storage initialized.");
            if s.check_connection().await {
                info!("R2 Storage connection verified.");
            } else {
                error!("R2 Storage connection failed.");
            }
            std::sync::Arc::new(s)
        }
        Err(e) => {
            error!("Failed to initialize R2 Storage: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize LLM Client
    let llm_client = std::sync::Arc::new(llm::LlmClient::new(&settings));
    info!("LLM Client initialized.");

    // Initialize Bot
    let bot = teloxide::Bot::from_env();
    
    // Authorization filter
    let allowed_users = settings.allowed_users().clone();
    let auth_filter = move |msg: teloxide::types::Message| {
        let user_id = msg.from.as_ref().map(|u| u.id.0 as i64).unwrap_or(0);
        allowed_users.contains(&user_id)
    };

    use teloxide::dispatching::dialogue::InMemStorage;
    use teloxide::prelude::*;
    use bot::handlers::Command;
    use bot::state::State;

    let handler = Update::filter_message()
        .filter(auth_filter)
        .enter_dialogue::<Message, InMemStorage<State>, State>()
        .branch(
            dptree::entry()
                .filter_command::<Command>()
                .endpoint(move |bot: Bot, msg: Message, cmd: Command, storage: std::sync::Arc<storage::R2Storage>| async move {
                    let res = match cmd {
                        Command::Start => bot::handlers::start(bot, msg, storage).await,
                        Command::Clear => bot::handlers::clear(bot, msg, storage).await,
                        Command::Healthcheck => bot::handlers::healthcheck(bot, msg).await,
                    };
                    if let Err(e) = res {
                        error!("Command error: {}", e);
                    }
                    respond(())
                })
        )
        .branch(
            dptree::case![State::Start]
                .branch(Update::filter_message().filter(|msg: Message| msg.text().is_some()).endpoint(|bot: Bot, msg: Message, storage: std::sync::Arc<storage::R2Storage>, llm: std::sync::Arc<llm::LlmClient>, dialogue: Dialogue<State, InMemStorage<State>>| async move {
                    if let Err(e) = bot::handlers::handle_text(bot, msg, storage, llm, dialogue).await {
                        error!("Text handler error: {}", e);
                    }
                    respond(())
                }))
                .branch(Update::filter_message().filter(|msg: Message| msg.voice().is_some()).endpoint(|bot: Bot, msg: Message, storage: std::sync::Arc<storage::R2Storage>, llm: std::sync::Arc<llm::LlmClient>| async move {
                    if let Err(e) = bot::handlers::handle_voice(bot, msg, storage, llm).await {
                        error!("Voice handler error: {}", e);
                    }
                    respond(())
                }))
                .branch(Update::filter_message().filter(|msg: Message| msg.photo().is_some()).endpoint(|bot: Bot, msg: Message, storage: std::sync::Arc<storage::R2Storage>, llm: std::sync::Arc<llm::LlmClient>| async move {
                    if let Err(e) = bot::handlers::handle_photo(bot, msg, storage, llm).await {
                        error!("Photo handler error: {}", e);
                    }
                    respond(())
                }))
        )
        .branch(
            dptree::case![State::EditingPrompt]
                .endpoint(|bot: Bot, msg: Message, storage: std::sync::Arc<storage::R2Storage>, dialogue: Dialogue<State, InMemStorage<State>>| async move {
                    if let Err(e) = bot::handlers::handle_editing_prompt(bot, msg, storage, dialogue).await {
                        error!("Editing prompt handler error: {}", e);
                    }
                    respond(())
                })
        );

    info!("Bot is running...");

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![storage, llm_client, InMemStorage::<State>::new()])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}
