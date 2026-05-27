use super::{user_config_key, R2Storage, StorageError, UserConfig};

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
}
