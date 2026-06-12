use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde_json::Value;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host != "pypi.org" {
        return None;
    }

    let segments: Vec<_> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let ["project", package_name] = segments.as_slice() else {
        return None;
    };
    if !is_package_name(package_name) {
        return None;
    }

    Some(KnownMarkdownSource::pypi_project(
        url.clone(),
        metadata_url(url.scheme(), package_name)?,
        (*package_name).to_string(),
        "pypi_project_fast_path",
    ))
}

pub(in crate::agent::providers::webfetch_md) struct PypiProjectMetadata {
    pub(in crate::agent::providers::webfetch_md) name: String,
    pub(in crate::agent::providers::webfetch_md) version: Option<String>,
    pub(in crate::agent::providers::webfetch_md) summary: Option<String>,
    pub(in crate::agent::providers::webfetch_md) description: String,
    pub(in crate::agent::providers::webfetch_md) description_content_type: Option<String>,
    pub(in crate::agent::providers::webfetch_md) project_url: Option<String>,
}

pub(in crate::agent::providers::webfetch_md) fn parse_project_metadata(
    metadata_json: &str,
    fallback_name: &str,
) -> Result<PypiProjectMetadata> {
    let value: Value = serde_json::from_str(metadata_json).context("invalid PyPI metadata JSON")?;
    let info = value
        .get("info")
        .and_then(Value::as_object)
        .context("PyPI metadata did not include info object")?;

    let description = info
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .ok_or_else(|| anyhow::anyhow!("PyPI metadata did not include a usable description"))?
        .to_string();

    Ok(PypiProjectMetadata {
        name: optional_str(info.get("name")).unwrap_or_else(|| fallback_name.to_string()),
        version: optional_str(info.get("version")),
        summary: optional_str(info.get("summary")),
        description,
        description_content_type: optional_str(info.get("description_content_type")),
        project_url: project_url(info),
    })
}

pub(in crate::agent::providers::webfetch_md) fn render_project(
    source_url: &Url,
    final_url: &Url,
    mode: &str,
    metadata: &PypiProjectMetadata,
    content_type: &str,
    bytes_read: usize,
    truncated: &str,
    description: &str,
) -> String {
    let version = metadata.version.as_deref().unwrap_or("unknown");
    let content_kind = metadata
        .description_content_type
        .as_deref()
        .unwrap_or("unknown");

    let mut output = format!(
        "## Web Markdown\n\nURL: {final_url}\nSource-URL: {source_url}\nMode: {mode}\nPackage: {}\nVersion: {version}\nDescription-Content-Type: {content_kind}\nContent-Type: {content_type}\nFetched-Bytes: {bytes_read}\nTruncated: {truncated}",
        metadata.name
    );

    if let Some(summary) = metadata
        .summary
        .as_deref()
        .filter(|summary| !summary.is_empty())
    {
        output.push_str("\nSummary: ");
        output.push_str(summary);
    }
    if let Some(project_url) = metadata
        .project_url
        .as_deref()
        .filter(|project_url| !project_url.is_empty())
    {
        output.push_str("\nProject-URL: ");
        output.push_str(project_url);
    }

    output.push_str("\n\n### Content\n\n");
    output.push_str(description);
    output
}

pub(in crate::agent::providers::webfetch_md) fn pypi_project_parts(
    source: &KnownMarkdownSource,
) -> Result<(&Url, &Url, &str, &'static str)> {
    match source {
        KnownMarkdownSource::PypiProject {
            source_url,
            metadata_url,
            package_name,
            mode,
        } => Ok((source_url, metadata_url, package_name.as_str(), mode)),
        KnownMarkdownSource::DirectReadme { .. }
        | KnownMarkdownSource::GitHubReadme { .. }
        | KnownMarkdownSource::CrateReadme { .. }
        | KnownMarkdownSource::GitHubGist { .. }
        | KnownMarkdownSource::HuggingFaceBlog { .. }
        | KnownMarkdownSource::HuggingFaceTree { .. }
        | KnownMarkdownSource::HabrArticle { .. }
        | KnownMarkdownSource::HabrComments { .. } => bail!("not a PyPI project source"),
    }
}

fn metadata_url(scheme: &str, package_name: &str) -> Option<Url> {
    let mut url = Url::parse(&format!("{scheme}://pypi.org")).ok()?;
    url.set_path(&format!("/pypi/{package_name}/json"));
    Some(url)
}

fn optional_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn project_url(info: &serde_json::Map<String, Value>) -> Option<String> {
    optional_str(info.get("home_page")).or_else(|| {
        info.get("project_urls")
            .and_then(Value::as_object)
            .and_then(|urls| {
                ["Homepage", "Source", "Repository", "Documentation"]
                    .into_iter()
                    .find_map(|key| optional_str(urls.get(key)))
                    .or_else(|| urls.values().find_map(|value| optional_str(Some(value))))
            })
    })
}

fn is_package_name(package_name: &str) -> bool {
    let len = package_name.len();
    if !(1..=214).contains(&len) {
        return false;
    }
    package_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || byte == b'.')
}
