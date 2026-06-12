use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::Url;
use reqwest::header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_TYPE, USER_AGENT};
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use super::convert::{OutputWindow, WindowedOutput, html_to_markdown, window_chars};
use super::error::{display_content_type, is_html_content_type, reject_anti_bot_challenge};
use super::known_sources::{
    KnownMarkdownSource, classify as classify_known_source, github_gist, pypi, rust_packages,
};
use super::reddit::{
    parse_reddit_atom_entries, reddit_thread_rss_url, render_reddit_atom_markdown, xml_tag_text,
};
use super::url::{parse_web_url, reject_media_url, reject_unsafe_url};
use super::{
    BROWSER_USER_AGENT, DEFAULT_TIMEOUT_SECS, MARKDOWN_ACCEPT_HEADER, MAX_OFFSET_CHARS,
    MAX_OUTPUT_CHARS, MAX_OUTPUT_CHARS_REQUEST, MAX_RESPONSE_BYTES, MAX_TIMEOUT_SECS,
    MIN_OUTPUT_CHARS, SIMPLE_BOT_USER_AGENT, WebFetchMdProvider, WebMarkdownArgs,
};

struct FetchResult {
    final_url: Url,
    content_type: String,
    bytes_read: usize,
    text: String,
}

impl WebFetchMdProvider {
    pub(super) async fn fetch_markdown(
        &self,
        args: WebMarkdownArgs,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let url = parse_web_url(&args.url)?;
        reject_media_url(&url)?;
        reject_unsafe_url(&url)?;

        let timeout_secs = args
            .timeout_secs
            .unwrap_or(DEFAULT_TIMEOUT_SECS)
            .clamp(1, MAX_TIMEOUT_SECS);
        let output_window = resolve_output_window(&args);

        if let Some(source) = classify_known_source(&url) {
            match self
                .fetch_known_markdown(&source, timeout_secs, output_window, cancellation_token)
                .await
            {
                Ok(output) => return Ok(output),
                Err(error) => {
                    if source.is_authoritative() {
                        return Err(error).context("known markdown fast-path failed");
                    }
                    tracing::warn!(
                        url = url.as_str(),
                        fetch_url = source.fetch_url().as_str(),
                        mode = source.mode(),
                        error = %error,
                        "known markdown fast-path failed, trying normal fetch"
                    );
                }
            }
        }

        // Reddit thread shortcut: fetch Atom RSS feed directly instead of
        // hitting the HTML page (which is typically blocked by anti-bot/403).
        if let Some(rss_url) = reddit_thread_rss_url(&url) {
            let markdown = self
                .fetch_reddit_rss(&url, &rss_url, timeout_secs, cancellation_token)
                .await
                .context("reddit rss fast-path failed")?;
            let windowed = window_chars(markdown.trim().to_string(), output_window);
            return Ok(format_web_markdown_output(
                &[
                    ("URL", rss_url.as_str()),
                    ("Source-URL", url.as_str()),
                    ("Mode", "reddit_rss_fast_path"),
                    ("Content-Type", "text/plain"),
                ],
                Some(0),
                output_window,
                &windowed,
            ));
        }

        let fetched = self
            .fetch_text(url, timeout_secs, cancellation_token)
            .await
            .context("web_markdown fetch failed")?;

        reject_unsafe_url(&fetched.final_url)?;

        let markdown = if is_html_content_type(&fetched.content_type) {
            html_to_markdown(&fetched.text)?
        } else {
            fetched.text
        };

        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    /// Fetch known Markdown sources directly, without fetching their HTML shell.
    async fn fetch_known_markdown(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        reject_unsafe_url(source.fetch_url())?;

        match source {
            KnownMarkdownSource::CrateReadme { .. } => {
                return self
                    .fetch_crate_readme(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::PypiProject { .. } => {
                return self
                    .fetch_pypi_project(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::GitHubGist { .. } => {
                return self
                    .fetch_github_gist(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::GitHubReadme { .. } => {
                return self
                    .fetch_github_readme(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::HuggingFaceBlog { .. } => {
                return self
                    .fetch_huggingface_blog(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::HuggingFaceTree { .. } => {
                return self
                    .fetch_huggingface_tree(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::HabrArticle { .. } => {
                return self
                    .fetch_habr_article(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::HabrComments { .. } => {
                return self
                    .fetch_habr_comments(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::GoogleDevSite { .. } => {
                return self
                    .fetch_google_devsite(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::GoogleBlog { .. } => {
                return self
                    .fetch_google_blog(source, timeout_secs, output_window, cancellation_token)
                    .await;
            }
            KnownMarkdownSource::DirectReadme { .. } => {}
        }

        let fetched = self
            .fetch_text(source.fetch_url().clone(), timeout_secs, cancellation_token)
            .await
            .context("known markdown fetch failed")?;

        reject_unsafe_url(&fetched.final_url)?;

        let markdown = if is_html_content_type(&fetched.content_type) {
            html_to_markdown(&fetched.text)?
        } else {
            fetched.text
        };

        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Source-URL", source.source_url().as_str()),
                ("Mode", source.mode()),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_crate_readme(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let (source_url, metadata_url, crate_name, requested_version, mode) =
            rust_packages::crate_readme_parts(source)?;

        reject_unsafe_url(metadata_url)?;
        let metadata = self
            .fetch_text(metadata_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("crates.io metadata fetch failed")?;
        reject_unsafe_url(&metadata.final_url)?;

        let version = rust_packages::selected_crate_version(&metadata.text, requested_version)?;
        let readme_url = rust_packages::readme_url(metadata_url, crate_name, &version)?;
        reject_unsafe_url(&readme_url)?;

        let fetched = self
            .fetch_text(readme_url, timeout_secs, cancellation_token)
            .await
            .context("crates.io README fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;

        let markdown = if is_html_content_type(&fetched.content_type) {
            html_to_markdown(&fetched.text)?
        } else {
            fetched.text
        };
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(rust_packages::render_readme(
            source_url,
            &fetched.final_url,
            mode,
            crate_name,
            &version,
            display_content_type(&fetched.content_type),
            fetched.bytes_read,
            output_window,
            &windowed,
        ))
    }

    async fn fetch_pypi_project(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let (source_url, metadata_url, package_name, mode) = pypi::pypi_project_parts(source)?;

        reject_unsafe_url(metadata_url)?;
        let fetched = self
            .fetch_text(metadata_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("PyPI metadata fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;

        let metadata = pypi::parse_project_metadata(&fetched.text, package_name)?;
        let windowed = window_chars(metadata.description.trim().to_string(), output_window);

        Ok(pypi::render_project(
            source_url,
            &fetched.final_url,
            mode,
            &metadata,
            display_content_type(&fetched.content_type),
            fetched.bytes_read,
            output_window,
            &windowed,
        ))
    }

    async fn fetch_github_gist(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let gist = github_gist::gist_parts(source)?;

        reject_unsafe_url(gist.api_url)?;
        let metadata = self
            .fetch_github_api_text(gist.api_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("GitHub Gist metadata fetch failed")?;
        reject_unsafe_url(&metadata.final_url)?;

        let files = github_gist::selected_gist_files(&metadata.text)?;
        let mut bytes_read = metadata.bytes_read;
        let mut rendered_files = Vec::new();
        let mut file_names = Vec::new();

        for file in files {
            let content = if file.truncated {
                let raw_url = file
                    .raw_url
                    .clone()
                    .context("GitHub Gist truncated file did not include raw_url")?;
                reject_unsafe_url(&raw_url)?;
                let raw = self
                    .fetch_text(raw_url, timeout_secs, cancellation_token)
                    .await
                    .context("GitHub Gist raw file fetch failed")?;
                reject_unsafe_url(&raw.final_url)?;
                bytes_read += raw.bytes_read;
                raw.text
            } else {
                file.content.clone().unwrap_or_default()
            };

            if content.trim().is_empty() {
                continue;
            }
            file_names.push(file.filename.clone());
            rendered_files.push(format!("### File: {}\n\n{}", file.filename, content.trim()));
        }

        if rendered_files.is_empty() {
            bail!("GitHub Gist did not include usable file content");
        }

        let comment_id = gist
            .comment
            .as_ref()
            .map(|comment| comment.comment_id.clone());
        if let Some(comment) = gist.comment {
            reject_unsafe_url(&comment.api_url)?;
            let fetched = self
                .fetch_github_api_text(comment.api_url, timeout_secs, cancellation_token)
                .await
                .context("GitHub Gist comment fetch failed")?;
            reject_unsafe_url(&fetched.final_url)?;
            bytes_read += fetched.bytes_read;
            let body = github_gist::parse_gist_comment_body(&fetched.text)?;
            rendered_files.push(format!("### Permalink Comment\n\n{}", body.trim()));
        }

        let content = rendered_files.join("\n\n");
        let windowed = window_chars(content.trim().to_string(), output_window);

        Ok(github_gist::render_gist(github_gist::GistRender {
            source_url: gist.source_url,
            api_url: &metadata.final_url,
            mode: gist.mode,
            owner: gist.owner,
            gist_id: gist.gist_id,
            comment_id: comment_id.as_deref(),
            files: &file_names,
            bytes_read,
            output_window,
            windowed: &windowed,
        }))
    }

    async fn fetch_github_readme(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::GitHubReadme {
            source_url,
            api_url,
            owner,
            repo,
            mode,
        } = source
        else {
            bail!("not a GitHub README source");
        };

        reject_unsafe_url(api_url)?;
        let metadata = self
            .fetch_github_api_text(api_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("GitHub README metadata fetch failed")?;
        reject_unsafe_url(&metadata.final_url)?;

        let download_url = github_readme_download_url(&metadata.text)?;
        reject_unsafe_url(&download_url)?;
        let fetched = self
            .fetch_text(download_url, timeout_secs, cancellation_token)
            .await
            .context("GitHub README raw fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;

        let markdown = if is_html_content_type(&fetched.content_type) {
            html_to_markdown(&fetched.text)?
        } else {
            fetched.text
        };
        let windowed = window_chars(markdown.trim().to_string(), output_window);
        let repo_label = format!("{owner}/{repo}");

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Source-URL", source_url.as_str()),
                ("Mode", mode),
                ("GitHub-Repo", &repo_label),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(metadata.bytes_read + fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_huggingface_blog(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::HuggingFaceBlog {
            source_url,
            fetch_url,
            mode,
        } = source
        else {
            bail!("not a HuggingFace Blog source");
        };

        let fetched = self
            .fetch_text_without_antibot(fetch_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("HuggingFace Blog fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;
        if !is_html_content_type(&fetched.content_type) {
            bail!("HuggingFace Blog returned non-HTML content");
        }

        let article_html = extract_huggingface_blog_html(&fetched.text)?;
        let markdown = html_to_markdown(article_html)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Source-URL", source_url.as_str()),
                ("Mode", mode),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_huggingface_tree(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::HuggingFaceTree {
            source_url,
            api_url,
            repo_id,
            revision,
            tree_path,
            mode,
        } = source
        else {
            bail!("not a HuggingFace tree source");
        };

        let fetched = self
            .fetch_text(api_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("HuggingFace tree API fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;

        let markdown = render_huggingface_tree_json(&fetched.text, repo_id, revision, tree_path)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Source-URL", source_url.as_str()),
                ("Mode", mode),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_habr_article(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::HabrArticle {
            source_url,
            api_url,
            fallback_url,
            article_id,
            lang,
            company,
            mode,
        } = source
        else {
            bail!("not a Habr article source");
        };

        match self
            .fetch_habr_article_json(
                source_url,
                api_url,
                article_id,
                lang,
                company,
                mode,
                timeout_secs,
                output_window,
                cancellation_token,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(error) => {
                tracing::warn!(
                    url = source_url.as_str(),
                    api_url = api_url.as_str(),
                    error = %error,
                    "Habr article JSON fast-path failed, trying article HTML fallback"
                );
                self.fetch_habr_article_html_fallback(
                    source_url,
                    fallback_url,
                    article_id,
                    lang,
                    company,
                    timeout_secs,
                    output_window,
                    cancellation_token,
                )
                .await
                .context("Habr article HTML fallback failed")
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_habr_article_json(
        &self,
        source_url: &Url,
        api_url: &Url,
        article_id: &str,
        lang: &str,
        company: &Option<String>,
        mode: &'static str,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let fetched = self
            .fetch_text(api_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("Habr article API fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;

        let markdown = render_habr_article_json(&fetched.text, article_id)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        let mut metadata = vec![
            ("URL", fetched.final_url.as_str()),
            ("Source-URL", source_url.as_str()),
            ("Mode", mode),
            ("Habr-Article-ID", article_id),
            ("Habr-Lang", lang),
        ];
        if let Some(company) = company {
            metadata.push(("Habr-Company", company.as_str()));
        }
        metadata.push(("Content-Type", display_content_type(&fetched.content_type)));

        Ok(format_web_markdown_output(
            &metadata,
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_habr_article_html_fallback(
        &self,
        source_url: &Url,
        fallback_url: &Url,
        article_id: &str,
        lang: &str,
        company: &Option<String>,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let fetched = self
            .fetch_text(fallback_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("Habr article page fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;
        if !is_html_content_type(&fetched.content_type) {
            bail!("Habr article page returned non-HTML content");
        }

        let article_html = extract_habr_article_html(&fetched.text)?;
        let markdown = html_to_markdown(article_html)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        let mut metadata = vec![
            ("URL", fetched.final_url.as_str()),
            ("Source-URL", source_url.as_str()),
            ("Mode", "habr_article_html_fallback"),
            ("Habr-Article-ID", article_id),
            ("Habr-Lang", lang),
        ];
        if let Some(company) = company {
            metadata.push(("Habr-Company", company.as_str()));
        }
        metadata.push(("Content-Type", display_content_type(&fetched.content_type)));

        Ok(format_web_markdown_output(
            &metadata,
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_habr_comments(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::HabrComments {
            source_url,
            api_url,
            fallback_url,
            article_id,
            lang,
            company,
            mode,
        } = source
        else {
            bail!("not a Habr comments source");
        };

        match self
            .fetch_habr_comments_json(
                source_url,
                api_url,
                article_id,
                lang,
                company,
                mode,
                timeout_secs,
                output_window,
                cancellation_token,
            )
            .await
        {
            Ok(output) => Ok(output),
            Err(error) => {
                tracing::warn!(
                    url = source_url.as_str(),
                    api_url = api_url.as_str(),
                    error = %error,
                    "Habr comments JSON fast-path failed, trying comments HTML fallback"
                );
                self.fetch_habr_comments_html_fallback(
                    source_url,
                    fallback_url,
                    article_id,
                    lang,
                    company,
                    timeout_secs,
                    output_window,
                    cancellation_token,
                )
                .await
                .context("Habr comments HTML fallback failed")
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn fetch_habr_comments_json(
        &self,
        source_url: &Url,
        api_url: &Url,
        article_id: &str,
        lang: &str,
        company: &Option<String>,
        mode: &'static str,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let fetched = self
            .fetch_text(api_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("Habr comments API fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;

        let markdown = render_habr_comments_json(&fetched.text, article_id)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        let mut metadata = vec![
            ("URL", fetched.final_url.as_str()),
            ("Source-URL", source_url.as_str()),
            ("Mode", mode),
            ("Habr-Article-ID", article_id),
            ("Habr-Lang", lang),
        ];
        if let Some(company) = company {
            metadata.push(("Habr-Company", company.as_str()));
        }
        metadata.push(("Content-Type", display_content_type(&fetched.content_type)));

        Ok(format_web_markdown_output(
            &metadata,
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_habr_comments_html_fallback(
        &self,
        source_url: &Url,
        fallback_url: &Url,
        article_id: &str,
        lang: &str,
        company: &Option<String>,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let fetched = self
            .fetch_text(fallback_url.clone(), timeout_secs, cancellation_token)
            .await
            .context("Habr comments page fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;
        if !is_html_content_type(&fetched.content_type) {
            bail!("Habr comments page returned non-HTML content");
        }

        let comments_html = extract_habr_comments_html(&fetched.text)?;
        let markdown = html_to_markdown(comments_html)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        let mut metadata = vec![
            ("URL", fetched.final_url.as_str()),
            ("Source-URL", source_url.as_str()),
            ("Mode", "habr_comments_html_fallback"),
            ("Habr-Article-ID", article_id),
            ("Habr-Lang", lang),
        ];
        if let Some(company) = company {
            metadata.push(("Habr-Company", company.as_str()));
        }
        metadata.push(("Content-Type", display_content_type(&fetched.content_type)));

        Ok(format_web_markdown_output(
            &metadata,
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    /// Fetch a Reddit thread via its Atom RSS feed and render as Markdown.
    async fn fetch_reddit_rss(
        &self,
        target_url: &Url,
        rss_url: &Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            bail!("web_markdown cancelled before reddit rss request");
        }

        let response = self
            .client
            .get(rss_url.clone())
            .timeout(Duration::from_secs(timeout_secs))
            .header(USER_AGENT, BROWSER_USER_AGENT)
            .header(
                ACCEPT,
                "application/atom+xml, application/xml, text/xml, */*;q=0.1",
            )
            .send()
            .await
            .context("reddit rss request failed")?;

        let status = response.status();
        if !status.is_success() {
            bail!("reddit rss returned non-success status: {status}");
        }

        let body = read_limited_body(response, cancellation_token).await?;
        let atom = String::from_utf8_lossy(&body).into_owned();

        let feed_title =
            xml_tag_text(&atom, "title").unwrap_or_else(|| "Reddit thread".to_string());
        let entries = parse_reddit_atom_entries(&atom)?;
        if entries.is_empty() {
            bail!("reddit rss parse error: empty Atom entries");
        }

        Ok(render_reddit_atom_markdown(
            target_url,
            &feed_title,
            &entries,
        ))
    }

    async fn fetch_google_devsite(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::GoogleDevSite {
            source_url,
            fetch_url,
            mode,
        } = source
        else {
            bail!("not a Google DevSite source");
        };

        let fetched = self
            .fetch_text_with_user_agent(
                fetch_url.clone(),
                timeout_secs,
                cancellation_token,
                SIMPLE_BOT_USER_AGENT,
            )
            .await
            .context("Google DevSite fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;
        if !is_html_content_type(&fetched.content_type) {
            bail!("Google DevSite returned non-HTML content");
        }

        let article_html = extract_google_devsite_html(&fetched.text)?;
        let markdown = html_to_markdown(article_html)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Source-URL", source_url.as_str()),
                ("Mode", mode),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_google_blog(
        &self,
        source: &KnownMarkdownSource,
        timeout_secs: u64,
        output_window: OutputWindow,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<String> {
        let KnownMarkdownSource::GoogleBlog {
            source_url,
            fetch_url,
            mode,
        } = source
        else {
            bail!("not a Google Blog source");
        };

        let fetched = self
            .fetch_text_with_user_agent(
                fetch_url.clone(),
                timeout_secs,
                cancellation_token,
                SIMPLE_BOT_USER_AGENT,
            )
            .await
            .context("Google Blog fetch failed")?;
        reject_unsafe_url(&fetched.final_url)?;
        if !is_html_content_type(&fetched.content_type) {
            bail!("Google Blog returned non-HTML content");
        }

        let article_html = extract_google_blog_html(&fetched.text)?;
        let markdown = html_to_markdown(article_html)?;
        let windowed = window_chars(markdown.trim().to_string(), output_window);

        Ok(format_web_markdown_output(
            &[
                ("URL", fetched.final_url.as_str()),
                ("Source-URL", source_url.as_str()),
                ("Mode", mode),
                ("Content-Type", display_content_type(&fetched.content_type)),
            ],
            Some(fetched.bytes_read),
            output_window,
            &windowed,
        ))
    }

    async fn fetch_text(
        &self,
        url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<FetchResult> {
        self.fetch_text_inner(
            url,
            timeout_secs,
            cancellation_token,
            true,
            BROWSER_USER_AGENT,
        )
        .await
    }

    async fn fetch_text_without_antibot(
        &self,
        url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<FetchResult> {
        self.fetch_text_inner(
            url,
            timeout_secs,
            cancellation_token,
            false,
            BROWSER_USER_AGENT,
        )
        .await
    }

    async fn fetch_text_with_user_agent(
        &self,
        url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
        user_agent: &'static str,
    ) -> Result<FetchResult> {
        self.fetch_text_inner(url, timeout_secs, cancellation_token, true, user_agent)
            .await
    }

    async fn fetch_text_inner(
        &self,
        url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
        reject_antibot: bool,
        user_agent: &'static str,
    ) -> Result<FetchResult> {
        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            bail!("web_markdown cancelled before request");
        }

        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(timeout_secs))
            .header(ACCEPT, MARKDOWN_ACCEPT_HEADER)
            .header(USER_AGENT, user_agent)
            .header(ACCEPT_LANGUAGE, "en-US,en;q=0.9")
            .send()
            .await
            .context("request failed")?;

        let status = response.status();
        let final_url = response.url().clone();
        let headers = response.headers().clone();
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if let Some(content_length) = response.content_length()
            && content_length > MAX_RESPONSE_BYTES as u64
        {
            bail!(
                "response too large by content-length: {} bytes; max is {}",
                content_length,
                MAX_RESPONSE_BYTES
            );
        }

        let body = read_limited_body(response, cancellation_token).await?;
        let bytes_read = body.len();
        let text = String::from_utf8_lossy(&body).into_owned();

        if reject_antibot {
            reject_anti_bot_challenge(&headers, &text)?;
        }

        if !status.is_success() {
            bail!("server returned non-success status: {status}");
        }

        Ok(FetchResult {
            final_url,
            content_type,
            bytes_read,
            text,
        })
    }

    async fn fetch_github_api_text(
        &self,
        url: Url,
        timeout_secs: u64,
        cancellation_token: Option<&CancellationToken>,
    ) -> Result<FetchResult> {
        if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
            bail!("web_markdown cancelled before GitHub API request");
        }

        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(timeout_secs))
            .header(ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10")
            .header(USER_AGENT, BROWSER_USER_AGENT)
            .send()
            .await
            .context("GitHub API request failed")?;

        let status = response.status();
        let final_url = response.url().clone();
        let headers = response.headers().clone();
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();

        if let Some(content_length) = response.content_length()
            && content_length > MAX_RESPONSE_BYTES as u64
        {
            bail!(
                "response too large by content-length: {} bytes; max is {}",
                content_length,
                MAX_RESPONSE_BYTES
            );
        }

        let body = read_limited_body(response, cancellation_token).await?;
        let bytes_read = body.len();
        let text = String::from_utf8_lossy(&body).into_owned();

        reject_anti_bot_challenge(&headers, &text)?;

        if !status.is_success() {
            bail!("GitHub API returned non-success status: {status}");
        }

        Ok(FetchResult {
            final_url,
            content_type,
            bytes_read,
            text,
        })
    }
}

fn format_web_markdown_output(
    metadata: &[(&str, &str)],
    fetched_bytes: Option<usize>,
    output_window: OutputWindow,
    windowed: &WindowedOutput,
) -> String {
    let mut output = String::from("## Web Markdown\n\n");
    for (key, value) in metadata {
        output.push_str(key);
        output.push_str(": ");
        output.push_str(value);
        output.push('\n');
    }
    if let Some(bytes) = fetched_bytes {
        output.push_str("Fetched-Bytes: ");
        output.push_str(&bytes.to_string());
        output.push('\n');
    }
    output.push_str("Max-Chars: ");
    output.push_str(&output_window.max_chars.to_string());
    output.push('\n');
    output.push_str("Offset-Chars: ");
    output.push_str(&output_window.offset_chars.to_string());
    output.push('\n');
    output.push_str("Markdown-Chars: ");
    output.push_str(&windowed.markdown_chars.to_string());
    output.push('\n');
    output.push_str("Returned-Chars: ");
    output.push_str(&windowed.returned_chars.to_string());
    output.push('\n');
    output.push_str("Remaining-Chars: ");
    output.push_str(&windowed.remaining_chars.to_string());
    output.push('\n');
    output.push_str("Next-Offset-Chars: ");
    match windowed.next_offset_chars {
        Some(offset) => output.push_str(&offset.to_string()),
        None => output.push_str("none"),
    }
    output.push('\n');
    output.push_str("Truncated: ");
    output.push_str(if windowed.was_truncated { "yes" } else { "no" });
    output.push_str("\n\n### Content\n\n");
    output.push_str(&windowed.text);
    output
}

fn resolve_output_window(args: &WebMarkdownArgs) -> OutputWindow {
    OutputWindow {
        max_chars: args
            .max_chars
            .unwrap_or(MAX_OUTPUT_CHARS)
            .clamp(MIN_OUTPUT_CHARS, MAX_OUTPUT_CHARS_REQUEST),
        offset_chars: args.offset_chars.unwrap_or(0).min(MAX_OFFSET_CHARS),
    }
}

fn github_readme_download_url(metadata_json: &str) -> Result<Url> {
    let value: Value = serde_json::from_str(metadata_json).context("invalid GitHub README JSON")?;
    let raw = value
        .get("download_url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .context("GitHub README metadata did not include download_url")?;

    Url::parse(raw).context("GitHub README metadata included invalid download_url")
}

fn extract_huggingface_blog_html(html: &str) -> Result<&str> {
    let marker = html
        .find("blog-content")
        .context("HuggingFace Blog HTML did not include blog-content")?;
    let start = html[..marker].rfind("<div").unwrap_or(marker);
    let tail = &html[marker..];
    let end = tail
        .find("</main>")
        .map(|offset| marker + offset)
        .or_else(|| tail.find("</article>").map(|offset| marker + offset))
        .or_else(|| tail.find("</body>").map(|offset| marker + offset))
        .unwrap_or(html.len());

    if end <= start {
        bail!("HuggingFace Blog HTML did not include a usable article body");
    }
    Ok(&html[start..end])
}

fn extract_habr_article_html(html: &str) -> Result<&str> {
    let marker = html
        .find("article-formatted-body")
        .or_else(|| html.find("post-content-body"))
        .context("Habr article HTML did not include article body")?;
    let start = html[..marker].rfind("<div").unwrap_or(marker);
    let tail = &html[marker..];
    let end = tail
        .find("</article>")
        .map(|offset| marker + offset)
        .or_else(|| {
            tail.find("tm-article-presenter__meta")
                .map(|offset| marker + offset)
        })
        .or_else(|| tail.find("</main>").map(|offset| marker + offset))
        .or_else(|| tail.find("</body>").map(|offset| marker + offset))
        .unwrap_or(html.len());

    if end <= start {
        bail!("Habr article HTML did not include a usable article body");
    }
    Ok(&html[start..end])
}

fn extract_habr_comments_html(html: &str) -> Result<&str> {
    let marker = html
        .find("tm-comments-wrapper")
        .context("Habr comments HTML did not include comments wrapper")?;
    let start = html[..marker].rfind("<div").unwrap_or(marker);
    let tail = &html[marker..];
    let end = tail
        .find("</main>")
        .map(|offset| marker + offset)
        .or_else(|| tail.find("</body>").map(|offset| marker + offset))
        .unwrap_or(html.len());

    if end <= start {
        bail!("Habr comments HTML did not include a usable comments body");
    }
    Ok(&html[start..end])
}

fn extract_google_devsite_html(html: &str) -> Result<&str> {
    if let Some(article) = extract_html_region(html, "devsite-article", "<article", "</article>") {
        return Ok(article);
    }

    extract_html_region(html, "id=\"main-content\"", "<main", "</main>")
        .or_else(|| extract_html_region(html, "devsite-main-content", "<main", "</main>"))
        .context("Google DevSite HTML did not include main article content")
}

fn extract_google_blog_html(html: &str) -> Result<&str> {
    if let Some(article) =
        extract_html_region(html, "uni-article-wrapper", "<article", "</article>")
    {
        return Ok(article);
    }

    if let Some(article_body) = extract_html_region(html, "article-body", "<div", "</article>") {
        return Ok(article_body);
    }

    extract_html_region(html, "id=\"jump-content\"", "<main", "</main>")
        .context("Google Blog HTML did not include article content")
}

fn extract_html_region<'a>(
    html: &'a str,
    marker: &str,
    start_tag: &str,
    end_tag: &str,
) -> Option<&'a str> {
    let marker_index = html.find(marker)?;
    let start = html[..marker_index]
        .rfind(start_tag)
        .unwrap_or(marker_index);
    let tail = &html[marker_index..];
    let end = tail
        .find(end_tag)
        .map(|offset| marker_index + offset + end_tag.len())
        .or_else(|| tail.find("</body>").map(|offset| marker_index + offset))
        .unwrap_or(html.len());

    (end > start).then_some(&html[start..end])
}

fn render_habr_article_json(article_json: &str, article_id: &str) -> Result<String> {
    let value: Value = serde_json::from_str(article_json).context("invalid Habr article JSON")?;
    let returned_id = habr_json_string_or_number(value.get("id"))
        .filter(|value| value == article_id)
        .context("Habr article JSON did not include matching article id")?;

    let mut output = String::new();
    let title = value
        .get("titleHtml")
        .and_then(Value::as_str)
        .map(html_to_markdown)
        .transpose()?
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("Habr article {returned_id}"));
    output.push_str("# ");
    output.push_str(&title);
    output.push_str("\n\n");

    output.push_str("Article ID: ");
    output.push_str(&returned_id);
    output.push('\n');
    if let Some(published) = value.get("timePublished").and_then(Value::as_str) {
        output.push_str("Published: ");
        output.push_str(published);
        output.push('\n');
    }
    if let Some(reading_time) = habr_json_string_or_number(value.get("readingTime")) {
        output.push_str("Reading time: ");
        output.push_str(&reading_time);
        output.push_str(" min\n");
    }
    if let Some(author) = habr_article_author(&value) {
        output.push_str("Author: ");
        output.push_str(&author);
        output.push('\n');
    }
    if let Some(hubs) = habr_named_items(&value, "hubs")? {
        output.push_str("Hubs: ");
        output.push_str(&hubs);
        output.push('\n');
    }
    if let Some(tags) = habr_named_items(&value, "tags")? {
        output.push_str("Tags: ");
        output.push_str(&tags);
        output.push('\n');
    }
    if let Some(comments_count) = value
        .get("statistics")
        .and_then(|statistics| habr_json_string_or_number(statistics.get("commentsCount")))
    {
        output.push_str("Comments: ");
        output.push_str(&comments_count);
        output.push('\n');
    }

    if let Some(lead_html) = value
        .get("leadData")
        .and_then(|lead| lead.get("textHtml"))
        .and_then(Value::as_str)
    {
        let lead = html_to_markdown(lead_html)?.trim().to_string();
        if !lead.is_empty() {
            output.push_str("\n## Lead\n\n");
            output.push_str(&lead);
            output.push('\n');
        }
    }

    let article_html = value
        .get("textHtml")
        .and_then(Value::as_str)
        .context("Habr article JSON did not include textHtml")?;
    let article = html_to_markdown(article_html)?.trim().to_string();
    if article.is_empty() {
        bail!("Habr article JSON textHtml was empty");
    }
    output.push_str("\n## Article\n\n");
    output.push_str(&article);
    output.push('\n');

    Ok(output)
}

fn render_habr_comments_json(comments_json: &str, article_id: &str) -> Result<String> {
    let value: Value = serde_json::from_str(comments_json).context("invalid Habr comments JSON")?;
    let comments = value
        .get("comments")
        .context("Habr comments JSON did not include comments")?;

    let mut entries = match comments {
        Value::Array(items) => items.iter().collect::<Vec<_>>(),
        Value::Object(map) => map.values().collect::<Vec<_>>(),
        _ => bail!("Habr comments JSON comments field was not an array or object"),
    };

    entries.sort_by(|left, right| {
        let left_key = habr_comment_sort_key(left);
        let right_key = habr_comment_sort_key(right);
        left_key.cmp(&right_key)
    });

    let mut output = String::new();
    output.push_str("# Habr comments for article ");
    output.push_str(article_id);
    output.push_str("\n\nComments: ");
    output.push_str(&entries.len().to_string());
    output.push_str("\n\n");

    if entries.is_empty() {
        output.push_str("_No comments returned._");
        return Ok(output);
    }

    for entry in entries {
        let comment_id =
            habr_json_string_or_number(entry.get("id")).unwrap_or_else(|| "unknown".to_string());
        output.push_str("## Comment ");
        output.push_str(&comment_id);
        output.push_str("\n\n");

        if let Some(author) = habr_comment_author(entry) {
            output.push_str("Author: ");
            output.push_str(&author);
            output.push('\n');
        }
        if let Some(parent_id) = habr_json_string_or_number(entry.get("parentId")) {
            output.push_str("Parent: ");
            output.push_str(&parent_id);
            output.push('\n');
        }
        if let Some(level) = habr_json_string_or_number(entry.get("level")) {
            output.push_str("Level: ");
            output.push_str(&level);
            output.push('\n');
        }
        if let Some(score) = habr_json_string_or_number(entry.get("score")) {
            output.push_str("Score: ");
            output.push_str(&score);
            output.push('\n');
        }
        if let Some(published) = entry.get("timePublished").and_then(Value::as_str) {
            output.push_str("Published: ");
            output.push_str(published);
            output.push('\n');
        }

        let message_html = entry
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let message = html_to_markdown(message_html)?.trim().to_string();
        if !message.is_empty() {
            output.push('\n');
            output.push_str(&message);
            output.push('\n');
        }
        output.push('\n');
    }

    Ok(output)
}

fn habr_comment_sort_key(comment: &Value) -> String {
    comment
        .get("timePublished")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| habr_json_string_or_number(comment.get("id")))
        .unwrap_or_default()
}

fn habr_comment_author(comment: &Value) -> Option<String> {
    let author = comment.get("author")?;
    habr_author_name(author)
}

fn habr_article_author(article: &Value) -> Option<String> {
    let author = article.get("author")?;
    habr_author_name(author)
}

fn habr_author_name(author: &Value) -> Option<String> {
    author
        .get("alias")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            author
                .get("fullname")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
        })
        .map(str::to_string)
}

fn habr_named_items(value: &Value, field: &str) -> Result<Option<String>> {
    let Some(items) = value.get(field).and_then(Value::as_array) else {
        return Ok(None);
    };

    let mut names = Vec::new();
    for item in items {
        let Some(raw_name) = item
            .get("titleHtml")
            .and_then(Value::as_str)
            .or_else(|| item.get("title").and_then(Value::as_str))
        else {
            continue;
        };
        let name = html_to_markdown(raw_name)?.trim().to_string();
        if !name.is_empty() {
            names.push(name);
        }
    }

    if names.is_empty() {
        Ok(None)
    } else {
        Ok(Some(names.join(", ")))
    }
}

fn habr_json_string_or_number(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) if !value.is_empty() => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn render_huggingface_tree_json(
    tree_json: &str,
    repo_id: &str,
    revision: &str,
    tree_path: &Option<String>,
) -> Result<String> {
    let value: Value = serde_json::from_str(tree_json).context("invalid HuggingFace tree JSON")?;
    let entries = value
        .as_array()
        .context("HuggingFace tree JSON was not an array")?;

    let mut output = String::new();
    output.push_str("# HuggingFace repository tree\n\n");
    output.push_str("Repository: `");
    output.push_str(repo_id);
    output.push_str("`\n");
    output.push_str("Revision: `");
    output.push_str(revision);
    output.push_str("`\n");
    if let Some(path) = tree_path {
        output.push_str("Path: `");
        output.push_str(path);
        output.push_str("`\n");
    }
    output.push('\n');

    if entries.is_empty() {
        output.push_str("_No entries returned._");
        return Ok(output);
    }

    for entry in entries {
        let path = entry
            .get("path")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("HuggingFace tree entry did not include path")?;
        let entry_type = entry.get("type").and_then(Value::as_str).unwrap_or("file");
        let suffix = if entry_type == "directory" { "/" } else { "" };

        output.push_str("- `");
        output.push_str(path);
        output.push_str(suffix);
        output.push('`');
        if entry_type != "file" && entry_type != "directory" {
            output.push_str(" (");
            output.push_str(entry_type);
            output.push(')');
        }
        if let Some(size) = entry.get("size").and_then(Value::as_u64)
            && size > 0
        {
            output.push_str(" — ");
            output.push_str(&size.to_string());
            output.push_str(" bytes");
        }
        if entry.get("lfs").is_some() {
            output.push_str(" — LFS");
        }
        output.push('\n');
    }

    Ok(output)
}

async fn read_limited_body(
    response: reqwest::Response,
    cancellation_token: Option<&CancellationToken>,
) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();

    loop {
        let next_chunk = if let Some(token) = cancellation_token {
            tokio::select! {
                () = token.cancelled() => bail!("web_markdown cancelled while reading response"),
                chunk = stream.next() => chunk,
            }
        } else {
            stream.next().await
        };

        let Some(chunk) = next_chunk else {
            return Ok(body);
        };
        let chunk = chunk.context("failed to read response chunk")?;

        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            bail!(
                "response body too large: exceeds {} bytes",
                MAX_RESPONSE_BYTES
            );
        }
        body.extend_from_slice(&chunk);
    }
}
