use reqwest::Url;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if !is_google_devsite_url(&host, url) {
        return None;
    }

    Some(KnownMarkdownSource::google_devsite(
        url.clone(),
        url.clone(),
        "google_devsite_html_fast_path",
    ))
}

fn is_google_devsite_url(host: &str, url: &Url) -> bool {
    match host {
        "ai.google.dev"
        | "developers.google.com"
        | "developer.android.com"
        | "firebase.google.com"
        | "docs.cloud.google.com" => true,
        "cloud.google.com" => {
            url.path_segments().and_then(|mut segments| segments.next()) == Some("docs")
        }
        _ => false,
    }
}
