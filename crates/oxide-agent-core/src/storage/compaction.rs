use super::R2Storage;
use crate::agent::compaction::{
    ArchiveChunk, ArchiveRef, ArchiveSink, ExternalizedPayloadRecord, PayloadSink,
};
use anyhow::Result;
use serde_json::Value;
use std::future::Future;
use std::sync::Arc;
use tracing::warn;

/// Sync blob backend used by compaction archive/payload sinks.
pub trait CompactionBlobBackend: Send + Sync {
    /// Persist a raw UTF-8 blob.
    fn put_text(&self, key: &str, data: &str) -> Result<()>;

    /// Persist a JSON payload.
    fn put_json_value(&self, key: &str, value: &Value) -> Result<()>;
}

fn run_blocking<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(future)
}

impl CompactionBlobBackend for R2Storage {
    fn put_text(&self, key: &str, data: &str) -> Result<()> {
        run_blocking(async { self.save_text(key, data).await.map_err(Into::into) })
    }

    fn put_json_value(&self, key: &str, value: &Value) -> Result<()> {
        run_blocking(async { self.save_json(key, value).await.map_err(Into::into) })
    }
}

/// Compaction archive sink backed by a blob backend.
#[derive(Clone)]
pub struct R2ArchiveSink {
    backend: Arc<dyn CompactionBlobBackend>,
}

impl R2ArchiveSink {
    /// Create a new archive sink.
    #[must_use]
    pub fn new<B>(backend: Arc<B>) -> Self
    where
        B: CompactionBlobBackend + 'static,
    {
        Self { backend }
    }
}

/// Compaction payload sink backed by a blob backend.
#[derive(Clone)]
pub struct R2PayloadSink {
    backend: Arc<dyn CompactionBlobBackend>,
}

impl R2PayloadSink {
    /// Create a new payload sink.
    #[must_use]
    pub fn new<B>(backend: Arc<B>) -> Self
    where
        B: CompactionBlobBackend + 'static,
    {
        Self { backend }
    }
}

fn archive_metadata_key(storage_key: &str) -> String {
    format!("{storage_key}.archive.json")
}

fn payload_metadata_key(storage_key: &str) -> String {
    format!("{storage_key}.payload.json")
}

impl ArchiveSink for R2ArchiveSink {
    fn persist(&self, chunk: &ArchiveChunk) -> Result<Option<ArchiveRef>> {
        let storage_key = if chunk.record.payload_ref.trim().is_empty() {
            chunk.record.archive_id.clone()
        } else {
            chunk.record.payload_ref.clone()
        };

        if !chunk.content.is_empty() {
            self.backend.put_text(&storage_key, &chunk.content)?;
        }

        let metadata_key = archive_metadata_key(&storage_key);
        let metadata_value = serde_json::to_value(chunk)?;
        if let Err(error) = self.backend.put_json_value(&metadata_key, &metadata_value) {
            warn!(
                archive_id = %chunk.record.archive_id,
                storage_key = %metadata_key,
                error = %error,
                "Failed to persist compaction archive metadata sidecar"
            );
        }

        Ok(Some(ArchiveRef {
            archive_id: chunk.record.archive_id.clone(),
            created_at: chunk.record.created_at,
            title: chunk.record.title.clone(),
            storage_key,
        }))
    }
}

