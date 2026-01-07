use another_chat_rs::bot::UnauthorizedCache;
use another_chat_rs::config::{
    get_unauthorized_cache_max_size, get_unauthorized_cache_ttl, get_unauthorized_cooldown,
    Settings,
};
use another_chat_rs::{bot, llm, storage};
use bot::handlers::{get_user_id_safe, Command};
use bot::state::State;
use dotenvy::dotenv;
use regex::Regex;
use std::io::{self, Write};
use std::sync::Arc;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::UpdateHandler;
use teloxide::prelude::*;
use teloxide::types::CallbackQuery;
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

    info!("Starting Another Chat TG Bot (Rust port)...");

    // Load settings
    let settings = init_settings();

    // Initialize storage
    let storage = init_storage(&settings).await;

    // Initialize LLM Client
    let llm_client = Arc::new(llm::LlmClient::new(&settings));
    info!("LLM Client initialized.");

    // Initialize Bot
    let bot = Bot::new(settings.telegram_token.clone());

    // Initialize bot state
    let bot_state = init_bot_state();

    // Initialize unauthorized cache
    let unauthorized_cache = init_unauthorized_cache();

    // Setup handlers
    let handler = setup_handler();

    info!("Bot is running...");

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![
            storage,
            llm_client,
            settings,
            bot_state,
            unauthorized_cache
        ])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

fn init_logging(patterns: Arc<RedactionPatterns>) {
    let make_writer = RedactingMakeWriter::new(io::stderr, patterns);
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(make_writer))
        .init();
}

fn init_settings() -> Arc<Settings> {
    match Settings::new() {
        Ok(s) => {
            info!("Configuration loaded successfully.");
            Arc::new(s)
        }
        Err(e) => {
            error!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    }
}

async fn init_storage(settings: &Settings) -> Arc<storage::R2Storage> {
    match storage::R2Storage::new(settings).await {
        Ok(s) => {
            info!("R2 Storage initialized.");
            if s.check_connection().await.is_ok() {
                // Success message already logged in check_connection
            } else {
                error!("R2 Storage connection check returned error.");
            }
            Arc::new(s)
        }
        Err(e) => {
            error!("Failed to initialize R2 Storage: {}", e);
            std::process::exit(1);
        }
    }
}

fn init_bot_state() -> Arc<InMemStorage<State>> {
    InMemStorage::<State>::new()
}

fn init_unauthorized_cache() -> Arc<UnauthorizedCache> {
    let cooldown = get_unauthorized_cooldown();
    let ttl = get_unauthorized_cache_ttl();
    let max_size = get_unauthorized_cache_max_size();

    info!(
        "Initializing UnauthorizedCache (cooldown: {}s, ttl: {}s, max_size: {})",
        cooldown, ttl, max_size
    );

    Arc::new(UnauthorizedCache::new(cooldown, ttl, max_size))
}

fn setup_handler() -> UpdateHandler<teloxide::RequestError> {
    dptree::entry()
        .branch(
            Update::filter_callback_query()
                .filter(|q: CallbackQuery, settings: Arc<Settings>| {
                    settings
                        .allowed_users()
                        .contains(&q.from.id.0.cast_signed())
                })
                .endpoint(handle_loop_callback),
        )
        .branch(
            Update::filter_message().branch(
                // Основная ветка для авторизованных пользователей
                dptree::filter(|msg: Message, settings: Arc<Settings>| {
                    settings.allowed_users().contains(&get_user_id_safe(&msg))
                })
                .enter_dialogue::<Message, InMemStorage<State>, State>()
                .branch(
                    dptree::entry()
                        .filter_command::<Command>()
                        .endpoint(handle_command),
                )
                .branch(
                    dptree::case![State::Start]
                        .branch(
                            Update::filter_message()
                                .filter(|msg: Message| msg.text().is_some())
                                .endpoint(handle_start_text),
                        )
                        .branch(
                            Update::filter_message()
                                .filter(|msg: Message| msg.voice().is_some())
                                .endpoint(handle_start_voice),
                        )
                        .branch(
                            Update::filter_message()
                                .filter(|msg: Message| msg.photo().is_some())
                                .endpoint(handle_start_photo),
                        )
                        .branch(
                            dptree::filter(|msg: Message| msg.document().is_some())
                                .endpoint(handle_start_document),
                        ),
                )
                .branch(dptree::case![State::EditingPrompt].endpoint(handle_editing_prompt))
                .branch(dptree::case![State::AgentMode].endpoint(handle_agent_message))
                .branch(
                    dptree::case![State::AgentWipeConfirmation]
                        .endpoint(handle_agent_wipe_confirmation),
                ),
            ),
        )
        .branch(
            // Все, кто не попал в фильтр выше — неавторизованы
            Update::filter_message().endpoint(handle_unauthorized),
        )
}

async fn handle_unauthorized(
    bot: Bot,
    msg: Message,
    cache: Arc<UnauthorizedCache>,
) -> Result<(), teloxide::RequestError> {
    let user_id = get_user_id_safe(&msg);
    let user_name = msg
        .from
        .as_ref()
        .map(|u| u.first_name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    // Check if we should send a message (cooldown period passed or first attempt)
    if cache.should_send(user_id, &user_name).await {
        info!(
            "⛔️ Unauthorized access from user {} ({}). Sending denial message.",
            user_id, user_name
        );

        if let Err(e) = bot.send_message(msg.chat.id, "⛔️ Access denied").await {
            error!("Failed to send access denied message to {}: {}", user_id, e);
        } else {
            // Mark that message was sent successfully
            cache.mark_sent(user_id).await;
        }
    }
    // Note: Silenced attempts are logged inside cache.should_send() with throttling

    respond(())
}

async fn handle_command(
    bot: Bot,
    msg: Message,
    cmd: Command,
    storage: Arc<storage::R2Storage>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    cache: Arc<UnauthorizedCache>,
) -> Result<(), teloxide::RequestError> {
    let res = match cmd {
        Command::Start => bot::handlers::start(bot, msg, storage, dialogue).await,
        Command::Clear => bot::handlers::clear(bot, msg, storage).await,
        Command::Healthcheck => bot::handlers::healthcheck(bot, msg).await,
        Command::Stats => bot::handlers::stats(bot, msg, cache).await,
    };
    if let Err(e) = res {
        error!("Command error: {}", e);
    }
    respond(())
}

async fn handle_start_text(
    bot: Bot,
    msg: Message,
    storage: Arc<storage::R2Storage>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    settings: Arc<Settings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::handlers::handle_text(
        bot, msg, storage, llm, dialogue, settings,
    ))
    .await
    {
        error!("Text handler error: {}", e);
    }
    respond(())
}

async fn handle_start_voice(
    bot: Bot,
    msg: Message,
    storage: Arc<storage::R2Storage>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::handlers::handle_voice(
        bot, msg, storage, llm, dialogue,
    ))
    .await
    {
        error!("Voice handler error: {}", e);
    }
    respond(())
}

