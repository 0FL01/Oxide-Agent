use anyhow::{anyhow, bail, Context, Result};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;
use tracing::debug;
use uuid::Uuid;

use super::process::{await_capped_output, cleanup_bwrap_child, read_capped_counted};
use super::types::{BwrapNetworkMode, BwrapResolvConf, BwrapRootMode};
use super::workspace::ensure_no_symlink_escape;
use super::{BwrapSandboxManager, WORKSPACE_PREFIX};
use crate::sandbox::ExecResult;

impl BwrapSandboxManager {
    pub(crate) async fn exec_command(
        &mut self,
        cmd: &str,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let _lock = self.lock_scope()?;
        self.ensure_scope_dirs_locked()?;
        self.write_or_refresh_metadata_locked(false)?;
        self.config.validate()?;

        let work_dir = self.prepare_overlay_workdir()?;
        let resolv_conf = self.prepare_resolv_conf_bind()?;
        let args = self.bwrap_args(work_dir.as_deref(), resolv_conf.as_deref(), cmd);
        debug!(
            scope = %self.state.scope_name,
            root_mode = %self.config.root_mode,
            network_mode = %self.config.net,
            "Executing bwrap sandbox command"
        );

        let result = self.run_bwrap(args, cancellation_token).await;
        if let Some(work_dir) = work_dir {
            let _ = fs::remove_dir_all(work_dir);
        }
        result
    }

    pub(super) fn prepare_overlay_workdir(&self) -> Result<Option<PathBuf>> {
        if self.config.root_mode != BwrapRootMode::OverlayRw {
            return Ok(None);
        }
        let work_dir = self
            .state
            .system_work
            .join(format!("exec-{}", Uuid::new_v4()));
        fs::create_dir_all(&work_dir)
            .with_context(|| format!("Failed to create overlay workdir {}", work_dir.display()))?;
        Ok(Some(work_dir))
    }

    pub(super) fn bwrap_args(
        &self,
        work_dir: Option<&Path>,
        resolv_conf: Option<&Path>,
        command: &str,
    ) -> Vec<OsString> {
        let mut args = vec![
            "--unshare-user".into(),
            "--uid".into(),
            "0".into(),
            "--gid".into(),
            "0".into(),
            "--unshare-pid".into(),
            "--unshare-ipc".into(),
            "--unshare-uts".into(),
            "--unshare-cgroup-try".into(),
            "--die-with-parent".into(),
            "--new-session".into(),
            "--clearenv".into(),
        ];

        if self.config.net == BwrapNetworkMode::None {
            args.push("--unshare-net".into());
        }

        for (key, value) in &self.config.default_env {
            args.push("--setenv".into());
            args.push(key.into());
            args.push(value.into());
        }

        match self.config.root_mode {
            BwrapRootMode::ReadOnly => {
                args.push("--ro-bind".into());
                args.push(self.config.rootfs.clone().into_os_string());
                args.push("/".into());
            }
            BwrapRootMode::OverlayRw => {
                args.push("--overlay-src".into());
                args.push(self.config.rootfs.clone().into_os_string());
                args.push("--overlay".into());
                args.push(self.state.system_upper.clone().into_os_string());
                args.push(
                    work_dir
                        .expect("overlay workdir must exist")
                        .as_os_str()
                        .into(),
                );
                args.push("/".into());
            }
        }

        args.extend([
            "--proc".into(),
            "/proc".into(),
            "--dev".into(),
            "/dev".into(),
            "--tmpfs".into(),
            "/tmp".into(),
            "--bind".into(),
            self.state.workspace.clone().into_os_string(),
            WORKSPACE_PREFIX.into(),
        ]);

        if let Some(resolv_conf) = resolv_conf {
            args.push("--ro-bind".into());
            args.push(resolv_conf.as_os_str().into());
            args.push("/etc/resolv.conf".into());
        }

        args.extend(["--chdir".into(), self.config.default_workdir.clone().into()]);

        if self.config.disable_nested_userns {
            args.push("--disable-userns".into());
        }

        args.push(self.config.default_shell.clone().into());
        args.push("-lc".into());
        args.push(command.into());
        args
    }

    pub(super) fn prepare_resolv_conf_bind(&self) -> Result<Option<PathBuf>> {
        if self.config.net != BwrapNetworkMode::Host {
            return Ok(None);
        }
        match &self.config.resolv_conf {
            BwrapResolvConf::None => Ok(None),
            BwrapResolvConf::Path(path) => {
                let bytes = fs::read(path).with_context(|| {
                    format!("Failed to read configured resolver {}", path.display())
                })?;
                self.ensure_resolv_conf_bind_target(&bytes)?;
                Ok(Some(path.clone()))
            }
            BwrapResolvConf::Auto => {
                let host = PathBuf::from("/etc/resolv.conf");
                if !host.exists() {
                    return Ok(None);
                }
                let bytes = fs::read(&host)
                    .with_context(|| format!("Failed to read host resolver {}", host.display()))?;
                let path = self.state.scope_dir.join("resolv.conf");
                fs::write(&path, &bytes).with_context(|| {
                    format!("Failed to stage bwrap resolver config {}", path.display())
                })?;
                self.ensure_resolv_conf_bind_target(&bytes)?;
                Ok(Some(path))
            }
        }
    }

