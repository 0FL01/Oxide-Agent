# Telegram Bot Integration

## Core Integration Pattern

```rust
use teloxide::{prelude::*, types::Message};
use tokio::sync::Mutex;

pub struct TelegramBot {
    bot: Bot,
    loop_detector: Arc<Mutex<LoopDetectionService>>,
    // ... other fields
}

#[teloxide(subscription)]
async fn handle_message(
    msg: Message,
    bot: Bot,
    loop_detector: Arc<Mutex<LoopDetectionService>>,
) -> ResponseResult<()> {
    // Check for loop at turn start
    let is_loop = {
        let mut detector = loop_detector.lock().await;
        detector.turn_started(AbortSignal::new()).await
            .unwrap_or(false)
    };

    if is_loop {
        bot.send_message(msg.chat.id, "🔄 Loop detected! Restarting...")
            .await?;

        // Disable and retry
        let mut detector = loop_detector.lock().await;
        detector.disable_for_session();
        drop(detector);

        return Ok(());
    }

    // Generate response
    let response_stream = generate_response(&msg).await?;

    let mut full_response = String::new();

    for event in response_stream {
        // Check for loop during streaming
        let loop_detected = {
            let mut detector = loop_detector.lock().await;
            detector.add_and_check(&event)
        };

        if loop_detected {
            // Send partial response
            if !full_response.is_empty() {
                bot.send_message(msg.chat.id, &full_response).await?;
            }

            bot.send_message(msg.chat.id, "⚠️ Detected repetitive pattern.")
                .await?;
            return Ok(());
        }

        // Process event
        match event {
            StreamEvent::Content(text) => {
                full_response.push_str(&text);
            }
            StreamEvent::ToolCallRequest { name, args } => {
                execute_tool(&bot, &msg, &name, &args).await?;
            }
            _ => {}
        }
    }

    // Send final response
    bot.send_message(msg.chat.id, &full_response).await?;

    Ok(())
}
```

## Command to Disable Detection

```rust
use teloxide::dispatching::UpdateHandler;

fn handler() -> UpdateHandler<Box<dyn std::error::Error + Send + Sync + 'static>> {
    dptree::entry()
        .branch(
            Update::filter_message()
                .filter_command::<DisableLoopDetectionCommand>()
                .endpoint(handle_disable_command),
        )
        .branch(
            Update::filter_message()
                .filter_command::<EnableLoopDetectionCommand>()
                .endpoint(handle_enable_command),
        )
        .branch(
            Update::filter_message()
                .endpoint(handle_message),
        )
}

async fn handle_disable_command(
    bot: Bot,
    msg: Message,
    loop_detector: Arc<Mutex<LoopDetectionService>>,
) -> ResponseResult<()> {
    let mut detector = loop_detector.lock().await;
    detector.disable_for_session();
    drop(detector);

    bot.send_message(msg.chat.id, "✅ Loop detection disabled for this session")
        .await?;
    Ok(())
}
```

## Progress Updates During Long Responses

```rust
async fn send_with_progress(
    bot: &Bot,
    chat_id: ChatId,
    loop_detector: &Arc<Mutex<LoopDetectionService>>,
    mut stream: StreamEventStream,
) -> ResponseResult<String> {
    let mut full_response = String::new();
    let mut last_update = Instant::now();
    let update_interval = Duration::from_secs(2);

    while let Some(event) = stream.next().await {
        // Check loop
        {
            let detector = loop_detector.lock().await;
            if detector.add_and_check(&event) {
                return Err(ResponseError::Bot("Loop detected".into()));
            }
        }

        // Accumulate response
        if let StreamEvent::Content(text) = event {
            full_response.push_str(&text);

            // Send typing status
            if last_update.elapsed() > update_interval {
                bot.send_chat_action(chat_id, ChatAction::Typing).await?;
                last_update = Instant::now();
            }
        }
    }

    Ok(full_response)
}
```

## Retry Logic

