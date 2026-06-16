use reqwest::Url;

use super::KnownMarkdownSource;

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if !matches!(host.as_str(), "habr.com" | "habr.ru") {
        return None;
    }

    let segments = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let parsed = parse_habr_path(&segments)?;

    if parsed.comments {
        return Some(KnownMarkdownSource::habr_comments(
            url.clone(),
            habr_comments_api_url(url.scheme(), parsed.lang, parsed.article_id)?,
            normalized_habr_url(
                url,
                parsed.lang,
                parsed.kind,
                parsed.company,
                parsed.article_id,
                true,
            )?,
            parsed.article_id.to_string(),
            parsed.lang.to_string(),
            parsed.company.map(str::to_string),
            "habr_comments_json_fast_path",
        ));
    }

    Some(KnownMarkdownSource::habr_article(
        url.clone(),
        habr_article_api_url(url.scheme(), parsed.lang, parsed.article_id)?,
        normalized_habr_url(
            url,
            parsed.lang,
            parsed.kind,
            parsed.company,
            parsed.article_id,
            false,
        )?,
        parsed.article_id.to_string(),
        parsed.lang.to_string(),
        parsed.company.map(str::to_string),
        "habr_article_json_fast_path",
    ))
}

#[derive(Clone, Copy)]
struct HabrPath<'a> {
    lang: &'a str,
    kind: HabrKind,
    company: Option<&'a str>,
    article_id: &'a str,
    comments: bool,
}

#[derive(Clone, Copy)]
enum HabrKind {
    Article,
    News,
    CompanyArticle,
}

fn parse_habr_path<'a>(segments: &[&'a str]) -> Option<HabrPath<'a>> {
    match segments {
        [lang, "articles", article_id] if is_habr_lang(lang) && is_article_id(article_id) => {
            Some(HabrPath {
                lang,
                kind: HabrKind::Article,
                company: None,
                article_id,
                comments: false,
            })
        }
        [lang, "articles", article_id, "comments"]
            if is_habr_lang(lang) && is_article_id(article_id) =>
        {
            Some(HabrPath {
                lang,
                kind: HabrKind::Article,
                company: None,
                article_id,
                comments: true,
            })
        }
        [lang, "news", article_id] if is_habr_lang(lang) && is_article_id(article_id) => {
            Some(HabrPath {
                lang,
                kind: HabrKind::News,
                company: None,
                article_id,
                comments: false,
            })
        }
        [lang, "news", article_id, "comments"]
            if is_habr_lang(lang) && is_article_id(article_id) =>
        {
            Some(HabrPath {
                lang,
                kind: HabrKind::News,
                company: None,
                article_id,
                comments: true,
            })
        }
        [lang, "companies", company, "articles", article_id]
            if is_habr_lang(lang) && is_company_slug(company) && is_article_id(article_id) =>
        {
            Some(HabrPath {
                lang,
                kind: HabrKind::CompanyArticle,
                company: Some(company),
                article_id,
                comments: false,
            })
        }
        [
            lang,
            "companies",
            company,
            "articles",
            article_id,
            "comments",
        ] if is_habr_lang(lang) && is_company_slug(company) && is_article_id(article_id) => {
            Some(HabrPath {
                lang,
                kind: HabrKind::CompanyArticle,
                company: Some(company),
                article_id,
                comments: true,
            })
        }
        _ => None,
    }
}

fn normalized_habr_url(
    source: &Url,
    lang: &str,
    kind: HabrKind,
    company: Option<&str>,
    article_id: &str,
    comments: bool,
) -> Option<Url> {
    let mut url = Url::parse(&format!("{}://habr.com", source.scheme())).ok()?;
    let comments_suffix = if comments { "/comments" } else { "" };
    match kind {
        HabrKind::Article => {
            url.set_path(&format!("/{lang}/articles/{article_id}{comments_suffix}/"))
        }
        HabrKind::News => url.set_path(&format!("/{lang}/news/{article_id}{comments_suffix}/")),
        HabrKind::CompanyArticle => url.set_path(&format!(
            "/{lang}/companies/{}/articles/{article_id}{comments_suffix}/",
            company?
        )),
    }
    Some(url)
}

fn habr_comments_api_url(scheme: &str, lang: &str, article_id: &str) -> Option<Url> {
    Url::parse(&format!(
        "{scheme}://habr.com/kek/v2/articles/{article_id}/comments/?fl={lang}&hl={lang}"
    ))
    .ok()
}

fn habr_article_api_url(scheme: &str, lang: &str, article_id: &str) -> Option<Url> {
    Url::parse(&format!(
        "{scheme}://habr.com/kek/v2/articles/{article_id}/?fl={lang}&hl={lang}"
    ))
    .ok()
}

fn is_habr_lang(value: &str) -> bool {
    matches!(value, "ru" | "en")
}

fn is_article_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 12 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_company_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
