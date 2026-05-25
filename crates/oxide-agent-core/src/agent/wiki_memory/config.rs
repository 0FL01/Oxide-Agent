/// Runtime limits for durable LLM Wiki memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiMemoryConfig {
    /// Whether durable wiki memory should run.
    pub enabled: bool,
    /// Optional S3/R2 object prefix before `wiki/v1/`.
    pub storage_prefix: String,
    /// Maximum size for normal durable wiki pages.
    pub normal_page_max_bytes: usize,
    /// Maximum size for `index.md`.
    pub index_max_bytes: usize,
    /// Maximum size for `log.md`.
    pub log_max_bytes: usize,
    /// Maximum size for one inbox item.
    pub inbox_item_max_bytes: usize,
    /// Maximum size for one optional raw archive item.
    pub raw_archive_item_max_bytes: usize,
    /// Whether optional raw archive writes are enabled.
    pub raw_archive_enabled: bool,
}

impl Default for WikiMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_prefix: String::new(),
            normal_page_max_bytes: 64 * 1024,
            index_max_bytes: 64 * 1024,
            log_max_bytes: 64 * 1024,
            inbox_item_max_bytes: 16 * 1024,
            raw_archive_item_max_bytes: 64 * 1024,
            raw_archive_enabled: false,
        }
    }
}
