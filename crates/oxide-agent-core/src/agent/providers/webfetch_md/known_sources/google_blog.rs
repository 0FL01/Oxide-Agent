use reqwest::Url;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host != "blog.google" || url.path() == "/" {
        return None;
    }

    Some(KnownMarkdownSource::google_blog(
        url.clone(),
        url.clone(),
        "google_blog_html_fast_path",
    ))
}
