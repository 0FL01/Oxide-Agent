use anyhow::{anyhow, bail, Context, Result};
use chrono::Utc;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::info;
use uuid::Uuid;

use super::env::{
    absolute_path, absolute_path_env, env_parse, env_string, optional_path_env, optional_string_env,
};
use super::host_fs::ensure_configured_dir;
use super::image::{default_manifest_env, host_arch};
use super::{ScopeLock, BWRAP_DEFAULT_IMAGE, WORKSPACE_PREFIX};

const BWRAP_IMAGE_BOOTSTRAP_OFF: &str = "off";
const BWRAP_IMAGE_BOOTSTRAP_DOWNLOAD: &str = "download";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BwrapImageBootstrapMode {
    Off,
    Download,
}

impl fmt::Display for BwrapImageBootstrapMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Off => BWRAP_IMAGE_BOOTSTRAP_OFF,
            Self::Download => BWRAP_IMAGE_BOOTSTRAP_DOWNLOAD,
        })
    }
}

impl FromStr for BwrapImageBootstrapMode {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            BWRAP_IMAGE_BOOTSTRAP_OFF => Ok(Self::Off),
            BWRAP_IMAGE_BOOTSTRAP_DOWNLOAD => Ok(Self::Download),
            invalid => Err(anyhow!(
                "Invalid BWRAP_IMAGE_BOOTSTRAP='{invalid}'. Valid values: off, download."
            )),
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct BwrapImageBootstrapConfig {
    mode: BwrapImageBootstrapMode,
    image_id: String,
    image_store: PathBuf,
    lock_dir: PathBuf,
    rootfs_override: Option<PathBuf>,
    url: Option<String>,
    sha256: Option<String>,
    package_manager: Option<String>,
}

impl BwrapImageBootstrapConfig {
    pub(super) fn from_env() -> Result<Self> {
        let image_id = env_string("BWRAP_IMAGE", BWRAP_DEFAULT_IMAGE)?;
        let state_root = absolute_path(".oxide/sandbox")?;
        let image_store = absolute_path_env("BWRAP_IMAGE_STORE", state_root.join("images"))?;
        let state_dir = absolute_path_env("BWRAP_STATE_DIR", state_root.join("scopes"))?;
        let lock_dir = absolute_path_env(
            "BWRAP_LOCK_DIR",
            state_dir
                .parent()
                .map_or_else(|| state_root.join("locks"), |parent| parent.join("locks")),
        )?;
        Ok(Self {
            mode: env_parse("BWRAP_IMAGE_BOOTSTRAP", BwrapImageBootstrapMode::Off)?,
            image_id,
            image_store,
            lock_dir,
            rootfs_override: optional_path_env("BWRAP_ROOTFS")?,
            url: optional_string_env("BWRAP_IMAGE_URL"),
            sha256: optional_string_env("BWRAP_IMAGE_SHA256"),
            package_manager: optional_string_env("BWRAP_IMAGE_PACKAGE_MANAGER"),
        })
    }

    pub(super) async fn bootstrap_if_needed(&self) -> Result<()> {
        if self.mode == BwrapImageBootstrapMode::Off || self.rootfs_override.is_some() {
            return Ok(());
        }
        validate_bootstrap_image_id(&self.image_id)?;
        ensure_configured_dir("BWRAP_IMAGE_STORE", &self.image_store)?;
        let image_dir = self.image_dir();
        if image_dir.join("image.json").is_file() {
            return Ok(());
        }
        let _lock = lock_image_bootstrap(&self.lock_dir, &self.image_id)?;
        if image_dir.join("image.json").is_file() {
            return Ok(());
        }
        if image_dir.exists() {
            bail!(
                "Bwrap image directory {} exists but image.json is missing. Remove the partial directory or choose another BWRAP_IMAGE.",
                image_dir.display()
            );
        }
        match self.mode {
            BwrapImageBootstrapMode::Off => Ok(()),
            BwrapImageBootstrapMode::Download => self.download_image(&image_dir).await,
        }
    }

    fn image_dir(&self) -> PathBuf {
        self.image_store.join(&self.image_id)
    }

