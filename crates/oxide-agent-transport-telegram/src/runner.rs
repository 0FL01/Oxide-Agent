use crate::bot;
use crate::bot::handlers::Command;
use crate::bot::state::State;
use crate::bot::UnauthorizedCache;
use crate::config::{
    get_unauthorized_cache_max_size, get_unauthorized_cache_ttl, get_unauthorized_cooldown,
    BotSettings,
};
use crate::startup_maintenance::run_startup_tool_drift_prune;
use oxide_agent_core::agent::{connect_postgres_memory_store, PersistentMemoryStore};
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_core::{llm, storage};
use std::sync::Arc;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::UpdateHandler;
use teloxide::prelude::*;
use teloxide::types::{CallbackQuery, Message, User};
use tracing::{error, info};

/// Run the Telegram transport runtime.
pub async fn run_bot(settings: Arc<BotSettings>) {
    let storage = init_storage(&settings).await;
    let persistent_memory_store = init_persistent_memory_store(&settings).await;
    let llm_client = Arc::new(llm::LlmClient::new(settings.agent.as_ref()));
    info!("LLM Client initialized.");

    if let Err(error) = run_startup_tool_drift_prune(
        Arc::clone(&storage),
        Arc::clone(&persistent_memory_store),
        Arc::clone(&llm_client),
        Arc::clone(&settings),
    )
    .await
    {
        error!(%error, "Startup tool drift prune failed");
    }

    let storage: Arc<dyn storage::StorageProvider> = storage;

    let bot = Bot::new(settings.telegram.telegram_token.clone());
    bot::agent_handlers::spawn_reminder_scheduler(
        bot.clone(),
        storage.clone(),
        llm_client.clone(),
        persistent_memory_store.clone(),
        settings.clone(),
    );
    let bot_state = init_bot_state();
    let unauthorized_cache = init_unauthorized_cache();
    let handler = setup_handler();

    info!("Bot is running...");

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![
            storage,
            persistent_memory_store,
            llm_client,
            settings,
            bot_state,
            unauthorized_cache
        ])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

