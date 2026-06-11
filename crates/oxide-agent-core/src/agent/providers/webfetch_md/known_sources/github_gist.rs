use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde_json::Value;

use super::KnownMarkdownSource;

const MAX_GIST_FILES: usize = 5;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host != "gist.github.com" {
        return None;
    }

    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let [owner, gist_id, ..] = segments.as_slice() else {
        return None;
    };
    if !is_github_login(owner) || !is_gist_id(gist_id) {
        return None;
    }

    let comment_id = url
        .query_pairs()
        .find(|(key, _)| key == "permalink_comment_id")
        .map(|(_, value)| value.into_owned())
        .filter(|value| is_comment_id(value));

    Some(KnownMarkdownSource::github_gist(
        url.clone(),
        gist_api_url(url.scheme(), gist_id)?,
        (*owner).to_string(),
        (*gist_id).to_string(),
        comment_id,
        "github_gist_fast_path",
    ))
}

pub(in crate::agent::providers::webfetch_md) struct GistFileContent {
    pub(in crate::agent::providers::webfetch_md) filename: String,
    pub(in crate::agent::providers::webfetch_md) content: Option<String>,
    pub(in crate::agent::providers::webfetch_md) raw_url: Option<Url>,
    pub(in crate::agent::providers::webfetch_md) truncated: bool,
}

pub(in crate::agent::providers::webfetch_md) struct GistCommentPlan {
    pub(in crate::agent::providers::webfetch_md) comment_id: String,
    pub(in crate::agent::providers::webfetch_md) api_url: Url,
}

pub(in crate::agent::providers::webfetch_md) struct GistParts<'a> {
    pub(in crate::agent::providers::webfetch_md) source_url: &'a Url,
    pub(in crate::agent::providers::webfetch_md) api_url: &'a Url,
    pub(in crate::agent::providers::webfetch_md) owner: &'a str,
    pub(in crate::agent::providers::webfetch_md) gist_id: &'a str,
    pub(in crate::agent::providers::webfetch_md) comment: Option<GistCommentPlan>,
    pub(in crate::agent::providers::webfetch_md) mode: &'static str,
}

pub(in crate::agent::providers::webfetch_md) struct GistRender<'a> {
    pub(in crate::agent::providers::webfetch_md) source_url: &'a Url,
    pub(in crate::agent::providers::webfetch_md) api_url: &'a Url,
    pub(in crate::agent::providers::webfetch_md) mode: &'a str,
    pub(in crate::agent::providers::webfetch_md) owner: &'a str,
    pub(in crate::agent::providers::webfetch_md) gist_id: &'a str,
    pub(in crate::agent::providers::webfetch_md) comment_id: Option<&'a str>,
    pub(in crate::agent::providers::webfetch_md) files: &'a [String],
    pub(in crate::agent::providers::webfetch_md) bytes_read: usize,
    pub(in crate::agent::providers::webfetch_md) truncated: &'a str,
    pub(in crate::agent::providers::webfetch_md) content: &'a str,
}

pub(in crate::agent::providers::webfetch_md) fn gist_parts(
    source: &KnownMarkdownSource,
) -> Result<GistParts<'_>> {
    match source {
        KnownMarkdownSource::GitHubGist {
            source_url,
            api_url,
            owner,
            gist_id,
            comment_id,
            mode,
        } => Ok(GistParts {
            source_url,
            api_url,
            owner: owner.as_str(),
            gist_id: gist_id.as_str(),
            comment: comment_id.as_ref().map(|comment_id| GistCommentPlan {
                comment_id: comment_id.clone(),
                api_url: gist_comment_api_url(api_url.scheme(), gist_id, comment_id)
                    .expect("valid gist comment API URL"),
            }),
            mode,
        }),
        KnownMarkdownSource::DirectReadme { .. }
        | KnownMarkdownSource::CrateReadme { .. }
        | KnownMarkdownSource::PypiProject { .. } => bail!("not a GitHub Gist source"),
    }
}