    fn ensure_resolv_conf_bind_target(&self, bytes: &[u8]) -> Result<()> {
        let rootfs_target = self.config.rootfs.join("etc/resolv.conf");
        match rootfs_target.symlink_metadata() {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                return Ok(());
            }
            Ok(metadata) if metadata.is_dir() => {
                bail!(
                    "Bwrap resolver bind target is a directory in rootfs: {}",
                    rootfs_target.display()
                );
            }
            Ok(_) | Err(_) if self.config.root_mode != BwrapRootMode::OverlayRw => {
                bail!(
                    "Bwrap resolver bind target {} is missing or not a regular file. Use BWRAP_ROOT_MODE=overlay-rw or add /etc/resolv.conf to the rootfs image.",
                    rootfs_target.display()
                );
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| {
                    format!(
                        "Failed to inspect bwrap resolver bind target {}",
                        rootfs_target.display()
                    )
                });
            }
            _ => {}
        }

        let upper_target = self.state.system_upper.join("etc/resolv.conf");
        let upper_parent = upper_target
            .parent()
            .ok_or_else(|| anyhow!("Invalid bwrap resolver upper path"))?;
        ensure_no_symlink_escape(&self.state.system_upper, upper_parent)?;
        fs::create_dir_all(upper_parent).with_context(|| {
            format!(
                "Failed to create bwrap resolver upper dir {}",
                upper_parent.display()
            )
        })?;
        ensure_no_symlink_escape(&self.state.system_upper, upper_parent)?;
        if upper_target
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!(
                "Refusing bwrap resolver upper symlink: {}",
                upper_target.display()
            );
        }
        fs::write(&upper_target, bytes).with_context(|| {
            format!(
                "Failed to create bwrap resolver bind target {}",
                upper_target.display()
            )
        })?;
        Ok(())
    }

    async fn run_bwrap(
        &self,
        args: Vec<OsString>,
        cancellation_token: Option<&tokio_util::sync::CancellationToken>,
    ) -> Result<ExecResult> {
        let mut command = Command::new(&self.config.bwrap_bin);
        command
            .args(args)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        #[cfg(unix)]
        {
            command.process_group(0);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "Failed to spawn bwrap at {}. Install bubblewrap (`apk add bubblewrap` or `apt install bubblewrap`), set BWRAP_BIN=/path/to/bwrap, or choose another sandbox backend with SANDBOX_BACKEND=docker|broker.",
                self.config.bwrap_bin.display()
            )
        })?;
        let child_pid = child.id();
        let stdout_task = child
            .stdout
            .take()
            .map(|stdout| tokio::spawn(read_capped_counted(stdout, self.config.max_output_bytes)));
        let stderr_task = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(read_capped_counted(stderr, self.config.max_output_bytes)));

        let timeout = tokio::time::sleep(self.config.command_timeout);
        tokio::pin!(timeout);

        let status = if let Some(token) = cancellation_token {
            tokio::select! {
                status = child.wait() => status.context("Failed to wait for bwrap command")?,
                () = &mut timeout => {
                    let cleanup = cleanup_bwrap_child(&mut child, child_pid).await;
                    let _ = await_capped_output(stdout_task).await;
                    let _ = await_capped_output(stderr_task).await;
                    bail!("Bwrap command timed out after {}s and the {cleanup}.", self.config.command_timeout.as_secs());
                }
                () = token.cancelled() => {
                    let cleanup = cleanup_bwrap_child(&mut child, child_pid).await;
                    let _ = await_capped_output(stdout_task).await;
                    let _ = await_capped_output(stderr_task).await;
                    bail!("Bwrap command cancelled by user and the {cleanup}.");
                }
            }
        } else {
            tokio::select! {
                status = child.wait() => status.context("Failed to wait for bwrap command")?,
                () = &mut timeout => {
                    let cleanup = cleanup_bwrap_child(&mut child, child_pid).await;
                    let _ = await_capped_output(stdout_task).await;
                    let _ = await_capped_output(stderr_task).await;
                    bail!("Bwrap command timed out after {}s and the {cleanup}.", self.config.command_timeout.as_secs());
                }
            }
        };

        let stdout_capture = await_capped_output(stdout_task).await;
        let stderr_capture = await_capped_output(stderr_task).await;
        let (stdout, stdout_truncation) = stdout_capture.into_output();
        let (mut stderr, stderr_truncation) = stderr_capture.into_output();
        if let Some(truncation) = &stdout_truncation {
            stderr.push_str(&format!(
                "\n[oxide-agent] stdout truncated by BWRAP_MAX_OUTPUT_BYTES: captured {} of {} bytes",
                truncation.captured_bytes,
                truncation.original_bytes
            ));
        }
        if let Some(truncation) = &stderr_truncation {
            stderr.push_str(&format!(
                "\n[oxide-agent] stderr truncated by BWRAP_MAX_OUTPUT_BYTES: captured {} of {} bytes",
                truncation.captured_bytes,
                truncation.original_bytes
            ));
        }

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code: i64::from(status.code().unwrap_or(-1)),
        })
    }
}
