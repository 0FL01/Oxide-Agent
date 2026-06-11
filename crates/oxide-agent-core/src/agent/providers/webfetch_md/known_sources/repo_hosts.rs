use reqwest::Url;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    match host.as_str() {
        "github.com" => github_markdown_source(url),
        "gitlab.com" => gitlab_markdown_source(url),
        "huggingface.co" => huggingface_markdown_source(url),
        "codeberg.org" | "gitea.com" => gitea_markdown_source(url, true),
        _ => gitea_markdown_source(url, false),
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
        ["blog", slug] if is_huggingface_blog_slug(slug) => {
            (url_without_fragment(url), "huggingface_blog_fast_path")
        }
        ["blog", author, slug]
            if is_huggingface_blog_author(author) && is_huggingface_blog_slug(slug) =>
        {
            (url_without_fragment(url), "huggingface_blog_fast_path")
        }
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

    if mode == "huggingface_blog_fast_path" {
        return Some(KnownMarkdownSource::huggingface_blog(
            url.clone(),
            fetch_url,
            mode,
        ));
    }

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

fn url_without_fragment(url: &Url) -> Url {
    let mut fetch_url = url.clone();
    fetch_url.set_fragment(None);
    fetch_url
}

fn is_huggingface_blog_author(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn is_huggingface_blog_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn gitlab_markdown_source(url: &Url) -> Option<KnownMarkdownSource> {
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    let (fetch_url, mode) =
        if let Some(dash_index) = segments.iter().position(|segment| *segment == "-") {
            let repo_path = &segments[..dash_index];
            let suffix = &segments[dash_index + 1..];
            match suffix {
                ["blob", branch, path @ ..] if repo_path.len() >= 2 && is_readme_path(path) => (
                    gitlab_raw_url(repo_path, branch, &path.join("/"))?,
                    "gitlab_blob_fast_path",
                ),
                _ => return None,
            }
        } else if segments.len() >= 2 {
            (
                gitlab_raw_url(&segments, "HEAD", "README.md")?,
                "gitlab_readme_fast_path",
            )
        } else {
            return None;
        };

    Some(KnownMarkdownSource::direct_readme(
        url.clone(),
        fetch_url,
        mode,
    ))
}

fn gitlab_raw_url(repo_path: &[&str], branch: &str, path: &str) -> Option<Url> {
    let mut raw = Url::parse("https://gitlab.com").ok()?;
    raw.set_path(&format!("/{}/-/raw/{branch}/{path}", repo_path.join("/")));
    Some(raw)
}

fn gitea_markdown_source(url: &Url, known_host: bool) -> Option<KnownMarkdownSource> {
    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();

    let (fetch_url, mode) = match segments.as_slice() {
        [owner, repo] if known_host => (
            gitea_raw_url(url, owner, repo, "HEAD", "README.md")?,
            "gitea_readme_fast_path",
        ),
        [owner, repo, "src", "branch", branch, path @ ..] if is_readme_path(path) => (
            gitea_raw_url(url, owner, repo, branch, &path.join("/"))?,
            "gitea_src_fast_path",
        ),
        _ => return None,
    };

    Some(KnownMarkdownSource::direct_readme(
        url.clone(),
        fetch_url,
        mode,
    ))
}

fn gitea_raw_url(url: &Url, owner: &str, repo: &str, branch: &str, path: &str) -> Option<Url> {
    let mut raw = url.clone();
    raw.set_query(None);
    raw.set_fragment(None);
    raw.set_path(&format!("/{owner}/{repo}/raw/branch/{branch}/{path}"));
    Some(raw)
}

fn is_readme_path(path: &[&str]) -> bool {
    path.last()
        .is_some_and(|file_name| file_name.eq_ignore_ascii_case("README.md"))
}
