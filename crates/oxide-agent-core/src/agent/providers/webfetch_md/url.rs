use anyhow::{Context, Result, anyhow, bail};
use reqwest::Url;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use url::Host;

pub(super) fn parse_web_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("invalid URL")?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => bail!("unsupported URL scheme: {other}; only http/https are allowed"),
    }
}

pub(super) fn reject_media_url(url: &Url) -> Result<()> {
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
        bail!(
            "web_markdown is for web pages, not direct media/PDF URLs; use a media or file-specific tool instead"
        );
    }

    Ok(())
}

pub(super) fn reject_unsafe_url(url: &Url) -> Result<()> {
    match url
        .host()
        .ok_or_else(|| anyhow!("URL must include a host"))?
    {
        Host::Domain(domain) => {
            let host = domain.trim_end_matches('.').to_ascii_lowercase();
            if host == "localhost" || host.ends_with(".localhost") {
                bail!("refusing to fetch localhost URL");
            }
        }
        Host::Ipv4(ipv4) => reject_unsafe_ip(IpAddr::V4(ipv4))?,
        Host::Ipv6(ipv6) => reject_unsafe_ip(IpAddr::V6(ipv6))?,
    }

    Ok(())
}

fn reject_unsafe_ip(ip: IpAddr) -> Result<()> {
    match ip {
        IpAddr::V4(ipv4) => reject_unsafe_ipv4(ipv4),
        IpAddr::V6(ipv6) => reject_unsafe_ipv6(ipv6),
    }
}

fn reject_unsafe_ipv4(ip: Ipv4Addr) -> Result<()> {
    if ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.octets() == [169, 254, 169, 254]
    {
        bail!("refusing to fetch private, loopback, link-local, or metadata IPv4 URL");
    }

    Ok(())
}

fn reject_unsafe_ipv6(ip: Ipv6Addr) -> Result<()> {
    let first_segment = ip.segments()[0];
    let is_unique_local = (first_segment & 0xfe00) == 0xfc00;
    let is_link_local = (first_segment & 0xffc0) == 0xfe80;

    if ip.is_loopback() || ip.is_unspecified() || is_unique_local || is_link_local {
        bail!("refusing to fetch local IPv6 URL");
    }

    Ok(())
}
