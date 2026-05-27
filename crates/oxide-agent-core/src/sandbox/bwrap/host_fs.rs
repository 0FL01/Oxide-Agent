use anyhow::{bail, Context, Result};
use std::fs;
use std::path::Path;

pub(super) fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)
            .with_context(|| format!("Failed to remove directory {}", path.display()))?;
    }
    Ok(())
}

pub(super) fn ensure_configured_dir(env_key: &str, path: &Path) -> Result<()> {
    if !path.exists() {
        return fs::create_dir_all(path)
            .with_context(|| format!("Failed to create {env_key} directory {}", path.display()));
    }

    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("Failed to inspect {env_key} path {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!("{env_key} must not be a symlink: {}", path.display());
    }
    if !metadata.is_dir() {
        bail!("{env_key} must be a directory: {}", path.display());
    }
    Ok(())
}