impl PayloadSink for R2PayloadSink {
    fn persist(&self, record: &ExternalizedPayloadRecord) -> Result<bool> {
        self.backend
            .put_text(&record.storage_key, &record.content)?;

        let metadata_key = payload_metadata_key(&record.storage_key);
        let metadata_value = serde_json::to_value(record)?;
        if let Err(error) = self.backend.put_json_value(&metadata_key, &metadata_value) {
            warn!(
                archive_id = %record.archive_id,
                storage_key = %metadata_key,
                error = %error,
                "Failed to persist externalized payload metadata sidecar"
            );
        }

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::{CompactionBlobBackend, R2ArchiveSink, R2PayloadSink};
    use crate::agent::compaction::archive::{
        ARCHIVE_KIND_COMPACTED_HISTORY, ARCHIVE_KIND_TOOL_OUTPUT,
    };
    use crate::agent::compaction::{
        ArchiveChunk, ArchiveRecord, ArchiveSink, ExternalizedPayloadRecord, PayloadSink,
    };
    use anyhow::Result;
    use chrono::{TimeZone, Utc};
    use serde_json::Value;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct RecordingBackend {
        texts: Mutex<HashMap<String, String>>,
        jsons: Mutex<HashMap<String, Value>>,
    }

    impl RecordingBackend {
        fn text(&self, key: &str) -> Option<String> {
            self.texts.lock().expect("texts lock").get(key).cloned()
        }

        fn json(&self, key: &str) -> Option<Value> {
            self.jsons.lock().expect("json lock").get(key).cloned()
        }
    }

    impl CompactionBlobBackend for RecordingBackend {
        fn put_text(&self, key: &str, data: &str) -> Result<()> {
            self.texts
                .lock()
                .expect("texts lock")
                .insert(key.to_string(), data.to_string());
            Ok(())
        }

        fn put_json_value(&self, key: &str, value: &Value) -> Result<()> {
            self.jsons
                .lock()
                .expect("json lock")
                .insert(key.to_string(), value.clone());
            Ok(())
        }
    }

    fn utc(ts: i64) -> chrono::DateTime<Utc> {
        Utc.timestamp_opt(ts, 0).single().expect("valid timestamp")
    }

    #[test]
    fn archive_sink_persists_content_to_real_storage_key() {
        let backend = Arc::new(RecordingBackend::default());
        let sink = R2ArchiveSink::new(Arc::clone(&backend));
        let chunk = ArchiveChunk {
            record: ArchiveRecord {
                archive_id: "archive-1".to_string(),
                context_key: "topic-a".to_string(),
                flow_id: "flow-1".to_string(),
                created_at: 42,
                time_range_start: 42,
                time_range_end: 42,
                title: "Compacted history".to_string(),
                short_summary: "summary".to_string(),
                kind: ARCHIVE_KIND_COMPACTED_HISTORY.to_string(),
                tool_names: vec![],
                file_paths: vec![],
                payload_ref: "archive/topic-a/flow-1/history-1.json".to_string(),
            },
            content_format: "application/json".to_string(),
            content: "{\"hello\":\"world\"}".to_string(),
        };

        let reference = sink
            .persist(&chunk)
            .expect("archive sink should persist")
            .expect("archive ref should be returned");

        assert_eq!(reference.storage_key, chunk.record.payload_ref);
        assert_eq!(
            backend.text(&reference.storage_key).as_deref(),
            Some("{\"hello\":\"world\"}")
        );
        assert!(backend
            .json(&format!("{}.archive.json", reference.storage_key))
            .is_some());
    }

    #[test]
    fn payload_sink_persists_content_to_real_storage_key() {
        let backend = Arc::new(RecordingBackend::default());
        let sink = R2PayloadSink::new(Arc::clone(&backend));
        let record = ExternalizedPayloadRecord {
            archive_id: "archive-2".to_string(),
            storage_key: "compaction/topic-a/flow-1/tool-1.txt".to_string(),
            created_at: 7,
            context_key: "topic-a".to_string(),
            flow_id: "flow-1".to_string(),
            tool_name: "read_file".to_string(),
            title: "read_file artifact".to_string(),
            short_summary: "summary".to_string(),
            estimated_tokens: 10,
            original_chars: 20,
            preview: "preview".to_string(),
            content: "payload body".to_string(),
        };

        assert!(sink.persist(&record).expect("payload sink should persist"));
        assert_eq!(
            backend.text(&record.storage_key).as_deref(),
            Some("payload body")
        );
        assert!(backend
            .json(&format!("{}.payload.json", record.storage_key))
            .is_some());
    }

    #[test]
    fn archive_sink_still_returns_reference_for_metadata_only_chunk() {
        let backend = Arc::new(RecordingBackend::default());
        let sink = R2ArchiveSink::new(Arc::clone(&backend));
        let chunk = ArchiveChunk {
            record: ArchiveRecord {
                archive_id: "archive-3".to_string(),
                context_key: "topic-a".to_string(),
                flow_id: "flow-1".to_string(),
                created_at: utc(100).timestamp(),
                time_range_start: utc(100).timestamp(),
                time_range_end: utc(100).timestamp(),
                title: "Metadata only".to_string(),
                short_summary: "summary".to_string(),
                kind: ARCHIVE_KIND_TOOL_OUTPUT.to_string(),
                tool_names: vec!["read_file".to_string()],
                file_paths: vec![],
                payload_ref: "compaction/topic-a/flow-1/tool-1.txt".to_string(),
            },
            content_format: String::new(),
            content: String::new(),
        };

        let reference = sink
            .persist(&chunk)
            .expect("archive sink should persist")
            .expect("archive ref should be returned");

        assert_eq!(reference.storage_key, chunk.record.payload_ref);
        assert!(backend.text(&reference.storage_key).is_none());
        assert!(backend
            .json(&format!("{}.archive.json", reference.storage_key))
            .is_some());
    }
}
