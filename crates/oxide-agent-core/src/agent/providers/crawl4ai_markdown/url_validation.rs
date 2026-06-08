//! URL validation, SSRF protection, wait-for normalization, and cancellation guards.

use anyhow::{Result, anyhow, bail, Context};
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio_util::sync::CancellationToken;
use url::Host;

use super::constants::MAX_WAIT_FOR_CHARS;

pub(crate) fn parse_public_http_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("invalid URL")?;
    match url.scheme() {
        "http" | "https" => {}
        other => bail!("unsupported URL scheme: {other}; only http/https are allowed"),
    }
    reject_unsafe_url_host(&url)?;
    Ok(url)
}

pub(crate) async fn dns_preflight_public(url: &Url) -> Result<()> {
    let Some(Host::Domain(domain)) = url.host() else {
        return Ok(());
    };
    let port = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL must include a known port for DNS preflight"))?;
    let host = domain.trim_end_matches('.').to_ascii_lowercase();
    let records = tokio::net::lookup_host((host.as_str(), port))
        .await
        .with_context(|| format!("dns preflight failed for host: {host}"))?;

    let mut saw_record = false;
    for addr in records {
        saw_record = true;
        reject_unsafe_ip(addr.ip())?;
    }
    if !saw_record {
        bail!("dns preflight returned no records for host: {host}");
    }
    Ok(())
}

pub(crate) fn reject_unsafe_url_host(url: &Url) -> Result<()> {
    match url
        .host()
        .ok_or_else(|| anyhow!("URL must include a host"))?
    {
        Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost" || host.ends_with(".localhost") {
                bail!("refusing to crawl localhost URL");
            }
        }
        Host::Ipv4(ipv4) => reject_unsafe_ip(IpAddr::V4(ipv4))?,
        Host::Ipv6(ipv6) => reject_unsafe_ip(IpAddr::V6(ipv6))?,
    }
    Ok(())
}

pub(crate) fn reject_unsafe_ip(ip: IpAddr) -> Result<()> {
    match ip {
        IpAddr::V4(ipv4) => reject_unsafe_ipv4(ipv4),
        IpAddr::V6(ipv6) => reject_unsafe_ipv6(ipv6),
    }
}

pub(crate) fn reject_unsafe_ipv4(ip: Ipv4Addr) -> Result<()> {
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.octets() == [169, 254, 169, 254]
    {
        bail!(
            "refusing to crawl private, loopback, link-local, documentation, or metadata IPv4 URL"
        );
    }
    Ok(())
}

pub(crate) fn reject_unsafe_ipv6(ip: Ipv6Addr) -> Result<()> {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return reject_unsafe_ipv4(mapped);
    }

    let first_segment = ip.segments()[0];
    let is_unique_local = (first_segment & 0xfe00) == 0xfc00;
    let is_link_local = (first_segment & 0xffc0) == 0xfe80;

    if ip.is_loopback() || ip.is_unspecified() || is_unique_local || is_link_local {
        bail!("refusing to crawl local IPv6 URL");
    }
    Ok(())
}

pub(crate) fn reject_media_url(url: &Url) -> Result<()> {
    let path = url.path().to_ascii_lowercase();
    if matches!(
        path.rsplit('.').next(),
        Some(
            "gif"
                | "png"
                | "jpg"
                | "jpeg"
                | "webp"
                | "bmp"
                | "svg"
                | "mp4"
                | "mov"
                | "webm"
                | "mkv"
                | "avi"
                | "mp3"
                | "wav"
                | "flac"
                | "ogg"
                | "pdf"
        )
    ) {
        bail!("crawl4ai_markdown is for web pages, not direct media/PDF URLs");
    }
    Ok(())
}

pub(crate) fn normalize_wait_for(wait_for: Option<&str>) -> Result<Option<String>> {
    let Some(selector) = wait_for.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if selector.chars().count() > MAX_WAIT_FOR_CHARS {
        bail!("wait_for selector is too long; max is {MAX_WAIT_FOR_CHARS} chars");
    }

    let lower = selector.to_ascii_lowercase();
    if lower.starts_with("js:")
        || lower.contains("function")
        || lower.contains("=>")
        || selector.contains('{')
        || selector.contains('}')
        || selector.contains(';')
        || selector.contains('\n')
        || selector.contains('\r')
    {
        bail!("wait_for accepts only CSS selectors, not JavaScript conditions");
    }

    Ok(Some(if selector.starts_with("css:") {
        selector.to_string()
    } else {
        format!("css:{selector}")
    }))
}

pub(crate) fn ensure_not_cancelled(cancellation_token: Option<&CancellationToken>) -> Result<()> {
    if cancellation_token.is_some_and(CancellationToken::is_cancelled) {
        bail!("crawl4ai_markdown cancelled before request");
    }
    Ok(())
}