    async fn download_image(&self, image_dir: &Path) -> Result<()> {
        let url = self
            .url
            .as_deref()
            .ok_or_else(|| anyhow!("BWRAP_IMAGE_BOOTSTRAP=download requires BWRAP_IMAGE_URL."))?;
        let expected_sha256 = self.sha256.as_deref().ok_or_else(|| {
            anyhow!("BWRAP_IMAGE_BOOTSTRAP=download requires BWRAP_IMAGE_SHA256.")
        })?;
        let expected_sha256 = normalize_sha256(expected_sha256)?;
        let staging_dir =
            self.image_store
                .join(format!(".{}.bootstrap-{}", self.image_id, Uuid::new_v4()));
        let result = self
            .download_image_to_staging(url, &expected_sha256, &staging_dir, image_dir)
            .await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging_dir);
        }
        result
    }

    async fn download_image_to_staging(
        &self,
        url: &str,
        expected_sha256: &str,
        staging_dir: &Path,
        image_dir: &Path,
    ) -> Result<()> {
        fs::create_dir_all(staging_dir).with_context(|| {
            format!(
                "Failed to create bwrap image bootstrap staging dir {}",
                staging_dir.display()
            )
        })?;
        let tarball = staging_dir.join("rootfs.tarball");
        let actual_sha256 = download_bootstrap_tarball(url, &tarball).await?;
        if actual_sha256 != expected_sha256 {
            bail!(
                "Checksum mismatch for BWRAP_IMAGE_URL={url}. expected: {expected_sha256}; actual: {actual_sha256}"
            );
        }
        let rootfs = staging_dir.join("rootfs");
        fs::create_dir_all(&rootfs)
            .with_context(|| format!("Failed to create rootfs dir {}", rootfs.display()))?;
        extract_bootstrap_tarball(&tarball, &rootfs, url)?;
        prepare_bootstrap_rootfs(&rootfs)?;
        write_bootstrap_image_metadata(self, staging_dir, url, expected_sha256)?;
        fs::remove_file(&tarball)
            .with_context(|| format!("Failed to remove temporary tarball {}", tarball.display()))?;
        fs::rename(staging_dir, image_dir).with_context(|| {
            format!(
                "Failed to publish bwrap image {} from {}",
                image_dir.display(),
                staging_dir.display()
            )
        })?;
        info!(
            image_id = %self.image_id,
            image_dir = %image_dir.display(),
            "Bootstrapped bwrap rootfs image"
        );
        Ok(())
    }
}

fn validate_bootstrap_image_id(image_id: &str) -> Result<()> {
    if image_id.trim().is_empty()
        || image_id.contains('/')
        || image_id.contains('\\')
        || image_id == "."
        || image_id == ".."
    {
        bail!("BWRAP_IMAGE_BOOTSTRAP=download requires a simple BWRAP_IMAGE directory name.");
    }
    Ok(())
}

fn normalize_sha256(value: &str) -> Result<String> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.len() != 64 || !normalized.chars().all(|char| char.is_ascii_hexdigit()) {
        bail!("BWRAP_IMAGE_SHA256 must be a 64-character hex SHA-256 digest.");
    }
    Ok(normalized)
}

fn lock_image_bootstrap(lock_dir: &Path, image_id: &str) -> Result<ScopeLock> {
    ensure_configured_dir("BWRAP_LOCK_DIR", lock_dir)?;
    let lock_path = lock_dir.join(format!("image-bootstrap-{image_id}.lock"));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Failed to open bwrap image lock {}", lock_path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("Failed to lock bwrap image {image_id}"))?;
    Ok(ScopeLock { file })
}

async fn download_bootstrap_tarball(url: &str, destination: &Path) -> Result<String> {
    if url.trim_start().starts_with("file://") {
        return copy_bootstrap_tarball_file(url, destination);
    }
    let parsed = url::Url::parse(url).with_context(|| format!("Invalid BWRAP_IMAGE_URL={url}"))?;
    match parsed.scheme() {
        "http" | "https" => download_bootstrap_tarball_http(url, destination).await,
        scheme => bail!("BWRAP_IMAGE_URL must use http, https, or file; got '{scheme}'."),
    }
}

fn copy_bootstrap_tarball_file(url: &str, destination: &Path) -> Result<String> {
    let parsed = url::Url::parse(url).with_context(|| format!("Invalid BWRAP_IMAGE_URL={url}"))?;
    let source = parsed
        .to_file_path()
        .map_err(|()| anyhow!("Invalid file URL for BWRAP_IMAGE_URL={url}"))?;
    validate_bootstrap_source_file(&source)?;
    copy_file_with_sha256(&source, destination)
}