```rust
pub struct TelegramAgent {
    loop_detector: Arc<Mutex<LoopDetectionService>>,
    max_retries: usize,
}

impl TelegramAgent {
    pub async fn handle_with_retry(&self, msg: Message) -> ResponseResult<()> {
        for attempt in 0..self.max_retries {
            let mut detector = self.loop_detector.lock().await;

            // Reset for new attempt
            let prompt_id = format!("{}-{}", msg.id, attempt);
            detector.reset(prompt_id);

            drop(detector);

            match self.process_message(&msg).await {
                Ok(_) => return Ok(()),
                Err(e) if attempt < self.max_retries - 1 => {
                    log::warn!("Attempt {} failed: {}", attempt + 1, e);

                    // Disable detection on retry
                    let mut detector = self.loop_detector.lock().await;
                    detector.disable_for_session();

                    continue;
                }
                Err(e) => return Err(e),
            }
        }

        Err(ResponseError::Bot("Max retries exceeded".into()))
    }
}
```

## Loop Notification Template

```rust
pub struct LoopNotification {
    templates: HashMap<LoopType, String>,
}

impl LoopNotification {
    pub fn new() -> Self {
        let mut templates = HashMap::new();

        templates.insert(
            LoopType::ConsecutiveIdenticalToolCalls,
            "🔁 I'm repeating the same action. Let me try a different approach.".to_string()
        );

        templates.insert(
            LoopType::ChantingIdenticalSentences,
            "📝 I seem to be stuck on the same text. Breaking the loop.".to_string()
        );

        templates.insert(
            LoopType::LlmDetectedLoop,
            "🤖 I appear to be in an unproductive state. Restarting...".to_string()
        );

        Self { templates }
    }

    pub fn get_message(&self, loop_type: LoopType) -> &str {
        self.templates.get(&loop_type)
            .map(|s| s.as_str())
            .unwrap_or("Loop detected. Restarting...")
    }
}
```

## Integration with User Session

```rust
pub struct UserSession {
    user_id: u64,
    loop_detector: LoopDetectionService,
    message_count: usize,
}

impl UserSession {
    pub async fn handle_message(&mut self, msg: Message, bot: Bot) -> ResponseResult<()> {
        // Check loop at turn start
        if self.loop_detector.turn_started(AbortSignal::new()).await? {
            bot.send_message(
                msg.chat.id,
                LoopNotification::new().get_message(LoopType::LlmDetectedLoop)
            ).await?;
            return Ok(());
        }

        // Process message
        let response = self.generate_response(&msg).await?;

        // Send response
        bot.send_message(msg.chat.id, response).await?;

        self.message_count += 1;
        Ok(())
    }
}

pub struct SessionManager {
    sessions: HashMap<u64, UserSession>,
}

impl SessionManager {
    pub fn get_or_create(&mut self, user_id: u64) -> &mut UserSession {
        self.sessions.entry(user_id)
            .or_insert_with(|| UserSession::new(user_id))
    }
}
```

## Handling Telegram-Specific Events

```rust
pub enum TelegramStreamEvent {
    Text(String),
    Photo(InputFile),
    Document(InputFile),
    ToolCall { name: String, args: serde_json::Value },
}

impl From<TelegramStreamEvent> for StreamEvent {
    fn from(event: TelegramStreamEvent) -> Self {
        match event {
            TelegramStreamEvent::Text(text) => StreamEvent::Content(text),
            TelegramStreamEvent::ToolCall { name, args } => StreamEvent::ToolCallRequest { name, args },
            _ => StreamEvent::Content(String::new()), // Ignore non-text for loop detection
        }
    }
}
```

## Configuration for Telegram

```toml
[telegram_loop_detection]
enabled = true
max_retries = 2
reply_on_loop = true

[telegram_loop_detection.templates]
tool_calls = "🔁 Repeating actions. Trying new approach..."
content = "📝 Stuck on same text. Breaking loop..."
llm = "🤖 Unproductive state detected. Restarting..."

[telegram_loop_detection.admins]
can_disable = [123456789, 987654321]
```