async fn init_storage(settings: &BotSettings) -> Arc<storage::R2Storage> {
    match storage::R2Storage::new(settings.agent.as_ref()).await {
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

async fn init_persistent_memory_store(settings: &BotSettings) -> Arc<dyn PersistentMemoryStore> {
    match connect_postgres_memory_store(settings.agent.as_ref()).await {
        Ok(store) => {
            info!("Postgres persistent memory initialized.");
            store
        }
        Err(error) => {
            error!(%error, "Failed to initialize Postgres persistent memory");
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
                .filter(|q: CallbackQuery, settings: Arc<BotSettings>| {
                    settings
                        .telegram
                        .allowed_users()
                        .contains(&q.from.id.0.cast_signed())
                })
                .endpoint(handle_callback),
        )
        .branch(
            Update::filter_message().branch(
                // Main branch for authorized users
                dptree::filter(|msg: Message, settings: Arc<BotSettings>| {
                    access_control_user_id(&msg)
                        .is_some_and(|user_id| settings.telegram.allowed_users().contains(&user_id))
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
                            Update::filter_message()
                                .filter(|msg: Message| msg.video().is_some())
                                .endpoint(handle_start_video),
                        )
                        .branch(
                            dptree::filter(|msg: Message| msg.document().is_some())
                                .endpoint(handle_start_document),
                        ),
                )
                .branch(
                    dptree::case![State::ChatMode]
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
                            Update::filter_message()
                                .filter(|msg: Message| msg.video().is_some())
                                .endpoint(handle_start_video),
                        )
                        .branch(
                            dptree::filter(|msg: Message| msg.document().is_some())
                                .endpoint(handle_start_document),
                        ),
                )
                .branch(dptree::case![State::EditingPrompt].endpoint(handle_editing_prompt))
                .branch(dptree::case![State::AgentMode].endpoint(handle_agent_message))
                .branch(
                    dptree::case![State::AgentConfirmation(action)]
                        .endpoint(handle_agent_confirmation),
                ),
            ),
        )
        .branch(
            // All who are not in the filter above — unauthorized
            Update::filter_message()
                .filter(|msg: Message| access_control_user_id(&msg).is_some())
                .endpoint(handle_unauthorized),
        )
}

fn access_control_user(message: &Message) -> Option<&User> {
    message.from.as_ref().filter(|user| !user.is_bot)
}

fn access_control_user_id(message: &Message) -> Option<i64> {
    access_control_user(message).map(|user| user.id.0.cast_signed())
}

async fn handle_unauthorized(
    bot: Bot,
    msg: Message,
    cache: Arc<UnauthorizedCache>,
) -> Result<(), teloxide::RequestError> {
    let Some(user) = access_control_user(&msg) else {
        return respond(());
    };
    let user_id = user.id.0.cast_signed();
    let user_name = user.first_name.clone();

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
    storage: Arc<dyn storage::StorageProvider>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    cache: Arc<UnauthorizedCache>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    let res = match cmd {
        Command::Start => bot::handlers::start(bot, msg, storage, settings, dialogue).await,
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
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::handlers::handle_text(
        bot,
        msg,
        storage,
        llm,
        dialogue,
        persistent_memory_store,
        settings,
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
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::handlers::handle_voice(
        bot,
        msg,
        storage,
        llm,
        dialogue,
        persistent_memory_store,
        settings,
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
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_photo(
        bot,
        msg,
        storage,
        llm,
        dialogue,
        persistent_memory_store,
        settings,
    )
    .await
    {
        error!("Photo handler error: {}", e);
    }
    respond(())
}

async fn handle_start_video(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_video(
        bot,
        msg,
        storage,
        llm,
        dialogue,
        persistent_memory_store,
        settings,
    )
    .await
    {
        error!("Video handler error: {}", e);
    }
    respond(())
}

async fn handle_start_document(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_document(
        bot,
        msg,
        dialogue,
        storage,
        llm,
        persistent_memory_store,
        settings,
    )
    .await
    {
        error!("Document handler error: {}", e);
    }
    respond(())
}

async fn handle_editing_prompt(
    bot: Bot,
    msg: Message,
    storage: Arc<dyn storage::StorageProvider>,
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
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    dialogue: Dialogue<State, InMemStorage<State>>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::agent_handlers::handle_agent_message(
        bot,
        msg,
        storage,
        llm,
        dialogue,
        persistent_memory_store,
        settings,
    ))
    .await
    {
        error!("Agent mode handler error: {}", e);
    }
    respond(())
}

async fn handle_callback(
    bot: Bot,
    q: CallbackQuery,
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
    bot_state: Arc<InMemStorage<State>>,
) -> Result<(), teloxide::RequestError> {
    let dialogue = q
        .message
        .as_ref()
        .map(|message| Dialogue::new(bot_state.clone(), message.chat().id));

    match bot::handlers::handle_chat_flow_callback(&bot, &q, &storage).await {
        Ok(true) => {
            return respond(());
        }
        Ok(false) => {}
        Err(e) => {
            error!("Chat flow callback handler error: {}", e);
            return respond(());
        }
    }

    if let Some(dialogue) = &dialogue {
        match bot::handlers::handle_menu_callback(
            &bot,
            &q,
            &storage,
            &llm,
            &persistent_memory_store,
            &settings,
            dialogue,
        )
        .await
        {
            Ok(true) => {
                return respond(());
            }
            Ok(false) => {}
            Err(e) => {
                error!("Menu callback handler error: {}", e);
                return respond(());
            }
        }
    }

    if !settings
        .telegram
        .agent_allowed_users()
        .contains(&q.from.id.0.cast_signed())
    {
        return respond(());
    }

    let Some(dialogue) = dialogue else {
        return respond(());
    };

    if let Err(e) = bot::agent_handlers::handle_agent_callback(
        bot,
        q,
        storage,
        llm,
        persistent_memory_store,
        settings,
        dialogue,
    )
    .await
    {
        error!("Agent callback handler error: {}", e);
    }
    respond(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_agent_confirmation(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    action: bot::state::ConfirmationType,
    storage: Arc<dyn storage::StorageProvider>,
    llm: Arc<llm::LlmClient>,
    persistent_memory_store: Arc<dyn oxide_agent_core::agent::PersistentMemoryStore>,
    settings: Arc<BotSettings>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::agent_handlers::handle_agent_confirmation(
        bot,
        msg,
        dialogue,
        action,
        storage,
        llm,
        persistent_memory_store,
        settings,
    )
    .await
    {
        error!("Agent confirmation handler error: {}", e);
    }
    respond(())
}

#[cfg(test)]
mod tests {
    use super::access_control_user_id;
    use crate::bot::handlers::get_user_id_safe;
    use teloxide::types::{
        Chat, ChatId, ChatKind, ChatPrivate, MediaKind, MediaText, Message, MessageCommon,
        MessageId, MessageKind, User, UserId,
    };

    fn text_message(from: Option<User>) -> Message {
        Message {
            id: MessageId(1),
            thread_id: None,
            from,
            sender_chat: None,
            date: std::time::SystemTime::UNIX_EPOCH.into(),
            chat: Chat {
                id: ChatId(42),
                kind: ChatKind::Private(ChatPrivate {
                    username: None,
                    first_name: Some("chat".to_string()),
                    last_name: None,
                }),
            },
            is_topic_message: false,
            via_bot: None,
            sender_business_bot: None,
            kind: MessageKind::Common(MessageCommon {
                author_signature: None,
                paid_star_count: None,
                effect_id: None,
                forward_origin: None,
                reply_to_message: None,
                external_reply: None,
                quote: None,
                reply_to_story: None,
                sender_boost_count: None,
                edit_date: None,
                media_kind: MediaKind::Text(MediaText {
                    text: "hello".to_string(),
                    entities: Vec::new(),
                    link_preview_options: None,
                }),
                reply_markup: None,
                is_automatic_forward: false,
                has_protected_content: false,
                is_from_offline: false,
                business_connection_id: None,
            }),
        }
    }

    #[test]
    fn access_control_accepts_human_messages() {
        let message = text_message(Some(User {
            id: UserId(77),
            is_bot: false,
            first_name: "Alice".to_string(),
            last_name: None,
            username: None,
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        }));

        assert_eq!(access_control_user_id(&message), Some(77));
        assert_eq!(get_user_id_safe(&message), 77);
    }

    #[test]
    fn access_control_ignores_bot_authored_messages() {
        let message = text_message(Some(User {
            id: UserId(999),
            is_bot: true,
            first_name: "oxide-bot".to_string(),
            last_name: None,
            username: Some("oxide_bot".to_string()),
            language_code: None,
            is_premium: false,
            added_to_attachment_menu: false,
        }));

        assert_eq!(access_control_user_id(&message), None);
        assert_eq!(get_user_id_safe(&message), 999);
    }

    #[test]
    fn access_control_ignores_messages_without_user_sender() {
        let message = text_message(None);

        assert_eq!(access_control_user_id(&message), None);
        assert_eq!(get_user_id_safe(&message), 0);
    }
}