pub(in crate::agent::providers::webfetch_md) fn selected_gist_files(
    metadata_json: &str,
) -> Result<Vec<GistFileContent>> {
    let value: Value = serde_json::from_str(metadata_json).context("invalid GitHub Gist JSON")?;
    let files = value
        .get("files")
        .and_then(Value::as_object)
        .context("GitHub Gist JSON did not include files object")?;

    let mut candidates = files
        .values()
        .filter_map(parse_gist_file)
        .collect::<Vec<_>>();
    candidates.sort_by_key(file_rank);

    let selected = candidates
        .into_iter()
        .filter(|file| file_rank(file) < 100)
        .take(MAX_GIST_FILES)
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("GitHub Gist did not include usable Markdown or text files");
    }

    Ok(selected)
}

pub(in crate::agent::providers::webfetch_md) fn parse_gist_comment_body(
    comment_json: &str,
) -> Result<String> {
    let value: Value =
        serde_json::from_str(comment_json).context("invalid GitHub Gist comment JSON")?;
    let body = value
        .get("body")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|body| !body.is_empty())
        .context("GitHub Gist comment JSON did not include a usable body")?;
    Ok(body.to_string())
}

pub(in crate::agent::providers::webfetch_md) fn render_gist(render: GistRender<'_>) -> String {
    let mut output = format!(
        "## Web Markdown\n\nURL: {}\nSource-URL: {}\nMode: {}\nOwner: {}\nGist-ID: {}\nFiles: {}\nFetched-Bytes: {}\nTruncated: {}",
        render.api_url,
        render.source_url,
        render.mode,
        render.owner,
        render.gist_id,
        render.files.join(", "),
        render.bytes_read,
        render.truncated
    );
    if let Some(comment_id) = render.comment_id {
        output.push_str("\nComment-ID: ");
        output.push_str(comment_id);
    }
    output.push_str("\n\n### Content\n\n");
    output.push_str(render.content);
    output
}

fn parse_gist_file(value: &Value) -> Option<GistFileContent> {
    let filename = value.get("filename")?.as_str()?.trim().to_string();
    if filename.is_empty() {
        return None;
    }
    let content = value
        .get("content")
        .and_then(Value::as_str)
        .map(str::to_string);
    let raw_url = value
        .get("raw_url")
        .and_then(Value::as_str)
        .and_then(|raw| Url::parse(raw).ok());
    let truncated = value
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let file = GistFileContent {
        filename,
        content,
        raw_url,
        truncated,
    };
    (file_rank(&file) < 100).then_some(file)
}

fn file_rank(file: &GistFileContent) -> usize {
    let name = file.filename.to_ascii_lowercase();
    if name == "readme" || name.starts_with("readme.") {
        0
    } else if name.ends_with(".md") || name.ends_with(".markdown") {
        1
    } else if name.ends_with(".txt") {
        2
    } else if file.content.is_some() || file.raw_url.is_some() {
        10
    } else {
        100
    }
}

fn gist_api_url(scheme: &str, gist_id: &str) -> Option<Url> {
    let mut url = Url::parse(&format!("{scheme}://api.github.com")).ok()?;
    url.set_path(&format!("/gists/{gist_id}"));
    Some(url)
}

fn gist_comment_api_url(scheme: &str, gist_id: &str, comment_id: &str) -> Option<Url> {
    let mut url = Url::parse(&format!("{scheme}://api.github.com")).ok()?;
    url.set_path(&format!("/gists/{gist_id}/comments/{comment_id}"));
    Some(url)
}

fn is_github_login(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 39 || bytes[0] == b'-' || bytes[bytes.len() - 1] == b'-' {
        return false;
    }
    bytes
        .iter()
        .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'-')
}

fn is_gist_id(value: &str) -> bool {
    matches!(value.len(), 20 | 32) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_comment_id(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}
