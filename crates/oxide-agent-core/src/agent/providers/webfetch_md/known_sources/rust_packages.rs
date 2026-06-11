use anyhow::{Context, Result, bail};
use reqwest::Url;
use serde_json::Value;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    match host.as_str() {
        "crates.io" => crates_io_source(url),
        "docs.rs" => docs_rs_source(url),
        _ => None,
    }
}

pub(in crate::agent::providers::webfetch_md) fn selected_crate_version(
    metadata_json: &str,
    requested: Option<&str>,
) -> Result<String> {
    if let Some(version) = requested.filter(|version| !version.trim().is_empty()) {
        return Ok(version.to_string());
    }

    let value: Value =
        serde_json::from_str(metadata_json).context("invalid crates.io metadata JSON")?;
    value
        .pointer("/crate/newest_version")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/crate/max_version").and_then(Value::as_str))
        .or_else(|| {
            value
                .get("versions")
                .and_then(Value::as_array)
                .and_then(|versions| versions.first())
                .and_then(|version| version.get("num"))
                .and_then(Value::as_str)
        })
        .filter(|version| !version.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("crates.io metadata did not include a usable version"))
}

pub(in crate::agent::providers::webfetch_md) fn readme_url(
    metadata_url: &Url,
    crate_name: &str,
    version: &str,
) -> Result<Url> {
    let mut url = metadata_url.clone();
    url.set_path(&format!("/api/v1/crates/{crate_name}/{version}/readme"));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub(in crate::agent::providers::webfetch_md) fn render_readme(
    source_url: &Url,
    final_url: &Url,
    mode: &str,
    crate_name: &str,
    version: &str,
    content_type: &str,
    bytes_read: usize,
    truncated: &str,
    readme: &str,
) -> String {
    format!(
        "## Web Markdown\n\nURL: {final_url}\nSource-URL: {source_url}\nMode: {mode}\nCrate: {crate_name}\nVersion: {version}\nContent-Type: {content_type}\nFetched-Bytes: {bytes_read}\nTruncated: {truncated}\n\n### Content\n\n{readme}"
    )
}

fn crates_io_source(url: &Url) -> Option<KnownMarkdownSource> {
    let segments = path_segments(url)?;
    let ["crates", crate_name] = segments.as_slice() else {
        return None;
    };
    if !is_crate_name(crate_name) {
        return None;
    }

    Some(KnownMarkdownSource::crate_readme(
        url.clone(),
        crates_metadata_url(url.scheme(), crate_name)?,
        (*crate_name).to_string(),
        None,
        "crates_io_readme_fast_path",
    ))
}

fn docs_rs_source(url: &Url) -> Option<KnownMarkdownSource> {
    let segments = path_segments(url)?;
    let (crate_name, version) = match segments.as_slice() {
        ["crate", crate_name, version, ..] if is_crate_name(crate_name) => {
            (*crate_name, explicit_version(version))
        }
        [crate_name] if is_crate_name(crate_name) => (*crate_name, None),
        [crate_name, version, ..] if is_crate_name(crate_name) => {
            (*crate_name, explicit_version(version))
        }
        _ => return None,
    };

    Some(KnownMarkdownSource::crate_readme(
        url.clone(),
        crates_metadata_url(url.scheme(), crate_name)?,
        crate_name.to_string(),
        version,
        "docs_rs_readme_fast_path",
    ))
}

fn path_segments(url: &Url) -> Option<Vec<&str>> {
    Some(
        url.path_segments()?
            .filter(|segment| !segment.is_empty())
            .collect(),
    )
}

fn crates_metadata_url(scheme: &str, crate_name: &str) -> Option<Url> {
    let mut url = Url::parse(&format!("{scheme}://crates.io")).ok()?;
    url.set_path(&format!("/api/v1/crates/{crate_name}"));
    Some(url)
}

fn explicit_version(version: &str) -> Option<String> {
    if version == "latest" || version.trim().is_empty() {
        None
    } else {
        Some(version.to_string())
    }
}

fn is_crate_name(crate_name: &str) -> bool {
    let len = crate_name.len();
    if !(1..=64).contains(&len) {
        return false;
    }
    crate_name
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub(in crate::agent::providers::webfetch_md) fn crate_readme_parts(
    source: &KnownMarkdownSource,
) -> Result<(&Url, &Url, &str, Option<&str>, &'static str)> {
    match source {
        KnownMarkdownSource::CrateReadme {
            source_url,
            metadata_url,
            crate_name,
            version,
            mode,
        } => Ok((
            source_url,
            metadata_url,
            crate_name.as_str(),
            version.as_deref(),
            mode,
        )),
        KnownMarkdownSource::DirectReadme { .. }
        | KnownMarkdownSource::PypiProject { .. }
        | KnownMarkdownSource::GitHubGist { .. }
        | KnownMarkdownSource::HuggingFaceBlog { .. }
        | KnownMarkdownSource::HuggingFaceTree { .. } => bail!("not a crate README source"),
    }
}
