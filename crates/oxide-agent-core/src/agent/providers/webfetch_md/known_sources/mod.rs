//! Known source fast paths for URLs whose Markdown can be fetched directly.

pub(super) mod github_gist;
pub(super) mod pypi;
mod repo_hosts;
pub(super) mod rust_packages;

use reqwest::Url;

pub(super) enum KnownMarkdownSource {
    DirectReadme {
        source_url: Url,
        fetch_url: Url,
        mode: &'static str,
    },
    GitHubReadme {
        source_url: Url,
        api_url: Url,
        owner: String,
        repo: String,
        mode: &'static str,
    },
    CrateReadme {
        source_url: Url,
        metadata_url: Url,
        crate_name: String,
        version: Option<String>,
        mode: &'static str,
    },
    PypiProject {
        source_url: Url,
        metadata_url: Url,
        package_name: String,
        mode: &'static str,
    },
    GitHubGist {
        source_url: Url,
        api_url: Url,
        owner: String,
        gist_id: String,
        comment_id: Option<String>,
        mode: &'static str,
    },
    HuggingFaceBlog {
        source_url: Url,
        fetch_url: Url,
        mode: &'static str,
    },
    HuggingFaceTree {
        source_url: Url,
        api_url: Url,
        repo_id: String,
        revision: String,
        tree_path: Option<String>,
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

    pub(super) fn github_readme(
        source_url: Url,
        api_url: Url,
        owner: String,
        repo: String,
        mode: &'static str,
    ) -> Self {
        Self::GitHubReadme {
            source_url,
            api_url,
            owner,
            repo,
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

    pub(super) fn pypi_project(
        source_url: Url,
        metadata_url: Url,
        package_name: String,
        mode: &'static str,
    ) -> Self {
        Self::PypiProject {
            source_url,
            metadata_url,
            package_name,
            mode,
        }
    }

    pub(super) fn github_gist(
        source_url: Url,
        api_url: Url,
        owner: String,
        gist_id: String,
        comment_id: Option<String>,
        mode: &'static str,
    ) -> Self {
        Self::GitHubGist {
            source_url,
            api_url,
            owner,
            gist_id,
            comment_id,
            mode,
        }
    }

    pub(super) fn huggingface_blog(source_url: Url, fetch_url: Url, mode: &'static str) -> Self {
        Self::HuggingFaceBlog {
            source_url,
            fetch_url,
            mode,
        }
    }

    pub(super) fn huggingface_tree(
        source_url: Url,
        api_url: Url,
        repo_id: String,
        revision: String,
        tree_path: Option<String>,
        mode: &'static str,
    ) -> Self {
        Self::HuggingFaceTree {
            source_url,
            api_url,
            repo_id,
            revision,
            tree_path,
            mode,
        }
    }

    pub(super) fn source_url(&self) -> &Url {
        match self {
            Self::DirectReadme { source_url, .. } => source_url,
            Self::GitHubReadme { source_url, .. } => source_url,
            Self::CrateReadme { source_url, .. } => source_url,
            Self::PypiProject { source_url, .. } => source_url,
            Self::GitHubGist { source_url, .. } => source_url,
            Self::HuggingFaceBlog { source_url, .. } => source_url,
            Self::HuggingFaceTree { source_url, .. } => source_url,
        }
    }

    pub(super) fn fetch_url(&self) -> &Url {
        match self {
            Self::DirectReadme { fetch_url, .. } => fetch_url,
            Self::GitHubReadme { api_url, .. } => api_url,
            Self::CrateReadme { metadata_url, .. } => metadata_url,
            Self::PypiProject { metadata_url, .. } => metadata_url,
            Self::GitHubGist { api_url, .. } => api_url,
            Self::HuggingFaceBlog { fetch_url, .. } => fetch_url,
            Self::HuggingFaceTree { api_url, .. } => api_url,
        }
    }

    pub(super) fn mode(&self) -> &'static str {
        match self {
            Self::DirectReadme { mode, .. } => mode,
            Self::GitHubReadme { mode, .. } => mode,
            Self::CrateReadme { mode, .. } => mode,
            Self::PypiProject { mode, .. } => mode,
            Self::GitHubGist { mode, .. } => mode,
            Self::HuggingFaceBlog { mode, .. } => mode,
            Self::HuggingFaceTree { mode, .. } => mode,
        }
    }

    pub(super) fn is_authoritative(&self) -> bool {
        match self {
            Self::GitHubReadme { .. } | Self::GitHubGist { .. } => true,
            Self::DirectReadme { mode, .. } => mode.starts_with("github_"),
            Self::CrateReadme { .. }
            | Self::PypiProject { .. }
            | Self::HuggingFaceBlog { .. }
            | Self::HuggingFaceTree { .. } => false,
        }
    }
}

pub(super) fn classify(url: &Url) -> Option<KnownMarkdownSource> {
    github_gist::classify(url)
        .or_else(|| repo_hosts::classify(url))
        .or_else(|| rust_packages::classify(url))
        .or_else(|| pypi::classify(url))
}
