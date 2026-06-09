use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::env::{
    absolute_existing_path, absolute_maybe_existing_path, absolute_path, absolute_path_env,
    env_bool, env_parse, env_string, env_u64, env_usize, optional_path_env, parse_resolv_conf,
};
use super::image::{
    BwrapImageManifest, bwrap_rootfs_hint, default_manifest_env, load_manifest,
    resolve_image_rootfs, validate_direct_rootfs_override, validate_root_upper_dir,
    validate_rootfs,
};
use super::preflight::{bwrap_supports_disable_userns, resolve_executable};
use super::state::BwrapScopeMetadata;
use super::types::{BwrapNetworkMode, BwrapResolvConf, BwrapRootMode};
use super::{
    BWRAP_DEFAULT_BIN, BWRAP_DEFAULT_IMAGE, BWRAP_DEFAULT_MAX_OUTPUT_BYTES,
    BWRAP_DEFAULT_MAX_READ_FILE_BYTES, BWRAP_DEFAULT_NET, BWRAP_DEFAULT_ROOT_MODE,
    BWRAP_DEFAULT_TIMEOUT_SECS,
};

#[derive(Debug, Clone)]
pub(super) struct BwrapSandboxConfig {
    pub(super) bwrap_bin: PathBuf,
    pub(super) image_id: String,
    pub(super) manifest_path: Option<PathBuf>,
    pub(super) manifest_sha256: Option<String>,
    pub(super) package_manager: Option<String>,
    pub(super) rootfs: PathBuf,
    pub(super) state_dir: PathBuf,
    pub(super) lock_dir: PathBuf,
    pub(super) root_upper_dir: Option<PathBuf>,
    pub(super) pinned_system_dir: Option<PathBuf>,
    pub(super) net: BwrapNetworkMode,
    pub(super) root_mode: BwrapRootMode,
    pub(super) command_timeout: Duration,
    pub(super) lock_timeout: Duration,
    pub(super) max_output_bytes: usize,
    pub(super) max_read_file_bytes: u64,
    pub(super) allow_overlay: bool,
    pub(super) disable_nested_userns: bool,
    pub(super) resolv_conf: BwrapResolvConf,
    pub(super) default_shell: String,
    pub(super) default_workdir: String,
    pub(super) default_env: BTreeMap<String, String>,
}

