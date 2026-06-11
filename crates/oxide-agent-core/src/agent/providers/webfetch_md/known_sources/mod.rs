//! Known source fast paths for URLs whose Markdown can be fetched directly.

mod repo_hosts;
pub(super) mod rust_packages;

use reqwest::Url;

pub(super) enum KnownMarkdownSource {
    DirectReadme {
        source_url: Url,
        fetch_url: Url,
        mode: &'static str,
    },
    CrateReadme {
        source_url: Url,
        metadata_url: Url,
        crate_name: String,
        version: Option<String>,
        mode: &'static str,
    },
}

impl KnownMarkdownSource {
    pub(super) fn direct_readme(source_url: Url, fetch_url: Url, mode: &'static str) -> Self {
        Self::DirectReadme {
            source_url,
            fetch_url,
            mode,
        }
    }

    pub(super) fn crate_readme(
        source_url: Url,
        metadata_url: Url,
        crate_name: String,
        version: Option<String>,
        mode: &'static str,
    ) -> Self {
        Self::CrateReadme {
            source_url,
            metadata_url,
            crate_name,
            version,
            mode,
        }
    }

    pub(super) fn source_url(&self) -> &Url {
        match self {
            Self::DirectReadme { source_url, .. } => source_url,
            Self::CrateReadme { source_url, .. } => source_url,
        }
    }

    pub(super) fn fetch_url(&self) -> &Url {
        match self {
            Self::DirectReadme { fetch_url, .. } => fetch_url,
            Self::CrateReadme { metadata_url, .. } => metadata_url,
        }
    }

    pub(super) fn mode(&self) -> &'static str {
        match self {
            Self::DirectReadme { mode, .. } => mode,
            Self::CrateReadme { mode, .. } => mode,
        }
    }
}

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    repo_hosts::classify(url).or_else(|| rust_packages::classify(url))
}
