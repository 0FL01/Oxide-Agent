use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::{MAX_LIST_DEPTH, MAX_LIST_ENTRIES, WORKSPACE_PREFIX};

pub(super) fn resolve_workspace_path(workspace: &Path, requested: &str) -> Result<PathBuf> {
    if requested.as_bytes().contains(&0) {
        bail!("Workspace path contains NUL byte");
    }
    let requested = requested.trim();
    if requested.is_empty() || requested == WORKSPACE_PREFIX {
        return Ok(workspace.to_path_buf());
    }

    let relative = if let Some(stripped) = requested.strip_prefix("/workspace/") {
        stripped
    } else if requested.starts_with('/') {
        bail!("Absolute sandbox paths must start with /workspace/: {requested}");
    } else {
        requested
    };

    let path = Path::new(relative);
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => bail!("Workspace path must not contain '..': {requested}"),
            Component::RootDir | Component::Prefix(_) => {
                bail!("Workspace path must be relative or under /workspace: {requested}");
            }
        }
    }
    Ok(workspace.join(normalized))
}

pub(super) fn ensure_no_symlink_escape(workspace: &Path, target: &Path) -> Result<()> {
    let relative = target.strip_prefix(workspace).map_err(|_| {
        anyhow!(
            "Resolved workspace path escaped workspace: {}",
            target.display()
        )
    })?;
    let mut cursor = workspace.to_path_buf();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            continue;
        };
        cursor.push(part);
        if cursor
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!(
                "Refusing workspace symlink path component: {}",
                cursor.display()
            );
        }
    }
    Ok(())
}

pub(super) fn list_workspace_entries(
    workspace: &Path,
    current: &Path,
    depth: usize,
    entries: &mut Vec<String>,
) -> Result<()> {
    if entries.len() >= MAX_LIST_ENTRIES {
        return Ok(());
    }
    ensure_no_symlink_escape(workspace, current)?;
    let metadata = current
        .symlink_metadata()
        .with_context(|| format!("Workspace path not found: {}", current.display()))?;
    let relative = current.strip_prefix(workspace).unwrap_or(current);
    let display = if relative.as_os_str().is_empty() {
        WORKSPACE_PREFIX.to_string()
    } else {
        format!("{WORKSPACE_PREFIX}/{}", relative.display())
    };
    if metadata.is_dir() {
        entries.push(format!("{display}/"));
        if depth >= MAX_LIST_DEPTH {
            return Ok(());
        }
        let mut children = fs::read_dir(current)?.collect::<std::result::Result<Vec<_>, _>>()?;
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            list_workspace_entries(workspace, &child.path(), depth + 1, entries)?;
            if entries.len() >= MAX_LIST_ENTRIES {
                break;
            }
        }
    } else if metadata.is_file() {
        entries.push(format!("{display} ({} bytes)", metadata.len()));
    }
    Ok(())
}

pub(super) fn dir_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total += dir_size(&entry.path())?;
        } else if metadata.is_file() {
            total += metadata.len();
        }
    }
    Ok(total)
}

pub(super) fn cleanup_old_files(path: &Path, max_age: Duration) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let cutoff = SystemTime::now()
        .checked_sub(max_age)
        .ok_or_else(|| anyhow!("Invalid cleanup age"))?;
    let mut deleted = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            deleted += cleanup_old_files(&entry.path(), max_age)?;
        } else if metadata.is_file() && metadata.modified().is_ok_and(|modified| modified < cutoff)
        {
            fs::remove_file(entry.path())?;
            deleted += 1;
        }
    }
    Ok(deleted)
}
