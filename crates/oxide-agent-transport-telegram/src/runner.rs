use crate::bot;
use crate::bot::agent_handlers::AgentTaskRuntime;
use crate::bot::context::TelegramHandlerContext;
use crate::bot::handlers::{get_user_id_safe, Command};
use crate::bot::state::State;
use crate::bot::UnauthorizedCache;
use crate::config::{
    get_unauthorized_cache_max_size, get_unauthorized_cache_ttl, get_unauthorized_cooldown,
    BotSettings,
};
use anyhow::Context;
use oxide_agent_core::storage::StorageProvider;
use oxide_agent_core::{llm, storage};
use oxide_agent_runtime::{TaskRecovery, TaskRecoveryOptions, TaskRegistry};
use std::sync::Arc;
use teloxide::dispatching::dialogue::InMemStorage;
use teloxide::dispatching::UpdateHandler;
use teloxide::prelude::*;
use teloxide::types::CallbackQuery;
use tracing::{error, info};

/// Run the Telegram transport runtime.
pub async fn run_bot(settings: Arc<BotSettings>) -> anyhow::Result<()> {
    let storage = init_storage(&settings).await?;
    let task_registry = Arc::new(TaskRegistry::new());
    run_startup_recovery(Arc::clone(&storage), Arc::clone(&task_registry)).await?;
    let task_runtime = Arc::new(AgentTaskRuntime::new(
        Arc::clone(&storage),
        Arc::clone(&task_registry),
        settings.telegram.agent_allowed_users().len().max(1),
    ));

    let llm_client = Arc::new(llm::LlmClient::new(settings.agent.as_ref()));
    let handler_context = Arc::new(TelegramHandlerContext {
        storage: Arc::clone(&storage),
        llm: Arc::clone(&llm_client),
        settings: Arc::clone(&settings),
        task_runtime: Arc::clone(&task_runtime),
    });
    info!("LLM Client initialized.");

    let bot = Bot::new(settings.telegram.telegram_token.clone());
    let bot_state = init_bot_state();
    let unauthorized_cache = init_unauthorized_cache();
    let handler = setup_handler();

    info!("Bot is running...");

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![
            handler_context,
            bot_state,
            unauthorized_cache
        ])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;

    Ok(())
}

async fn init_storage(settings: &BotSettings) -> anyhow::Result<Arc<dyn storage::StorageProvider>> {
    match storage::R2Storage::new(settings.agent.as_ref()).await {
        Ok(s) => {
            info!("R2 Storage initialized.");
            if s.check_connection().await.is_ok() {
                // Success message already logged in check_connection
            } else {
                error!("R2 Storage connection check returned error.");
            }
            Ok(Arc::new(s) as Arc<dyn storage::StorageProvider>)
        }
        Err(error) => Err(anyhow::Error::new(error).context("failed to initialize R2 storage")),
    }
}

