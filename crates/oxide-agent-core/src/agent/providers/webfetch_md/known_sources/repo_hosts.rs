use reqwest::Url;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    match host.as_str() {
        "github.com" => github_markdown_source(url),
        "huggingface.co" => huggingface_markdown_source(url),
        _ => None,
    }
}

fn github_markdown_source(url: &Url) -> Option<KnownMarkdownSource> {
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    let (fetch_url, mode) = match segments.as_slice() {
        [owner, repo] => (
            github_raw_url(owner, repo, "HEAD", "README.md")?,
            "github_readme_fast_path",
        ),
        [owner, repo, "blob", branch, path @ ..] if is_readme_path(path) => (
            github_raw_url(owner, repo, branch, &path.join("/"))?,
            "github_blob_fast_path",
        ),
        _ => return None,
    };

    Some(KnownMarkdownSource::direct_readme(
        url.clone(),
        fetch_url,
        mode,
    ))
}

fn github_raw_url(owner: &str, repo: &str, branch: &str, path: &str) -> Option<Url> {
    let mut raw = Url::parse("https://raw.githubusercontent.com").ok()?;
    raw.set_path(&format!("/{owner}/{repo}/{branch}/{path}"));
    Some(raw)
}

fn huggingface_markdown_source(url: &Url) -> Option<KnownMarkdownSource> {
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    let (fetch_url, mode) = match segments.as_slice() {
        [owner, repo] => (
            huggingface_resolve_url(&[*owner, *repo], "main", "README.md")?,
            "huggingface_readme_fast_path",
        ),
        [kind @ ("datasets" | "spaces"), owner, repo] => (
            huggingface_resolve_url(&[*kind, *owner, *repo], "main", "README.md")?,
            "huggingface_readme_fast_path",
        ),
        [owner, repo, "blob", branch, path @ ..] if is_readme_path(path) => (
            huggingface_resolve_url(&[*owner, *repo], branch, &path.join("/"))?,
            "huggingface_blob_fast_path",
        ),
        [
            kind @ ("datasets" | "spaces"),
            owner,
            repo,
            "blob",
            branch,
            path @ ..,
        ] if is_readme_path(path) => (
            huggingface_resolve_url(&[*kind, *owner, *repo], branch, &path.join("/"))?,
            "huggingface_blob_fast_path",
        ),
        _ => return None,
    };

    Some(KnownMarkdownSource::direct_readme(
        url.clone(),
        fetch_url,
        mode,
    ))
}

fn huggingface_resolve_url(prefix: &[&str], branch: &str, path: &str) -> Option<Url> {
    let mut resolve = Url::parse("https://huggingface.co").ok()?;
    resolve.set_path(&format!("/{}/resolve/{branch}/{path}", prefix.join("/")));
    Some(resolve)
}

fn is_readme_path(path: &[&str]) -> bool {
    path.last()
        .is_some_and(|file_name| file_name.eq_ignore_ascii_case("README.md"))
}