impl BwrapSandboxConfig {
    pub(super) fn from_env() -> Result<Self> {
        let bwrap_bin = resolve_executable(&env_string("BWRAP_BIN", BWRAP_DEFAULT_BIN)?)?;
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
        let rootfs_override = optional_path_env("BWRAP_ROOTFS")?;
        let root_upper_dir = optional_path_env("BWRAP_ROOT_UPPER_DIR")?;
        let net = env_parse("BWRAP_NET", BWRAP_DEFAULT_NET)?;
        let root_mode = env_parse("BWRAP_ROOT_MODE", BWRAP_DEFAULT_ROOT_MODE)?;
        let allow_overlay = env_bool("BWRAP_ALLOW_OVERLAY", true)?;
        if root_mode == BwrapRootMode::OverlayRw && !allow_overlay {
            bail!("BWRAP_ALLOW_OVERLAY=false requires BWRAP_ROOT_MODE=ro.");
        }
        let command_timeout = Duration::from_secs(env_u64(
            "BWRAP_COMMAND_TIMEOUT_SECS",
            std::env::var("SANDBOX_EXEC_TIMEOUT_SECS")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(BWRAP_DEFAULT_TIMEOUT_SECS),
        )?);
        let lock_timeout = Duration::from_secs(env_u64(
            "BWRAP_RECREATE_LOCK_TIMEOUT_SECS",
            command_timeout.as_secs().saturating_add(5),
        )?);
        let max_output_bytes = env_usize("BWRAP_MAX_OUTPUT_BYTES", BWRAP_DEFAULT_MAX_OUTPUT_BYTES)?;
        let max_read_file_bytes = env_u64(
            "BWRAP_MAX_READ_FILE_BYTES",
            BWRAP_DEFAULT_MAX_READ_FILE_BYTES,
        )?;
        let disable_nested_userns = env_bool("BWRAP_DISABLE_NESTED_USERNS", true)?;
        let resolv_conf = parse_resolv_conf()?;

        let (manifest, manifest_path, manifest_sha256, rootfs) =
            if let Some(rootfs) = rootfs_override {
                validate_direct_rootfs_override(&rootfs)?;
                let manifest_path = rootfs.parent().map(|parent| parent.join("image.json"));
                let (manifest, loaded_manifest_path, manifest_sha256) =
                    match manifest_path.as_ref().filter(|path| path.is_file()) {
                        Some(path) => {
                            let (manifest, manifest_sha256) = load_manifest(path)?;
                            (manifest, Some(path.clone()), manifest_sha256)
                        }
                        None => (BwrapImageManifest::fallback(&image_id), None, None),
                    };
                (manifest, loaded_manifest_path, manifest_sha256, rootfs)
            } else {
                let image_dir = image_store.join(&image_id);
                let manifest_path = image_dir.join("image.json");
                if !manifest_path.is_file() {
                    bail!(
                        "Bwrap image manifest not found at {}. {}",
                        manifest_path.display(),
                        bwrap_rootfs_hint()
                    );
                }
                let (manifest, manifest_sha256) = load_manifest(&manifest_path)?;
                let rootfs = resolve_image_rootfs(&image_dir, &manifest.rootfs)?;
                (manifest, Some(manifest_path), manifest_sha256, rootfs)
            };

        let mut default_env = default_manifest_env();
        default_env.extend(manifest.default_env.clone());

        Ok(Self {
            bwrap_bin,
            image_id: manifest.id.clone(),
            manifest_path: manifest_path.map(absolute_path).transpose()?,
            manifest_sha256,
            package_manager: manifest.package_manager.clone(),
            rootfs: absolute_maybe_existing_path(&rootfs)?,
            state_dir,
            lock_dir,
            root_upper_dir,
            pinned_system_dir: None,
            net,
            root_mode,
            command_timeout,
            lock_timeout,
            max_output_bytes,
            max_read_file_bytes,
            allow_overlay,
            disable_nested_userns,
            resolv_conf,
            default_shell: manifest.default_shell,
            default_workdir: manifest.default_workdir,
            default_env,
        })
    }

    pub(super) fn apply_scope_pin(&mut self, metadata: &BwrapScopeMetadata) -> Result<()> {
        if metadata.backend != "bwrap" {
            bail!(
                "Invalid bwrap metadata backend '{}' in existing scope {}",
                metadata.backend,
                metadata.scope_name
            );
        }
        self.image_id.clone_from(&metadata.image_id);
        self.manifest_path = metadata.image_manifest_path.as_deref().map(PathBuf::from);
        self.manifest_sha256 = metadata.image_manifest_sha256.clone();
        self.package_manager = metadata.package_manager.clone();
        self.rootfs = absolute_existing_path(Path::new(&metadata.rootfs))?;
        self.pinned_system_dir = metadata.system_dir.as_deref().map(PathBuf::from);
        self.root_mode = metadata.root_mode;
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.root_mode == BwrapRootMode::OverlayRw && !self.allow_overlay {
            bail!("BWRAP_ALLOW_OVERLAY=false requires BWRAP_ROOT_MODE=ro.");
        }
        if self.command_timeout.is_zero() {
            bail!("BWRAP_COMMAND_TIMEOUT_SECS must be greater than zero.");
        }
        if self.lock_timeout.is_zero() {
            bail!("BWRAP_RECREATE_LOCK_TIMEOUT_SECS must be greater than zero.");
        }
        if self.max_output_bytes == 0 {
            bail!("BWRAP_MAX_OUTPUT_BYTES must be greater than zero.");
        }
        if self.max_read_file_bytes == 0 {
            bail!("BWRAP_MAX_READ_FILE_BYTES must be greater than zero.");
        }
        if self.disable_nested_userns && !bwrap_supports_disable_userns(&self.bwrap_bin)? {
            bail!(
                "BWRAP_DISABLE_NESTED_USERNS=true, but {} does not support --disable-userns. Upgrade bubblewrap or set BWRAP_DISABLE_NESTED_USERNS=false for development only.",
                self.bwrap_bin.display()
            );
        }
        validate_rootfs(self)?;
        if let Some(root_upper_dir) = &self.root_upper_dir {
            validate_root_upper_dir(root_upper_dir, &self.rootfs)?;
        }
        Ok(())
    }
}
