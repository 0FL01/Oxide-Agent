use super::{
    user_chat_history_key, user_config_key, user_context_chat_history_prefix, user_history_key,
    Message, R2Storage, StorageError, UserConfig,
};

impl R2Storage {
    pub(super) async fn get_user_config_inner(
        &self,
        user_id: i64,
    ) -> Result<UserConfig, StorageError> {
        Ok(self
            .load_json(&user_config_key(user_id))
            .await?
            .unwrap_or_default())
    }

    pub(super) async fn update_user_config_inner(
        &self,
        user_id: i64,
        config: UserConfig,
    ) -> Result<(), StorageError> {
        self.save_json(&user_config_key(user_id), &config).await
    }

    pub(super) async fn update_user_prompt_inner(
        &self,
        user_id: i64,
        system_prompt: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.system_prompt = Some(system_prompt);
        })
        .await
    }

    pub(super) async fn get_user_prompt_inner(
        &self,
        user_id: i64,
    ) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config_inner(user_id).await?;
        Ok(config.system_prompt)
    }

    pub(super) async fn update_user_model_inner(
        &self,
        user_id: i64,
        model_name: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.model_name = Some(model_name);
        })
        .await
    }

    pub(super) async fn get_user_model_inner(
        &self,
        user_id: i64,
    ) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config_inner(user_id).await?;
        Ok(config.model_name)
    }

    pub(super) async fn update_user_state_inner(
        &self,
        user_id: i64,
        state: String,
    ) -> Result<(), StorageError> {
        self.modify_user_config(user_id, |config| {
            config.state = Some(state);
        })
        .await
    }

    pub(super) async fn get_user_state_inner(
        &self,
        user_id: i64,
    ) -> Result<Option<String>, StorageError> {
        let config = self.get_user_config_inner(user_id).await?;
        Ok(config.state)
    }

    pub(super) async fn save_message_inner(
        &self,
        user_id: i64,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let key = user_history_key(user_id);
        let mut history: Vec<Message> = self.load_json(&key).await?.unwrap_or_default();
        history.push(Message { role, content });
        self.save_json(&key, &history).await
    }

    pub(super) async fn get_chat_history_inner(
        &self,
        user_id: i64,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let history: Vec<Message> = self
            .load_json(&user_history_key(user_id))
            .await?
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    pub(super) async fn clear_chat_history_inner(&self, user_id: i64) -> Result<(), StorageError> {
        self.delete_object(&user_history_key(user_id)).await
    }

    pub(super) async fn save_message_for_chat_inner(
        &self,
        user_id: i64,
        chat_uuid: String,
        role: String,
        content: String,
    ) -> Result<(), StorageError> {
        let key = user_chat_history_key(user_id, &chat_uuid);
        let mut history: Vec<Message> = self.load_json(&key).await?.unwrap_or_default();
        history.push(Message { role, content });
        self.save_json(&key, &history).await
    }

    pub(super) async fn get_chat_history_for_chat_inner(
        &self,
        user_id: i64,
        chat_uuid: String,
        limit: usize,
    ) -> Result<Vec<Message>, StorageError> {
        let history: Vec<Message> = self
            .load_json(&user_chat_history_key(user_id, &chat_uuid))
            .await?
            .unwrap_or_default();
        let start = history.len().saturating_sub(limit);
        Ok(history[start..].to_vec())
    }

    pub(super) async fn clear_chat_history_for_chat_inner(
        &self,
        user_id: i64,
        chat_uuid: String,
    ) -> Result<(), StorageError> {
        self.delete_object(&user_chat_history_key(user_id, &chat_uuid))
            .await
    }

    pub(super) async fn clear_chat_history_for_context_inner(
        &self,
        user_id: i64,
        context_key: String,
    ) -> Result<(), StorageError> {
        let prefix = user_context_chat_history_prefix(user_id, &context_key);
        self.delete_prefix(&prefix).await
    }
}