async fn download_bootstrap_tarball_http(url: &str, destination: &Path) -> Result<String> {
    let mut response = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to download BWRAP_IMAGE_URL={url}"))?
        .error_for_status()
        .with_context(|| format!("BWRAP_IMAGE_URL returned an error status: {url}"))?;
    let mut file = File::create(destination)
        .with_context(|| format!("Failed to create download file {}", destination.display()))?;
    let mut hasher = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("Failed to read BWRAP_IMAGE_URL={url}"))?
    {
        file.write_all(&chunk)
            .with_context(|| format!("Failed to write download file {}", destination.display()))?;
        hasher.update(&chunk);
    }
    file.flush()
        .with_context(|| format!("Failed to flush download file {}", destination.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn validate_bootstrap_source_file(path: &Path) -> Result<()> {
    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("BWRAP_IMAGE_URL source file not found: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "BWRAP_IMAGE_URL file source must not be a symlink: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!(
            "BWRAP_IMAGE_URL file source must be a regular file: {}",
            path.display()
        );
    }
    Ok(())
}

fn copy_file_with_sha256(source: &Path, destination: &Path) -> Result<String> {
    let mut input = File::open(source)
        .with_context(|| format!("Failed to open source tarball {}", source.display()))?;
    let mut output = File::create(destination)
        .with_context(|| format!("Failed to create download file {}", destination.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .with_context(|| format!("Failed to read source tarball {}", source.display()))?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .with_context(|| format!("Failed to write download file {}", destination.display()))?;
        hasher.update(&buffer[..read]);
    }
    output
        .flush()
        .with_context(|| format!("Failed to flush download file {}", destination.display()))?;
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_bootstrap_tarball(tarball: &Path, rootfs: &Path, url: &str) -> Result<()> {
    let mut command = std::process::Command::new("tar");
    command.arg("--extract");
    if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
        command.arg("--gzip");
    } else if url.ends_with(".tar.xz") || url.ends_with(".txz") {
        command.arg("--xz");
    }
    let output = command
        .arg("--file")
        .arg(tarball)
        .arg("--directory")
        .arg(rootfs)
        .arg("--numeric-owner")
        .output()
        .with_context(|| "Failed to run tar while bootstrapping bwrap image. Install tar or disable BWRAP_IMAGE_BOOTSTRAP.")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to extract bwrap rootfs tarball with tar: {stderr}");
    }
    Ok(())
}

fn prepare_bootstrap_rootfs(rootfs: &Path) -> Result<()> {
    for directory in ["proc", "dev", "workspace"] {
        fs::create_dir_all(rootfs.join(directory))
            .with_context(|| format!("Failed to create required rootfs /{directory} directory"))?;
    }
    let tmp = rootfs.join("tmp");
    fs::create_dir_all(&tmp).context("Failed to create required rootfs /tmp directory")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o1777))
            .context("Failed to chmod rootfs /tmp")?;
    }
    if !rootfs.join("bin/sh").is_file() {
        bail!("Downloaded bwrap rootfs is missing /bin/sh.");
    }
    Ok(())
}

fn write_bootstrap_image_metadata(
    config: &BwrapImageBootstrapConfig,
    image_dir: &Path,
    source_url: &str,
    source_sha256: &str,
) -> Result<()> {
    let created_at = Utc::now().to_rfc3339();
    let manifest = serde_json::json!({
        "schema_version": 1,
        "id": config.image_id.clone(),
        "arch": host_arch(),
        "rootfs": "rootfs",
        "default_shell": "/bin/sh",
        "default_workdir": WORKSPACE_PREFIX,
        "package_manager": config.package_manager.clone(),
        "default_env": default_manifest_env(),
        "provenance": {
            "builder": "oxide-agent bwrap image bootstrap",
            "source": source_url,
            "source_sha256": source_sha256,
            "created_at": created_at,
        }
    });
    let provenance = serde_json::json!({
        "builder": "oxide-agent bwrap image bootstrap",
        "source": source_url,
        "source_sha256": source_sha256,
        "created_at": created_at,
        "host_arch": host_arch(),
    });
    write_bootstrap_json_files(image_dir, &manifest, &provenance)
}

fn write_bootstrap_json_files(
    image_dir: &Path,
    manifest: &serde_json::Value,
    provenance: &serde_json::Value,
) -> Result<()> {
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    let provenance_bytes = serde_json::to_vec_pretty(provenance)?;
    fs::write(image_dir.join("image.json"), &manifest_bytes)
        .with_context(|| format!("Failed to write {}", image_dir.join("image.json").display()))?;
    fs::write(image_dir.join("provenance.json"), &provenance_bytes).with_context(|| {
        format!(
            "Failed to write {}",
            image_dir.join("provenance.json").display()
        )
    })?;
    let checksums = format!(
        "{:x}  image.json\n{:x}  provenance.json\n",
        Sha256::digest(&manifest_bytes),
        Sha256::digest(&provenance_bytes)
    );
    fs::write(image_dir.join("checksums.txt"), checksums).with_context(|| {
        format!(
            "Failed to write {}",
            image_dir.join("checksums.txt").display()
        )
    })
}
