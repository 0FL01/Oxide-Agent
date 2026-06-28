use async_trait::async_trait;
use oxide_agent_life::delivery::{
    LifeDeliverySendFailure, LifeDeliverySender, LifeDeliveryWorker, LifeDeliveryWorkerOutcome,
};
use oxide_agent_life::domain::{ClaimedLifeDelivery, LifeTransportId, TELEGRAM_TRANSPORT_ID};
use oxide_agent_life::storage::SqlxLifeStorage;
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, error, info, warn};

use super::AppState;

const LIFE_TELEGRAM_BOT_TOKEN_ENV: &str = "LIFE_TELEGRAM_BOT_TOKEN";
const TELEGRAM_DELIVERY_WORKER_ID: &str = "web-telegram-delivery";
const TELEGRAM_SEND_MESSAGE_LIMIT: usize = 4096;
const TELEGRAM_DELIVERY_IDLE_SLEEP: Duration = Duration::from_secs(2);
const TELEGRAM_DELIVERY_ACTIVE_SLEEP: Duration = Duration::from_millis(100);

pub(crate) fn spawn_telegram_delivery_worker_from_env(state: AppState) {
    let Some(life_storage) = state.life_storage() else {
        return;
    };
    let Some(bot_token) = telegram_bot_token_from_env() else {
        return;
    };

    let Ok(transport_id) = LifeTransportId::new(TELEGRAM_TRANSPORT_ID) else {
        error!("Telegram life delivery transport id is invalid");
        return;
    };
    let sender = TelegramLifeDeliverySender::new(bot_token);
    let worker = match LifeDeliveryWorker::new(
        life_storage.as_ref().clone(),
        sender,
        transport_id,
        TELEGRAM_DELIVERY_WORKER_ID,
    ) {
        Ok(worker) => worker,
        Err(error) => {
            error!("Telegram life delivery worker is misconfigured: {error}");
            return;
        }
    };

    tokio::spawn(async move {
        info!("Telegram life delivery worker started");
        run_telegram_delivery_loop(worker).await;
    });
}

async fn run_telegram_delivery_loop(
    worker: LifeDeliveryWorker<SqlxLifeStorage, TelegramLifeDeliverySender>,
) {
    loop {
        let now = now_millis();

        let sleep_for = match worker.process_one(now).await {
            Ok(LifeDeliveryWorkerOutcome::Idle) => TELEGRAM_DELIVERY_IDLE_SLEEP,
            Ok(LifeDeliveryWorkerOutcome::Delivered { delivery_id }) => {
                debug!(%delivery_id, "Telegram life delivery delivered");
                TELEGRAM_DELIVERY_ACTIVE_SLEEP
            }
            Ok(LifeDeliveryWorkerOutcome::Failed {
                delivery_id,
                next_attempt_at,
            }) => {
                warn!(%delivery_id, next_attempt_at = next_attempt_at.get(), "Telegram life delivery scheduled retry");
                TELEGRAM_DELIVERY_ACTIVE_SLEEP
            }
            Ok(LifeDeliveryWorkerOutcome::Dead { delivery_id }) => {
                warn!(%delivery_id, "Telegram life delivery dead-lettered");
                TELEGRAM_DELIVERY_ACTIVE_SLEEP
            }
            Err(error) => {
                error!("Telegram life delivery worker failed: {error}");
                TELEGRAM_DELIVERY_IDLE_SLEEP
            }
        };
        tokio::time::sleep(sleep_for).await;
    }
}

fn now_millis() -> oxide_agent_life::domain::TimestampMillis {
    oxide_agent_life::domain::TimestampMillis::new(chrono::Utc::now().timestamp_millis())
}

fn telegram_bot_token_from_env() -> Option<String> {
    std::env::var(LIFE_TELEGRAM_BOT_TOKEN_ENV)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

#[derive(Clone)]
struct TelegramLifeDeliverySender {
    client: reqwest::Client,
    bot_token: String,
}

impl TelegramLifeDeliverySender {
    fn new(bot_token: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            bot_token,
        }
    }
}

