use oxide_agent_web_contracts::{PersistedTaskEvent, TaskEventKind};

use super::payload::payload_str_event;

#[derive(Clone)]
pub(super) struct DeliveredFileLink {
    pub(super) file_name: String,
    pub(super) download_url: String,
    pub(super) content_type: String,
    pub(super) size_bytes: u64,
}

pub(super) fn delivered_files_for_task(
    events: &[PersistedTaskEvent],
    task_id: &str,
) -> Vec<DeliveredFileLink> {
    events
        .iter()
        .filter(|event| event.task_id == task_id)
        .filter_map(delivered_file_link)
        .collect()
}

pub(super) fn delivered_file_link(event: &PersistedTaskEvent) -> Option<DeliveredFileLink> {
    if event.kind != TaskEventKind::FileToSend {
        return None;
    }
    Some(DeliveredFileLink {
        file_name: payload_str_event(event, "file_name")?,
        download_url: payload_str_event(event, "download_url")?,
        content_type: payload_str_event(event, "content_type").unwrap_or_default(),
        size_bytes: event
            .payload
            .get("size_bytes")
            .and_then(|value| value.as_u64())
            .or_else(|| {
                event
                    .payload
                    .get("byte_len")
                    .and_then(|value| value.as_u64())
            })
            .unwrap_or(0),
    })
}

pub(super) fn linkify_delivered_files_in_markdown(
    markdown: &str,
    files: &[DeliveredFileLink],
) -> String {
    if files.is_empty() {
        return markdown.to_string();
    }

    let mut result = String::new();
    let mut in_fenced_code_block = false;

    for segment in markdown.split_inclusive('\n') {
        let trimmed = segment.trim_start();
        if trimmed.starts_with("```") {
            in_fenced_code_block = !in_fenced_code_block;
            result.push_str(segment);
            continue;
        }

        if in_fenced_code_block {
            result.push_str(segment);
            continue;
        }

        let mut rewritten = segment.to_string();
        for file in files {
            let code_span = format!("`{}`", file.file_name);
            let markdown_link = format!("[`{}`]({})", file.file_name, file.download_url);
            rewritten = rewritten.replace(&code_span, &markdown_link);
        }
        result.push_str(&rewritten);
    }

    if !markdown.ends_with('\n') {
        result.truncate(result.trim_end_matches('\n').len());
    }

    result
}

#[cfg(test)]
mod tests {
    use super::{linkify_delivered_files_in_markdown, DeliveredFileLink};

    fn delivered_file(file_name: &str, download_url: &str) -> DeliveredFileLink {
        DeliveredFileLink {
            file_name: file_name.to_string(),
            download_url: download_url.to_string(),
            content_type: "application/octet-stream".to_string(),
            size_bytes: 1,
        }
    }

    #[test]
    fn linkifies_delivered_file_code_spans_in_final_markdown() {
        let markdown = "Done: `duckduckgo.zip`\n\n- File: `duckduckgo.zip`";
        let rendered = linkify_delivered_files_in_markdown(
            markdown,
            &[delivered_file(
                "duckduckgo.zip",
                "/api/v1/files/duckduckgo.zip",
            )],
        );

        assert!(rendered.contains("[`duckduckgo.zip`](/api/v1/files/duckduckgo.zip)"));
        assert!(!rendered.contains("- File: `duckduckgo.zip`"));
    }

    #[test]
    fn does_not_linkify_inside_fenced_code_blocks() {
        let markdown = "Before `duckduckgo.zip`\n\n```text\n`duckduckgo.zip`\n```\n";
        let rendered = linkify_delivered_files_in_markdown(
            markdown,
            &[delivered_file(
                "duckduckgo.zip",
                "/api/v1/files/duckduckgo.zip",
            )],
        );

        assert!(rendered.contains("Before [`duckduckgo.zip`](/api/v1/files/duckduckgo.zip)"));
        assert!(rendered.contains("```text\n`duckduckgo.zip`\n```"));
    }
}