async fn handle_start_photo(
    bot: Bot,
    msg: Message,
    storage: Arc<storage::R2Storage>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_photo(bot, msg, storage, llm, dialogue).await {
        error!("Photo handler error: {}", e);
    }
    respond(())
}

async fn handle_start_document(
    bot: Bot,
    msg: Message,
    storage: Arc<storage::R2Storage>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_document(bot, msg, dialogue, storage, llm).await {
        error!("Document handler error: {}", e);
    }
    respond(())
}

async fn handle_editing_prompt(
    bot: Bot,
    msg: Message,
    storage: Arc<storage::R2Storage>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_editing_prompt(bot, msg, storage, dialogue).await {
        error!("Editing prompt handler error: {}", e);
    }
    respond(())
}

async fn handle_agent_message(
    bot: Bot,
    msg: Message,
    storage: Arc<storage::R2Storage>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::agent_handlers::handle_agent_message(
        bot, msg, storage, llm, dialogue,
    ))
    .await
    {
        error!("Agent mode handler error: {}", e);
    }
    respond(())
}

async fn handle_loop_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<storage::R2Storage>,
    llm: Arc<llm::LlmClient>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::agent_handlers::handle_loop_callback(bot, q, storage, llm).await {
        error!("Loop callback handler error: {}", e);
    }
    respond(())
}

async fn handle_agent_wipe_confirmation(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::agent_handlers::handle_agent_wipe_confirmation(bot, msg, dialogue).await {
        error!("Agent wipe confirmation handler error: {}", e);
    }
    respond(())
}