#[async_trait]
impl LifeDeliverySender for TelegramLifeDeliverySender {
    async fn send(&self, delivery: &ClaimedLifeDelivery) -> Result<(), LifeDeliverySendFailure> {
        let chat_id = telegram_chat_id_value(&delivery.delivery.delivery_address)?;
        let chunks = chunk_plain_text(&delivery.content)?;
        for chunk in chunks {
            self.send_chunk(&chat_id, &chunk).await?;
        }
        Ok(())
    }
}

impl TelegramLifeDeliverySender {
    async fn send_chunk(&self, chat_id: &Value, text: &str) -> Result<(), LifeDeliverySendFailure> {
        let url = format!("https://api.telegram.org/bot{}/sendMessage", self.bot_token);
        let response = self
            .client
            .post(url)
            .json(&serde_json::json!({
                "chat_id": chat_id,
                "text": text,
            }))
            .send()
            .await
            .map_err(|_| {
                LifeDeliverySendFailure::Retryable("telegram send request failed".to_string())
            })?;

        let status = response.status();
        let body = response
            .json::<TelegramBotApiResponse>()
            .await
            .map_err(|_| {
                LifeDeliverySendFailure::Retryable(format!(
                    "telegram response decode failed with HTTP {status}"
                ))
            })?;

        if status.is_success() && body.ok {
            return Ok(());
        }

        let description = body
            .description
            .unwrap_or_else(|| format!("telegram api returned HTTP {status}"));
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
            Err(LifeDeliverySendFailure::Retryable(description))
        } else {
            Err(LifeDeliverySendFailure::Permanent(description))
        }
    }
}

#[derive(Debug, Deserialize)]
struct TelegramBotApiResponse {
    ok: bool,
    description: Option<String>,
}

fn telegram_chat_id_value(delivery_address: &Value) -> Result<Value, LifeDeliverySendFailure> {
    let Some(chat_id) = delivery_address.get("chat_id") else {
        return Err(LifeDeliverySendFailure::Permanent(
            "telegram delivery address missing chat_id".to_string(),
        ));
    };
    if chat_id.is_string() || chat_id.is_i64() || chat_id.is_u64() {
        Ok(chat_id.clone())
    } else {
        Err(LifeDeliverySendFailure::Permanent(
            "telegram delivery address chat_id must be string or integer".to_string(),
        ))
    }
}

fn chunk_plain_text(text: &str) -> Result<Vec<String>, LifeDeliverySendFailure> {
    if text.is_empty() {
        return Err(LifeDeliverySendFailure::Permanent(
            "telegram message text is empty".to_string(),
        ));
    }

    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;
    for ch in text.chars() {
        if current_len == TELEGRAM_SEND_MESSAGE_LIMIT {
            chunks.push(current);
            current = String::new();
            current_len = 0;
        }
        current.push(ch);
        current_len += 1;
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chunk_plain_text_keeps_chunks_within_telegram_limit() {
        let text = "x".repeat(TELEGRAM_SEND_MESSAGE_LIMIT * 2 + 3);

        let chunks = chunk_plain_text(&text).expect("text should chunk");

        assert_eq!(chunks.len(), 3);
        assert!(
            chunks
                .iter()
                .all(|chunk| chunk.chars().count() <= TELEGRAM_SEND_MESSAGE_LIMIT)
        );
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn telegram_chat_id_accepts_string_or_integer_only() {
        assert_eq!(
            telegram_chat_id_value(&json!({ "chat_id": "424242" })).expect("string id"),
            json!("424242")
        );
        assert_eq!(
            telegram_chat_id_value(&json!({ "chat_id": 424242 })).expect("integer id"),
            json!(424242)
        );
        assert!(telegram_chat_id_value(&json!({ "chat_id": true })).is_err());
        assert!(telegram_chat_id_value(&json!({})).is_err());
    }

    #[test]
    fn chunk_plain_text_rejects_empty_message() {
        assert!(chunk_plain_text("").is_err());
    }
}
