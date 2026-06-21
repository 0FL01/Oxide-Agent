use std::collections::HashMap;

use tokio::sync::Mutex;

use super::convert::{OutputWindow, WindowedOutput};
use super::fetch::{FetchedMarkdownDocument, window_markdown_document};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkdownReadMode {
    Auto,
    Next,
}

#[derive(Debug, Clone)]
pub(crate) struct MarkdownDeliveryResult {
    pub(crate) requested_url: String,
    pub(crate) document: FetchedMarkdownDocument,
    pub(crate) output_window: OutputWindow,
    pub(crate) windowed: WindowedOutput,
}

#[derive(Debug, Default)]
pub(crate) struct MarkdownDeliveryCache {
    inner: Mutex<MarkdownDeliveryState>,
}

#[derive(Debug, Default)]
struct MarkdownDeliveryState {
    documents: HashMap<MarkdownDocumentKey, CachedMarkdownDocument>,
    last_by_session: HashMap<i64, MarkdownDocumentKey>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct MarkdownDocumentKey {
    session_id: i64,
    requested_url: String,
}

#[derive(Debug, Clone)]
struct CachedMarkdownDocument {
    document: FetchedMarkdownDocument,
    next_offset_chars: Option<usize>,
}

impl MarkdownDeliveryCache {
    pub(crate) async fn store_document_window(
        &self,
        session_id: i64,
        requested_url: String,
        document: FetchedMarkdownDocument,
        output_window: OutputWindow,
    ) -> MarkdownDeliveryResult {
        let windowed = window_markdown_document(&document, output_window);
        let key = MarkdownDocumentKey {
            session_id,
            requested_url: requested_url.clone(),
        };
        let mut state = self.inner.lock().await;
        state.documents.insert(
            key.clone(),
            CachedMarkdownDocument {
                document: document.clone(),
                next_offset_chars: windowed.next_offset_chars,
            },
        );
        state.last_by_session.insert(session_id, key);
        MarkdownDeliveryResult {
            requested_url,
            document,
            output_window,
            windowed,
        }
    }

    pub(crate) async fn next_document_window(
        &self,
        session_id: i64,
        requested_url: Option<&str>,
        mut output_window: OutputWindow,
    ) -> Option<MarkdownDeliveryResult> {
        let mut state = self.inner.lock().await;
        let key = if let Some(requested_url) = requested_url
            .map(str::trim)
            .filter(|requested_url| !requested_url.is_empty())
        {
            MarkdownDocumentKey {
                session_id,
                requested_url: requested_url.to_string(),
            }
        } else {
            state.last_by_session.get(&session_id)?.clone()
        };
        let cached = state.documents.get_mut(&key)?;
        output_window.offset_chars = cached
            .next_offset_chars
            .unwrap_or_else(|| cached.document.markdown.chars().count());
        let windowed = window_markdown_document(&cached.document, output_window);
        cached.next_offset_chars = windowed.next_offset_chars;
        let document = cached.document.clone();
        state.last_by_session.insert(session_id, key.clone());
        Some(MarkdownDeliveryResult {
            requested_url: key.requested_url,
            document,
            output_window,
            windowed,
        })
    }
}

pub(crate) fn document_metadata(document: &FetchedMarkdownDocument, key: &str) -> Option<String> {
    document
        .metadata
        .iter()
        .find_map(|(metadata_key, value)| (metadata_key == key).then(|| value.clone()))
}
