use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use super::types::BwrapResolvConf;

pub(super) fn env_string(key: &str, default: &str) -> Result<String> {
    Ok(std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string()))
}

pub(super) fn optional_string_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(super) fn env_u64(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value
            .parse()
            .with_context(|| format!("{key} must be a positive integer")),
        _ => Ok(default),
    }
}

pub(super) fn env_usize(key: &str, default: usize) -> Result<usize> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value
            .parse()
            .with_context(|| format!("{key} must be a positive integer")),
        _ => Ok(default),
    }
}

pub(super) fn env_bool(key: &str, default: bool) -> Result<bool> {
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(true),
            "false" | "0" => Ok(false),
            invalid => Err(anyhow!(
                "{key} must be true, false, 1, or 0; got '{invalid}'"
            )),
        },
        _ => Ok(default),
    }
}

pub(super) fn env_parse<T>(key: &str, default: T) -> Result<T>
where
    T: FromStr<Err = anyhow::Error>,
{
    match std::env::var(key) {
        Ok(value) if !value.trim().is_empty() => value.parse(),
        _ => Ok(default),
    }
}

pub(super) fn absolute_path_env(key: &str, default: PathBuf) -> Result<PathBuf> {
    match std::env::var_os(key) {
        Some(value) if !value.is_empty() => absolute_path(value),
        _ => absolute_path(default),
    }
}

pub(super) fn optional_path_env(key: &str) -> Result<Option<PathBuf>> {
    match std::env::var_os(key) {
        Some(value) if !value.is_empty() => absolute_path(value).map(Some),
        _ => Ok(None),
    }
}

pub(super) fn absolute_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let path = path.as_ref();
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub(super) fn absolute_existing_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    absolute
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", absolute.display()))
}

pub(super) fn absolute_maybe_existing_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let absolute = absolute_path(path)?;
    if absolute.exists() {
        return absolute
            .canonicalize()
            .with_context(|| format!("Path does not exist: {}", absolute.display()));
    }
    Ok(absolute)
}

pub(super) fn parse_resolv_conf() -> Result<BwrapResolvConf> {
    let value = env_string("BWRAP_RESOLV_CONF", "auto")?;
    match value.trim() {
        "auto" => Ok(BwrapResolvConf::Auto),
        "none" => Ok(BwrapResolvConf::None),
        path => {
            let path = absolute_path(path)?;
            validate_resolv_conf_path(&path)?;
            Ok(BwrapResolvConf::Path(path))
        }
    }
}

fn validate_resolv_conf_path(path: &Path) -> Result<()> {
    let metadata = path
        .symlink_metadata()
        .with_context(|| format!("BWRAP_RESOLV_CONF path does not exist: {}", path.display()))?;
    if metadata.file_type().is_symlink() {
        bail!(
            "BWRAP_RESOLV_CONF must not be a symlink: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        bail!(
            "BWRAP_RESOLV_CONF must be a regular file: {}",
            path.display()
        );
    }
    Ok(())
}
