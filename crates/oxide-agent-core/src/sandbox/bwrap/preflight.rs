use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

use super::BWRAP_DEFAULT_BIN;
use super::bootstrap::BwrapImageBootstrapConfig;
use super::env::{absolute_existing_path, env_bool, env_string};

pub(crate) fn preflight_from_env() -> Result<()> {
    let bwrap_bin = resolve_executable(&env_string("BWRAP_BIN", BWRAP_DEFAULT_BIN)?)?;
    let help = bwrap_help_stdout(&bwrap_bin)?;
    if env_bool("BWRAP_DISABLE_NESTED_USERNS", true)? && !help.contains("--disable-userns") {
        bail!(
            "BWRAP_DISABLE_NESTED_USERNS=true, but {} does not support --disable-userns. Upgrade bubblewrap or set BWRAP_DISABLE_NESTED_USERNS=false for development only.",
            bwrap_bin.display()
        );
    }
    Ok(())
}

pub(crate) async fn bootstrap_image_from_env() -> Result<()> {
    BwrapImageBootstrapConfig::from_env()?
        .bootstrap_if_needed()
        .await
}

pub(super) fn bwrap_supports_disable_userns(bwrap_bin: &Path) -> Result<bool> {
    Ok(bwrap_help_stdout(bwrap_bin)?.contains("--disable-userns"))
}

pub(super) fn resolve_executable(value: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.components().count() > 1 || path.is_absolute() {
        if path.is_file() {
            return absolute_existing_path(&path);
        }
        bail!(
            "Bwrap backend selected, but BWRAP_BIN='{}' was not found or is not executable. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker.",
            value
        );
    }
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(value);
        if candidate.is_file() {
            return absolute_existing_path(&candidate);
        }
    }
    bail!(
        "Bwrap backend selected, but BWRAP_BIN='{value}' was not found or is not executable. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker."
    )
}

fn bwrap_help_stdout(bwrap_bin: &Path) -> Result<String> {
    let output = std::process::Command::new(bwrap_bin)
        .arg("--help")
        .output()
        .with_context(|| {
            format!(
                "Bwrap backend selected, but BWRAP_BIN='{}' was not found or is not executable. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker.",
                bwrap_bin.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "Bwrap backend selected, but '{}' failed to run --help. Install bubblewrap, set BWRAP_BIN=/path/to/bwrap, or choose SANDBOX_BACKEND=docker|broker.",
            bwrap_bin.display()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