async fn run_startup_recovery(
    storage: Arc<dyn storage::StorageProvider>,
    task_registry: Arc<TaskRegistry>,
) -> anyhow::Result<()> {
    let recovery = TaskRecovery::new(TaskRecoveryOptions {
        task_registry,
        storage,
    });

    let report = recovery
        .reconcile()
        .await
        .context("boot-time task recovery failed")?;

    info!(
        total_snapshots = report.total_snapshots,
        restored_records = report.restored_records,
        failed_recoveries = report.failed_recoveries,
        "Boot-time task recovery completed"
    );

    Ok(())
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
                .filter(|q: CallbackQuery, context: Arc<TelegramHandlerContext>| {
                    context
                        .settings
                        .telegram
                        .allowed_users()
                        .contains(&q.from.id.0.cast_signed())
                })
                .endpoint(handle_callback),
        )
        .branch(
            Update::filter_message().branch(
                // Main branch for authorized users
                dptree::filter(|msg: Message, context: Arc<TelegramHandlerContext>| {
                    context
                        .settings
                        .telegram
                        .allowed_users()
                        .contains(&get_user_id_safe(&msg))
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
    dialogue: Dialogue<State, InMemStorage<State>>,
    cache: Arc<UnauthorizedCache>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    let res = match cmd {
        Command::Start => bot::handlers::start(bot, msg, dialogue, Arc::clone(&context)).await,
        Command::Clear => bot::handlers::clear(bot, msg, Arc::clone(&context.storage)).await,
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
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::handlers::handle_text(bot, msg, dialogue, context)).await {
        error!("Text handler error: {}", e);
    }
    respond(())
}

async fn handle_start_voice(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::handlers::handle_voice(bot, msg, dialogue, context)).await {
        error!("Voice handler error: {}", e);
    }
    respond(())
}

async fn handle_start_photo(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_photo(bot, msg, dialogue, context).await {
        error!("Photo handler error: {}", e);
    }
    respond(())
}

async fn handle_start_document(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = bot::handlers::handle_document(bot, msg, dialogue, context).await {
        error!("Document handler error: {}", e);
    }
    respond(())
}

async fn handle_editing_prompt(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) =
        bot::handlers::handle_editing_prompt(bot, msg, Arc::clone(&context.storage), dialogue).await
    {
        error!("Editing prompt handler error: {}", e);
    }
    respond(())
}

async fn handle_agent_message(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) = Box::pin(bot::agent_handlers::handle_agent_message(
        bot, msg, dialogue, context,
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
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    match bot::handlers::handle_chat_flow_callback(&bot, &q, &context.storage).await {
        Ok(true) => {
            return respond(());
        }
        Ok(false) => {}
        Err(e) => {
            error!("Chat flow callback handler error: {}", e);
            return respond(());
        }
    }

    if !context
        .settings
        .telegram
        .agent_allowed_users()
        .contains(&q.from.id.0.cast_signed())
    {
        return respond(());
    }

    if let Err(e) = bot::agent_handlers::handle_loop_callback(bot, q, context).await {
        error!("Loop callback handler error: {}", e);
    }
    respond(())
}

async fn handle_agent_confirmation(
    bot: Bot,
    msg: Message,
    dialogue: Dialogue<State, InMemStorage<State>>,
    action: bot::state::ConfirmationType,
    context: Arc<TelegramHandlerContext>,
) -> Result<(), teloxide::RequestError> {
    if let Err(e) =
        bot::agent_handlers::handle_agent_confirmation(bot, msg, dialogue, action, context).await
    {
        error!("Agent confirmation handler error: {}", e);
    }
    respond(())
}

#[cfg(test)]
mod tests {
    use super::run_startup_recovery;
    use crate::bot::agent_handlers::AgentTaskRuntime;
    use anyhow::Result as AnyResult;
    use async_trait::async_trait;
    use oxide_agent_core::agent::{AgentMemory, SessionId, TaskMetadata, TaskSnapshot, TaskState};
    use oxide_agent_core::storage::{Message, StorageError, StorageProvider, UserConfig};
    use oxide_agent_runtime::{
        TaskExecutionBackend, TaskExecutionOutcome, TaskExecutionRequest, TaskRegistry,
    };
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::{Mutex, Notify};
    use tokio::time::{timeout, Duration};

    #[derive(Default)]
    struct RecoveryStorage {
        snapshots: Mutex<HashMap<oxide_agent_core::agent::TaskId, TaskSnapshot>>,
        events: Mutex<
            HashMap<oxide_agent_core::agent::TaskId, Vec<oxide_agent_core::agent::TaskEvent>>,
        >,
    }

    struct BlockingBackend {
        started: Arc<Notify>,
        release: Arc<Notify>,
    }

    #[async_trait]
    impl TaskExecutionBackend for BlockingBackend {
        async fn execute(&self, _request: TaskExecutionRequest) -> AnyResult<TaskExecutionOutcome> {
            self.started.notify_one();
            self.release.notified().await;
            Ok(TaskExecutionOutcome::Completed)
        }
    }

    #[async_trait]
    impl StorageProvider for RecoveryStorage {
        async fn get_user_config(&self, _user_id: i64) -> Result<UserConfig, StorageError> {
            Ok(UserConfig::default())
        }

        async fn update_user_config(
            &self,
            _user_id: i64,
            _config: UserConfig,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn update_user_prompt(
            &self,
            _user_id: i64,
            _prompt: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_prompt(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_model(
            &self,
            _user_id: i64,
            _model_name: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_model(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn update_user_state(
            &self,
            _user_id: i64,
            _state: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_user_state(&self, _user_id: i64) -> Result<Option<String>, StorageError> {
            Ok(None)
        }

        async fn save_message(
            &self,
            _user_id: i64,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history(
            &self,
            _user_id: i64,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_message_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _role: String,
            _content: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn get_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
            _limit: usize,
        ) -> Result<Vec<Message>, StorageError> {
            Ok(Vec::new())
        }

        async fn clear_chat_history_for_chat(
            &self,
            _user_id: i64,
            _chat_uuid: String,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_agent_memory(
            &self,
            _user_id: i64,
            _memory: &AgentMemory,
        ) -> Result<(), StorageError> {
            Ok(())
        }

        async fn load_agent_memory(
            &self,
            _user_id: i64,
        ) -> Result<Option<AgentMemory>, StorageError> {
            Ok(None)
        }

        async fn clear_agent_memory(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn clear_all_context(&self, _user_id: i64) -> Result<(), StorageError> {
            Ok(())
        }

        async fn save_task_snapshot(&self, snapshot: &TaskSnapshot) -> Result<(), StorageError> {
            self.snapshots
                .lock()
                .await
                .insert(snapshot.metadata.id, snapshot.clone());
            Ok(())
        }

        async fn load_task_snapshot(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Option<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.get(&task_id).cloned())
        }

        async fn list_task_snapshots(&self) -> Result<Vec<TaskSnapshot>, StorageError> {
            Ok(self.snapshots.lock().await.values().cloned().collect())
        }

        async fn append_task_event(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
            event: oxide_agent_core::agent::TaskEvent,
        ) -> Result<(), StorageError> {
            self.events
                .lock()
                .await
                .entry(task_id)
                .or_default()
                .push(event);
            Ok(())
        }

        async fn load_task_events(
            &self,
            task_id: oxide_agent_core::agent::TaskId,
        ) -> Result<Vec<oxide_agent_core::agent::TaskEvent>, StorageError> {
            Ok(self
                .events
                .lock()
                .await
                .get(&task_id)
                .cloned()
                .unwrap_or_default())
        }

        async fn check_connection(&self) -> Result<(), String> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn task_recovery_runs_before_dispatcher_starts_and_retains_registry_entries() {
        let storage = Arc::new(RecoveryStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());

        let mut snapshot = TaskSnapshot::new(
            TaskMetadata::new(),
            SessionId::from(7),
            "running".to_string(),
            1,
        );
        snapshot.metadata.state = TaskState::Running;
        let task_id = snapshot.metadata.id;
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let result = run_startup_recovery(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
        )
        .await;
        assert!(result.is_ok());

        let snapshot = storage.load_task_snapshot(task_id).await;
        assert!(snapshot.is_ok());
        let snapshot = snapshot.ok().flatten();
        assert!(matches!(
            snapshot,
            Some(ref snapshot) if snapshot.metadata.state == TaskState::Failed
        ));

        let recovered_record = task_registry.get(&task_id).await;
        assert!(matches!(
            recovered_record,
            Some(ref record) if record.metadata.state == TaskState::Failed
        ));
    }

    #[tokio::test]
    async fn task_runtime_uses_recovery_registry_for_live_submission() {
        let storage = Arc::new(RecoveryStorage::default());
        let task_registry = Arc::new(TaskRegistry::new());

        let mut snapshot = TaskSnapshot::new(
            TaskMetadata::new(),
            SessionId::from(7),
            "running".to_string(),
            1,
        );
        snapshot.metadata.state = TaskState::Running;
        let recovered_task_id = snapshot.metadata.id;
        assert!(storage.save_task_snapshot(&snapshot).await.is_ok());

        let recovery_result = run_startup_recovery(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
        )
        .await;
        assert!(recovery_result.is_ok());

        let task_runtime = AgentTaskRuntime::new(
            Arc::clone(&storage) as Arc<dyn StorageProvider>,
            Arc::clone(&task_registry),
            1,
        );
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let backend = Arc::new(BlockingBackend {
            started: Arc::clone(&started),
            release: Arc::clone(&release),
        });

        let submit_result = task_runtime
            .submit_task(SessionId::from(8), "live".to_string(), backend)
            .await;
        assert!(submit_result.is_ok());

        let wait_result = timeout(Duration::from_secs(1), started.notified()).await;
        assert!(wait_result.is_ok());

        let recovered_record = task_registry.get(&recovered_task_id).await;
        assert!(matches!(
            recovered_record,
            Some(ref record) if record.metadata.state == TaskState::Failed
        ));

        let live_record = task_runtime
            .active_task_for_session(SessionId::from(8))
            .await;
        assert!(matches!(
            live_record,
            Some(ref record)
                if matches!(record.metadata.state, TaskState::Pending | TaskState::Running)
        ));

        release.notify_one();
    }
}
