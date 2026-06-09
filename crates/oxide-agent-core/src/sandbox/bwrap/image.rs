use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use super::WORKSPACE_PREFIX;
use super::config::BwrapSandboxConfig;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct BwrapImageManifest {
    pub(super) schema_version: u32,
    pub(super) id: String,
    pub(super) arch: String,
    #[serde(default)]
    pub(super) package_manager: Option<String>,
    #[serde(default = "default_manifest_rootfs")]
    pub(super) rootfs: String,
    #[serde(default = "default_manifest_shell")]
    pub(super) default_shell: String,
    #[serde(default = "default_manifest_workdir")]
    pub(super) default_workdir: String,
    #[serde(default = "default_manifest_env")]
    pub(super) default_env: BTreeMap<String, String>,
}

impl BwrapImageManifest {
    pub(super) fn fallback(image_id: &str) -> Self {
        Self {
            schema_version: 1,
            id: image_id.to_string(),
            arch: host_arch().to_string(),
            package_manager: None,
            rootfs: ".".to_string(),
            default_shell: default_manifest_shell(),
            default_workdir: default_manifest_workdir(),
            default_env: default_manifest_env(),
        }
    }
}

pub(super) fn default_manifest_rootfs() -> String {
    "rootfs".to_string()
}

pub(super) fn default_manifest_shell() -> String {
    "/bin/sh".to_string()
}

pub(super) fn default_manifest_workdir() -> String {
    WORKSPACE_PREFIX.to_string()
}

pub(super) fn default_manifest_env() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("HOME".to_string(), WORKSPACE_PREFIX.to_string()),
        (
            "PATH".to_string(),
            "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".to_string(),
        ),
        ("LANG".to_string(), "C.UTF-8".to_string()),
        ("TMPDIR".to_string(), "/tmp".to_string()),
    ])
}

pub(super) fn load_manifest(path: &Path) -> Result<(BwrapImageManifest, Option<String>)> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("Failed to read bwrap manifest {}", path.display()))?;
    let manifest: BwrapImageManifest = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid bwrap image manifest {}", path.display()))?;
    if manifest.schema_version != 1 {
        bail!(
            "Invalid bwrap image manifest {}: schema_version must be 1",
            path.display()
        );
    }
    let rootfs_path = Path::new(&manifest.rootfs);
    if rootfs_path.is_absolute() {
        bail!(
            "Invalid bwrap image manifest {}: rootfs must be relative",
            path.display()
        );
    }
    if manifest.rootfs.trim().is_empty()
        || rootfs_path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        bail!(
            "Invalid bwrap image manifest {}: rootfs must stay under the image directory",
            path.display()
        );
    }
    if manifest.default_workdir != WORKSPACE_PREFIX {
        bail!(
            "Invalid bwrap image manifest {}: default_workdir must be /workspace for MVP",
            path.display()
        );
    }
    if manifest.arch != host_arch() {
        bail!(
            "Invalid bwrap image manifest {}: arch '{}' does not match host arch '{}'",
            path.display(),
            manifest.arch,
            host_arch()
        );
    }
    Ok((manifest, Some(format!("{:x}", Sha256::digest(&bytes)))))
}

pub(super) fn validate_rootfs(config: &BwrapSandboxConfig) -> Result<()> {
    if !config.rootfs.is_dir() {
        bail!(
            "Bwrap backend selected, but rootfs not found at {}. {}",
            config.rootfs.display(),
            bwrap_rootfs_hint()
        );
    }
    for required in ["proc", "dev", "tmp", "workspace"] {
        let path = config.rootfs.join(required);
        if !path.is_dir() {
            bail!(
                "Bwrap rootfs {} is missing required /{} directory.",
                config.rootfs.display(),
                required
            );
        }
    }
    let shell_path = config
        .default_shell
        .strip_prefix('/')
        .ok_or_else(|| anyhow!("Bwrap manifest default_shell must be absolute"))?;
    if !config.rootfs.join(shell_path).is_file() {
        bail!(
            "Bwrap rootfs {} does not contain default shell {}.",
            config.rootfs.display(),
            config.default_shell
        );
    }
    Ok(())
}

pub(super) fn validate_root_upper_dir(root_upper_dir: &Path, rootfs: &Path) -> Result<()> {
    if root_upper_dir.exists() && !root_upper_dir.is_dir() {
        bail!(
            "BWRAP_ROOT_UPPER_DIR must be a directory: {}",
            root_upper_dir.display()
        );
    }
    if root_upper_dir
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!(
            "BWRAP_ROOT_UPPER_DIR must not be a symlink: {}",
            root_upper_dir.display()
        );
    }
    let root_upper_dir = root_upper_dir
        .parent()
        .unwrap_or(root_upper_dir)
        .canonicalize()
        .unwrap_or_else(|_| root_upper_dir.to_path_buf());
    if root_upper_dir.starts_with(rootfs) {
        bail!(
            "BWRAP_ROOT_UPPER_DIR must not be inside the bwrap rootfs image: {}",
            root_upper_dir.display()
        );
    }
    Ok(())
}

pub(super) fn validate_direct_rootfs_override(rootfs: &Path) -> Result<()> {
    if rootfs
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        bail!("BWRAP_ROOTFS must not be a symlink: {}", rootfs.display());
    }
    Ok(())
}

pub(super) fn bwrap_rootfs_hint() -> String {
    let mut hint =
        "Run scripts/build-bwrap-rootfs-debian.sh, set BWRAP_IMAGE_BOOTSTRAP=download, or set BWRAP_IMAGE/BWRAP_ROOTFS.".to_string();
    if std::env::var_os("SANDBOX_IMAGE").is_some() {
        hint.push_str(" SANDBOX_IMAGE is Docker-only and is ignored by SANDBOX_BACKEND=bwrap.");
    }
    hint
}

pub(super) fn resolve_image_rootfs(image_dir: &Path, rootfs: &str) -> Result<PathBuf> {
    let image_dir = image_dir.canonicalize().with_context(|| {
        format!(
            "Failed to canonicalize bwrap image directory {}",
            image_dir.display()
        )
    })?;
    let rootfs = image_dir.join(rootfs);
    let canonical_rootfs = rootfs.canonicalize().with_context(|| {
        format!(
            "Bwrap backend selected, but rootfs not found at {}. {}",
            rootfs.display(),
            bwrap_rootfs_hint()
        )
    })?;
    if !canonical_rootfs.starts_with(&image_dir) {
        bail!(
            "Bwrap image rootfs {} resolves outside image directory {}. Refusing unsafe rootfs symlink.",
            canonical_rootfs.display(),
            image_dir.display()
        );
    }
    Ok(canonical_rootfs)
}

pub(super) fn host_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    }
}
