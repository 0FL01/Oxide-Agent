use super::workspace::resolve_workspace_path;
use super::{
    BwrapNetworkMode, BwrapRootMode, BwrapSandboxManager, WORKSPACE_PREFIX, host_arch,
    load_manifest,
};
use crate::sandbox::{SandboxEditReadGuard, SandboxFileEdit, SandboxScope};
use sha2::{Digest, Sha256};
use std::ffi::OsString;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::time::Duration;

const BWRAP_TEST_ENV_KEYS: &[&str] = &[
    "BWRAP_ALLOW_OVERLAY",
    "BWRAP_BIN",
    "BWRAP_COMMAND_TIMEOUT_SECS",
    "BWRAP_DISABLE_NESTED_USERNS",
    "BWRAP_IMAGE",
    "BWRAP_IMAGE_BOOTSTRAP",
    "BWRAP_IMAGE_PACKAGE_MANAGER",
    "BWRAP_IMAGE_SHA256",
    "BWRAP_IMAGE_STORE",
    "BWRAP_IMAGE_URL",
    "BWRAP_LOCK_DIR",
    "BWRAP_MAX_OUTPUT_BYTES",
    "BWRAP_MAX_READ_FILE_BYTES",
    "BWRAP_NET",
    "BWRAP_RECREATE_LOCK_TIMEOUT_SECS",
    "BWRAP_RESOLV_CONF",
    "BWRAP_ROOT_MODE",
    "BWRAP_ROOT_UPPER_DIR",
    "BWRAP_ROOTFS",
    "BWRAP_STATE_DIR",
    "SANDBOX_EXEC_TIMEOUT_SECS",
    "SANDBOX_IMAGE",
];

struct EnvGuard {
    previous: Vec<(&'static str, Option<OsString>)>,
}

impl EnvGuard {
    fn capture(keys: &'static [&'static str]) -> Self {
        Self {
            previous: keys
                .iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect(),
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn workspace_path_accepts_relative_and_workspace_absolute_paths() {
    let workspace = Path::new("/tmp/scope/workspace");

    assert_eq!(
        resolve_workspace_path(workspace, "foo.txt").unwrap(),
        Path::new("/tmp/scope/workspace/foo.txt")
    );
    assert_eq!(
        resolve_workspace_path(workspace, "dir/foo.txt").unwrap(),
        Path::new("/tmp/scope/workspace/dir/foo.txt")
    );
    assert_eq!(
        resolve_workspace_path(workspace, "/workspace/foo.txt").unwrap(),
        Path::new("/tmp/scope/workspace/foo.txt")
    );
    assert_eq!(
        resolve_workspace_path(workspace, "/workspace/dir/foo.txt").unwrap(),
        Path::new("/tmp/scope/workspace/dir/foo.txt")
    );
}

#[test]
fn workspace_path_rejects_escape_forms() {
    let workspace = Path::new("/tmp/scope/workspace");

    for path in [
        "..",
        "../x",
        "/workspace/../x",
        "/etc/passwd",
        "dir/../../x",
        "bad\0path",
    ] {
        assert!(
            resolve_workspace_path(workspace, path).is_err(),
            "{path} should be rejected"
        );
    }
}

#[test]
fn bwrap_modes_parse_valid_values() {
    assert_eq!(
        "host".parse::<BwrapNetworkMode>().unwrap(),
        BwrapNetworkMode::Host
    );
    assert_eq!(
        "none".parse::<BwrapNetworkMode>().unwrap(),
        BwrapNetworkMode::None
    );
    assert_eq!(
        "overlay-rw".parse::<BwrapRootMode>().unwrap(),
        BwrapRootMode::OverlayRw
    );
    assert_eq!(
        "ro".parse::<BwrapRootMode>().unwrap(),
        BwrapRootMode::ReadOnly
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_state_lifecycle_persists_workspace_and_recreate_wipes_it() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
    std::env::set_var("BWRAP_BIN", &fake_bwrap);
    std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "5");
    std::env::set_var("BWRAP_DISABLE_NESTED_USERNS", "false");
    std::env::set_var("BWRAP_IMAGE", "test-dev");
    std::env::set_var("BWRAP_LOCK_DIR", temp.path().join("locks"));
    std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1024");
    std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1024");
    std::env::set_var("BWRAP_NET", "none");
    std::env::set_var("BWRAP_RESOLV_CONF", "none");
    std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
    std::env::set_var("BWRAP_ROOTFS", &rootfs);
    std::env::set_var("BWRAP_STATE_DIR", temp.path().join("scopes"));
    std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");

    let scope = SandboxScope::new(42, "topic-alpha").with_transport_metadata(Some(1001), Some(77));
    let mut manager = BwrapSandboxManager::new(scope.clone()).await.unwrap();

    manager.create_sandbox().await.unwrap();
    assert!(manager.is_running());
    assert_eq!(
        manager.current_record().unwrap().container_name,
        scope.stable_name()
    );

    manager
        .write_file("notes/todo.txt", b"hello")
        .await
        .unwrap();
    assert_eq!(
        manager
            .read_file("/workspace/notes/todo.txt")
            .await
            .unwrap(),
        b"hello"
    );
    let listing = manager.list_files("/workspace").await.unwrap();
    assert!(listing.listing.contains("/workspace/notes/"));
    assert!(listing.listing.contains("/workspace/notes/todo.txt"));
    assert_eq!(
        manager
            .file_size_bytes("notes/todo.txt", None)
            .await
            .unwrap(),
        5
    );

    manager.recreate().await.unwrap();
    assert!(manager.read_file("notes/todo.txt").await.is_err());
    let record = manager.current_record().unwrap();
    assert_eq!(
        record.container_id,
        format!("bwrap:{}", scope.stable_name())
    );
    assert_eq!(
        record.labels.get("agent.sandbox_backend"),
        Some(&"bwrap".to_string())
    );

    manager.destroy().await.unwrap();
    assert!(
        !temp
            .path()
            .join("scopes")
            .join(scope.stable_name())
            .exists()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_workspace_file_ops_reject_symlink_escapes() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    std::fs::create_dir_all(rootfs.join("etc")).expect("fake rootfs etc");
    std::fs::write(rootfs.join("etc/resolv.conf"), b"").expect("fake rootfs resolv");
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "topic-symlinks"))
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();

    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&outside).expect("outside dir");
    std::fs::write(outside.join("secret.txt"), b"secret").expect("outside secret");

    symlink(&outside, manager.state.workspace.join("linked-dir")).expect("parent symlink");
    assert!(
        manager
            .write_file("linked-dir/new.txt", b"nope")
            .await
            .is_err()
    );
    assert!(manager.list_files("linked-dir").await.is_err());

    symlink(
        outside.join("secret.txt"),
        manager.state.workspace.join("secret-link.txt"),
    )
    .expect("final symlink");
    assert!(manager.read_file("secret-link.txt").await.is_err());
    assert!(
        manager
            .write_file("secret-link.txt", b"nope")
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_apply_file_edit_is_guarded_under_scope_lock() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "topic-apply-edit"))
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();
    manager
        .write_file("notes.txt", b"alpha\nbeta\n")
        .await
        .unwrap();
    let current = manager.read_file("notes.txt").await.unwrap();
    let read_guard = SandboxEditReadGuard {
        sha256: format!("{:x}", Sha256::digest(&current)),
        bytes: current.len(),
    };

    let result = manager
        .apply_file_edit(
            "notes.txt",
            SandboxFileEdit {
                search: "beta".to_string(),
                replace: "gamma".to_string(),
                expected_replacements: 1,
            },
            Some(read_guard.clone()),
        )
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.replacements, 1);
    assert_eq!(result.previous_sha256, read_guard.sha256);
    assert_eq!(
        manager.read_file("notes.txt").await.unwrap(),
        b"alpha\ngamma\n"
    );

    manager
        .write_file("notes.txt", b"changed elsewhere\n")
        .await
        .unwrap();
    let stale_error = manager
        .apply_file_edit(
            "notes.txt",
            SandboxFileEdit {
                search: "changed".to_string(),
                replace: "updated".to_string(),
                expected_replacements: 1,
            },
            Some(read_guard),
        )
        .await
        .err()
        .expect("stale read guard should fail")
        .to_string();
    assert!(stale_error.contains("file changed after last read"));

    let fresh = manager.read_file("notes.txt").await.unwrap();
    let fresh_guard = SandboxEditReadGuard {
        sha256: format!("{:x}", Sha256::digest(&fresh)),
        bytes: fresh.len(),
    };
    let count_error = manager
        .apply_file_edit(
            "notes.txt",
            SandboxFileEdit {
                search: "missing".to_string(),
                replace: "updated".to_string(),
                expected_replacements: 1,
            },
            Some(fresh_guard),
        )
        .await
        .err()
        .expect("replacement count mismatch should fail")
        .to_string();
    assert!(count_error.contains("expected 1 replacements, found 0"));
    assert_eq!(
        manager.read_file("notes.txt").await.unwrap(),
        b"changed elsewhere\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_invocation_args_encode_network_root_modes_and_bind_policy() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    std::fs::create_dir_all(rootfs.join("etc")).expect("fake rootfs etc");
    std::fs::write(rootfs.join("etc/resolv.conf"), b"").expect("fake rootfs resolv");
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_NET", "host");
    std::env::set_var("BWRAP_RESOLV_CONF", "auto");
    std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
    let overlay_manager = BwrapSandboxManager::new(SandboxScope::new(42, "args-overlay-host"))
        .await
        .unwrap();
    let work_dir = overlay_manager
        .prepare_overlay_workdir()
        .unwrap()
        .expect("overlay mode should create a workdir");
    let overlay_args = args_to_strings(overlay_manager.bwrap_args(Some(&work_dir), None, "true"));

    assert!(overlay_args.contains(&"--overlay-src".to_string()));
    assert!(overlay_args.contains(&"--overlay".to_string()));
    assert!(contains_arg_pair(
        &overlay_args,
        "--bind",
        &overlay_manager.state.workspace.display().to_string()
    ));
    assert!(contains_arg_pair(
        &overlay_args,
        "--chdir",
        WORKSPACE_PREFIX
    ));
    assert!(!overlay_args.contains(&"--unshare-net".to_string()));
    assert_args_do_not_bind_host_control_paths(&overlay_args);

    if Path::new("/etc/resolv.conf").exists() {
        overlay_manager.ensure_scope_dirs_locked().unwrap();
        let staged_resolv = overlay_manager
            .prepare_resolv_conf_bind()
            .unwrap()
            .expect("auto resolver should stage a bind source");
        assert!(staged_resolv.starts_with(&overlay_manager.state.scope_dir));
        assert_ne!(staged_resolv, PathBuf::from("/etc/resolv.conf"));
        assert!(
            !staged_resolv
                .symlink_metadata()
                .unwrap()
                .file_type()
                .is_symlink()
        );
        let overlay_args_with_resolv = args_to_strings(overlay_manager.bwrap_args(
            Some(&work_dir),
            Some(&staged_resolv),
            "true",
        ));
        assert!(contains_arg_pair(
            &overlay_args_with_resolv,
            "--ro-bind",
            &staged_resolv.display().to_string()
        ));
    }

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_NET", "none");
    std::env::set_var("BWRAP_ROOT_MODE", "ro");
    let readonly_manager = BwrapSandboxManager::new(SandboxScope::new(42, "args-ro-none"))
        .await
        .unwrap();
    let readonly_args = args_to_strings(readonly_manager.bwrap_args(None, None, "printf ok"));

    assert!(readonly_args.contains(&"--unshare-net".to_string()));
    assert!(contains_arg_pair(
        &readonly_args,
        "--ro-bind",
        &readonly_manager.config.rootfs.display().to_string()
    ));
    assert!(!readonly_args.contains(&"--overlay".to_string()));
    assert!(contains_arg_pair(
        &readonly_args,
        "--bind",
        &readonly_manager.state.workspace.display().to_string()
    ));
    assert_args_do_not_bind_host_control_paths(&readonly_args);
    assert!(readonly_args.ends_with(&[
        "/bin/sh".to_string(),
        "-lc".to_string(),
        "printf ok".to_string()
    ]));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_auto_resolver_creates_overlay_bind_target_when_rootfs_file_is_missing() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_NET", "host");
    std::env::set_var("BWRAP_RESOLV_CONF", "auto");
    std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
    let overlay_manager = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-overlay"))
        .await
        .unwrap();
    overlay_manager.ensure_scope_dirs_locked().unwrap();

    if Path::new("/etc/resolv.conf").exists() {
        let staged_resolv = overlay_manager
            .prepare_resolv_conf_bind()
            .unwrap()
            .expect("auto resolver should stage a bind source");
        let upper_target = overlay_manager.state.system_upper.join("etc/resolv.conf");
        assert!(upper_target.is_file());
        assert_eq!(
            std::fs::read(&upper_target).unwrap(),
            std::fs::read(staged_resolv).unwrap()
        );
    }

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_NET", "host");
    std::env::set_var("BWRAP_RESOLV_CONF", "auto");
    std::env::set_var("BWRAP_ROOT_MODE", "ro");
    let readonly_manager = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-ro"))
        .await
        .unwrap();
    readonly_manager.ensure_scope_dirs_locked().unwrap();

    if Path::new("/etc/resolv.conf").exists() {
        let readonly_error = readonly_manager
            .prepare_resolv_conf_bind()
            .err()
            .expect("missing readonly resolver target should fail")
            .to_string();
        assert!(readonly_error.contains("BWRAP_ROOT_MODE=overlay-rw"));
        assert!(readonly_error.contains("/etc/resolv.conf"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_config_errors_are_actionable() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_BIN", temp.path().join("missing-bwrap"));
    let missing_bwrap = BwrapSandboxManager::new(SandboxScope::new(42, "missing-bwrap"))
        .await
        .err()
        .expect("missing bwrap should fail")
        .to_string();
    assert!(missing_bwrap.contains("BWRAP_BIN"));
    assert!(missing_bwrap.contains("Install bubblewrap"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_ROOTFS", temp.path().join("missing-rootfs"));
    let missing_rootfs = BwrapSandboxManager::new(SandboxScope::new(42, "missing-rootfs"))
        .await
        .err()
        .expect("missing rootfs should fail")
        .to_string();
    assert!(missing_rootfs.contains("rootfs not found"));
    assert!(missing_rootfs.contains("scripts/build-bwrap-rootfs-debian.sh"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let rootfs_symlink = temp.path().join("rootfs-symlink");
    symlink(&rootfs, &rootfs_symlink).expect("rootfs symlink");
    std::env::set_var("BWRAP_ROOTFS", &rootfs_symlink);
    let rootfs_symlink_error = BwrapSandboxManager::new(SandboxScope::new(42, "rootfs-symlink"))
        .await
        .err()
        .expect("rootfs symlink should fail")
        .to_string();
    assert!(rootfs_symlink_error.contains("BWRAP_ROOTFS"));
    assert!(rootfs_symlink_error.contains("must not be a symlink"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_RESOLV_CONF", temp.path());
    let resolv_dir_error = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-dir"))
        .await
        .err()
        .expect("resolv dir should fail")
        .to_string();
    assert!(resolv_dir_error.contains("BWRAP_RESOLV_CONF"));
    assert!(resolv_dir_error.contains("regular file"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let resolv_file = temp.path().join("resolv.conf");
    std::fs::write(&resolv_file, b"nameserver 127.0.0.1\n").expect("resolv file");
    let resolv_symlink = temp.path().join("resolv-link.conf");
    symlink(&resolv_file, &resolv_symlink).expect("resolv symlink");
    std::env::set_var("BWRAP_RESOLV_CONF", &resolv_symlink);
    let resolv_symlink_error = BwrapSandboxManager::new(SandboxScope::new(42, "resolv-symlink"))
        .await
        .err()
        .expect("resolv symlink should fail")
        .to_string();
    assert!(resolv_symlink_error.contains("BWRAP_RESOLV_CONF"));
    assert!(resolv_symlink_error.contains("must not be a symlink"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::remove_var("BWRAP_ROOTFS");
    std::env::set_var("BWRAP_IMAGE_STORE", temp.path().join("empty-images"));
    std::env::set_var("SANDBOX_IMAGE", "agent-sandbox:custom");
    let docker_image_only = BwrapSandboxManager::new(SandboxScope::new(42, "docker-image-only"))
        .await
        .err()
        .expect("missing bwrap image should fail")
        .to_string();
    assert!(docker_image_only.contains("Bwrap image manifest not found"));
    assert!(docker_image_only.contains("BWRAP_IMAGE/BWRAP_ROOTFS"));
    assert!(docker_image_only.contains("SANDBOX_IMAGE is Docker-only"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let image_store = temp.path().join("images");
    let unsafe_image = image_store.join("unsafe-rootfs-link");
    std::fs::create_dir_all(&unsafe_image).expect("unsafe image dir");
    std::fs::write(
        unsafe_image.join("image.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "id": "unsafe-rootfs-link",
  "arch": "{}",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
            host_arch()
        ),
    )
    .expect("unsafe image manifest");
    symlink(&rootfs, unsafe_image.join("rootfs")).expect("unsafe rootfs symlink");
    std::env::remove_var("BWRAP_ROOTFS");
    std::env::set_var("BWRAP_IMAGE_STORE", &image_store);
    std::env::set_var("BWRAP_IMAGE", "unsafe-rootfs-link");
    let unsafe_rootfs_symlink =
        BwrapSandboxManager::new(SandboxScope::new(42, "unsafe-rootfs-symlink"))
            .await
            .err()
            .expect("unsafe rootfs symlink should fail")
            .to_string();
    assert!(unsafe_rootfs_symlink.contains("resolves outside image directory"));
    assert!(unsafe_rootfs_symlink.contains("unsafe rootfs symlink"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_ROOT_MODE", "tmp-overlay");
    let unsupported_root_mode =
        BwrapSandboxManager::new(SandboxScope::new(42, "unsupported-root-mode"))
            .await
            .err()
            .expect("unsupported root mode should fail")
            .to_string();
    assert!(unsupported_root_mode.contains("tmp-overlay is not supported"));
    assert!(unsupported_root_mode.contains("overlay-rw, ro"));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_lock_timeout_defaults_to_command_timeout_plus_five_and_rejects_zero() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "7");
    std::env::remove_var("BWRAP_RECREATE_LOCK_TIMEOUT_SECS");
    let manager = BwrapSandboxManager::new(SandboxScope::new(42, "default-lock-timeout"))
        .await
        .unwrap();
    assert_eq!(manager.config.lock_timeout, Duration::from_secs(12));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_RECREATE_LOCK_TIMEOUT_SECS", "0");
    let zero_lock_timeout = BwrapSandboxManager::new(SandboxScope::new(42, "zero-lock-timeout"))
        .await
        .err()
        .expect("zero lock timeout should fail")
        .to_string();
    assert!(zero_lock_timeout.contains("BWRAP_RECREATE_LOCK_TIMEOUT_SECS"));
    assert!(zero_lock_timeout.contains("greater than zero"));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_metadata_reports_manifest_path_package_manager_and_sha() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    let image_dir = temp.path().join("images/debian-test");
    let rootfs = image_dir.join("rootfs");
    create_fake_rootfs(&rootfs);
    let manifest_path = image_dir.join("image.json");
    std::fs::write(
        &manifest_path,
        format!(
            r#"{{
  "schema_version": 1,
  "id": "debian-test",
  "arch": "{}",
  "package_manager": "apt",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
            host_arch()
        ),
    )
    .expect("image manifest");

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::remove_var("BWRAP_ROOTFS");
    std::env::set_var("BWRAP_IMAGE_STORE", temp.path().join("images"));
    std::env::set_var("BWRAP_IMAGE", "debian-test");

    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "metadata-status"))
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();
    let record = manager.current_record().unwrap();

    assert_eq!(
        record.labels.get("agent.image_manifest_path"),
        Some(&manifest_path.display().to_string())
    );
    assert!(
        record
            .labels
            .get("agent.image_manifest_sha256")
            .is_some_and(|value| !value.is_empty())
    );
    assert_eq!(
        record.labels.get("agent.package_manager"),
        Some(&"apt".to_string())
    );
    let status = record.status.expect("status");
    assert!(status.contains("package_manager=apt"));
    assert!(status.contains(&format!("manifest={}", manifest_path.display())));
    assert!(status.contains(&format!("rootfs={}", rootfs.display())));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_image_bootstrap_download_creates_image() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);
    let source_rootfs = temp.path().join("source-rootfs");
    create_fake_rootfs(&source_rootfs);
    let (tarball, sha256) = create_rootfs_tarball(temp.path(), &source_rootfs);

    configure_bwrap_image_bootstrap_env(
        temp.path(),
        &fake_bwrap,
        &tarball,
        &sha256,
        "bootstrap-test",
    );

    let manager = BwrapSandboxManager::new(SandboxScope::new(42, "bootstrap-download"))
        .await
        .unwrap();
    assert_eq!(manager.config.image_id, "bootstrap-test");
    let image_dir = temp.path().join("images/bootstrap-test");
    assert!(image_dir.join("image.json").is_file());
    assert!(image_dir.join("rootfs/bin/sh").is_file());
    let (manifest, _) = load_manifest(&image_dir.join("image.json")).unwrap();
    assert_eq!(manifest.package_manager.as_deref(), Some("apk"));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_image_bootstrap_rejects_bad_sha256() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);
    let source_rootfs = temp.path().join("source-rootfs");
    create_fake_rootfs(&source_rootfs);
    let (tarball, _) = create_rootfs_tarball(temp.path(), &source_rootfs);

    configure_bwrap_image_bootstrap_env(
        temp.path(),
        &fake_bwrap,
        &tarball,
        &"0".repeat(64),
        "bootstrap-bad-sha",
    );

    let error = BwrapSandboxManager::new(SandboxScope::new(42, "bootstrap-bad-sha"))
        .await
        .err()
        .expect("bad checksum should fail")
        .to_string();
    assert!(error.contains("Checksum mismatch"));
    assert!(!temp.path().join("images/bootstrap-bad-sha").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_image_bootstrap_noops_when_manifest_exists() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);
    let image_dir = temp.path().join("images/existing-image");
    create_fake_rootfs(&image_dir.join("rootfs"));
    write_test_image_manifest(&image_dir, "existing-image", "apk");

    configure_fake_bwrap_env(temp.path(), &image_dir.join("rootfs"), &fake_bwrap);
    std::env::remove_var("BWRAP_ROOTFS");
    std::env::set_var("BWRAP_IMAGE", "existing-image");
    std::env::set_var("BWRAP_IMAGE_BOOTSTRAP", "download");
    std::env::set_var("BWRAP_IMAGE_STORE", temp.path().join("images"));
    std::env::remove_var("BWRAP_IMAGE_URL");
    std::env::remove_var("BWRAP_IMAGE_SHA256");

    let manager = BwrapSandboxManager::new(SandboxScope::new(42, "bootstrap-existing"))
        .await
        .unwrap();
    assert_eq!(manager.config.image_id, "existing-image");
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_state_and_lock_dir_errors_name_config_keys() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let state_file = temp.path().join("not-a-state-dir");
    std::fs::write(&state_file, b"file").expect("state file");
    std::env::set_var("BWRAP_STATE_DIR", &state_file);
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-state-dir"))
        .await
        .unwrap();
    let state_error = manager
        .create_sandbox()
        .await
        .err()
        .expect("state file should fail")
        .to_string();
    assert!(state_error.contains("BWRAP_STATE_DIR"));
    assert!(state_error.contains("must be a directory"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let state_target = temp.path().join("state-target");
    std::fs::create_dir(&state_target).expect("state target");
    let state_symlink = temp.path().join("state-link");
    symlink(&state_target, &state_symlink).expect("state symlink");
    std::env::set_var("BWRAP_STATE_DIR", &state_symlink);
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-state-symlink"))
        .await
        .unwrap();
    let state_symlink_error = manager
        .create_sandbox()
        .await
        .err()
        .expect("state symlink should fail")
        .to_string();
    assert!(state_symlink_error.contains("BWRAP_STATE_DIR"));
    assert!(state_symlink_error.contains("must not be a symlink"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let lock_file = temp.path().join("not-a-lock-dir");
    std::fs::write(&lock_file, b"file").expect("lock file");
    std::env::set_var("BWRAP_LOCK_DIR", &lock_file);
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-lock-dir"))
        .await
        .unwrap();
    let lock_error = manager
        .create_sandbox()
        .await
        .err()
        .expect("lock file should fail")
        .to_string();
    assert!(lock_error.contains("BWRAP_LOCK_DIR"));
    assert!(lock_error.contains("must be a directory"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let lock_target = temp.path().join("lock-target");
    std::fs::create_dir(&lock_target).expect("lock target");
    let lock_symlink = temp.path().join("lock-link");
    symlink(&lock_target, &lock_symlink).expect("lock symlink");
    std::env::set_var("BWRAP_LOCK_DIR", &lock_symlink);
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "bad-lock-symlink"))
        .await
        .unwrap();
    let lock_symlink_error = manager
        .create_sandbox()
        .await
        .err()
        .expect("lock symlink should fail")
        .to_string();
    assert!(lock_symlink_error.contains("BWRAP_LOCK_DIR"));
    assert!(lock_symlink_error.contains("must not be a symlink"));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_root_upper_dir_override_is_per_scope_and_rejects_unsafe_paths() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap(&fake_bwrap);

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let root_upper_parent = temp.path().join("root-upper");
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &root_upper_parent);
    let scope = SandboxScope::new(42, "upper-override");
    let manager = BwrapSandboxManager::new(scope.clone()).await.unwrap();
    assert_eq!(
        manager.state.system_upper,
        root_upper_parent.join(scope.stable_name()).join("upper")
    );
    assert_eq!(
        manager.state.system_work,
        root_upper_parent.join(scope.stable_name()).join("work")
    );
    let work_dir = manager
        .prepare_overlay_workdir()
        .unwrap()
        .expect("overlay workdir");
    assert!(work_dir.starts_with(root_upper_parent.join(scope.stable_name()).join("work")));
    let mut manager = manager;
    manager.create_sandbox().await.unwrap();
    assert!(root_upper_parent.join(scope.stable_name()).exists());
    let changed_root_upper_parent = temp.path().join("changed-root-upper");
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &changed_root_upper_parent);
    let pinned_manager = BwrapSandboxManager::new(scope.clone()).await.unwrap();
    assert_eq!(
        pinned_manager.state.system_dir,
        root_upper_parent.join(scope.stable_name())
    );
    assert!(!changed_root_upper_parent.join(scope.stable_name()).exists());
    manager.destroy().await.unwrap();
    assert!(!root_upper_parent.join(scope.stable_name()).exists());
    assert!(root_upper_parent.exists());

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &root_upper_parent);
    let delete_scope = SandboxScope::new(42, "upper-delete");
    let mut manager = BwrapSandboxManager::new(delete_scope.clone())
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();
    assert!(root_upper_parent.join(delete_scope.stable_name()).exists());
    assert!(
        BwrapSandboxManager::delete_sandbox_by_name(42, &delete_scope.stable_name())
            .await
            .unwrap()
    );
    assert!(!root_upper_parent.join(delete_scope.stable_name()).exists());

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &root_upper_parent);
    let changed_delete_scope = SandboxScope::new(42, "upper-delete-after-env-change");
    let mut manager = BwrapSandboxManager::new(changed_delete_scope.clone())
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &changed_root_upper_parent);
    assert!(
        BwrapSandboxManager::delete_sandbox_by_name(42, &changed_delete_scope.stable_name())
            .await
            .unwrap()
    );
    assert!(
        !root_upper_parent
            .join(changed_delete_scope.stable_name())
            .exists()
    );
    assert!(
        !changed_root_upper_parent
            .join(changed_delete_scope.stable_name())
            .exists()
    );

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let file_upper = temp.path().join("file-upper");
    std::fs::write(&file_upper, b"file").expect("file upper");
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &file_upper);
    let file_error = BwrapSandboxManager::new(SandboxScope::new(42, "file-upper"))
        .await
        .err()
        .expect("file upper should fail")
        .to_string();
    assert!(file_error.contains("BWRAP_ROOT_UPPER_DIR"));
    assert!(file_error.contains("must be a directory"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    let symlink_target = temp.path().join("upper-target");
    std::fs::create_dir_all(&symlink_target).expect("upper target");
    let symlink_upper = temp.path().join("upper-symlink");
    symlink(&symlink_target, &symlink_upper).expect("upper symlink");
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", &symlink_upper);
    let symlink_error = BwrapSandboxManager::new(SandboxScope::new(42, "symlink-upper"))
        .await
        .err()
        .expect("symlink upper should fail")
        .to_string();
    assert!(symlink_error.contains("BWRAP_ROOT_UPPER_DIR"));
    assert!(symlink_error.contains("must not be a symlink"));

    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_ROOT_UPPER_DIR", rootfs.join("unsafe-upper"));
    let rootfs_error = BwrapSandboxManager::new(SandboxScope::new(42, "rootfs-upper"))
        .await
        .err()
        .expect("rootfs upper should fail")
        .to_string();
    assert!(rootfs_error.contains("BWRAP_ROOT_UPPER_DIR"));
    assert!(rootfs_error.contains("must not be inside the bwrap rootfs image"));
}

#[cfg(unix)]
#[test]
fn bwrap_manifest_validation_rejects_unsafe_values() {
    let temp = tempfile::tempdir().expect("temp dir");
    let manifest_path = temp.path().join("image.json");

    std::fs::write(
        &manifest_path,
        format!(
            r#"{{
  "schema_version": 1,
  "id": "bad-rootfs",
  "arch": "{}",
  "rootfs": "/abs/rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
            host_arch()
        ),
    )
    .expect("manifest");
    let absolute_rootfs = load_manifest(&manifest_path).unwrap_err().to_string();
    assert!(absolute_rootfs.contains("rootfs must be relative"));

    std::fs::write(
        &manifest_path,
        format!(
            r#"{{
  "schema_version": 1,
  "id": "escaping-rootfs",
  "arch": "{}",
  "rootfs": "../rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
            host_arch()
        ),
    )
    .expect("manifest");
    let escaping_rootfs = load_manifest(&manifest_path).unwrap_err().to_string();
    assert!(escaping_rootfs.contains("rootfs must stay under the image directory"));

    std::fs::write(
        &manifest_path,
        format!(
            r#"{{
  "schema_version": 1,
  "id": "bad-workdir",
  "arch": "{}",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/tmp"
}}"#,
            host_arch()
        ),
    )
    .expect("manifest");
    let bad_workdir = load_manifest(&manifest_path).unwrap_err().to_string();
    assert!(bad_workdir.contains("default_workdir must be /workspace"));
}

#[cfg(unix)]
#[tokio::test]
async fn bwrap_exec_preserves_nonzero_exit_truncates_output_and_times_out() {
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    let rootfs = temp.path().join("rootfs");
    create_fake_rootfs(&rootfs);
    let fake_bwrap = temp.path().join("bwrap");
    create_fake_bwrap_script(
        &fake_bwrap,
        "#!/bin/sh\nprintf abcdefghijklmnop\nprintf qrstuvwxyz >&2\nexit 7\n",
    );
    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "8");

    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "exec-output"))
        .await
        .unwrap();
    let output = manager.exec_command("ignored", None).await.unwrap();
    assert_eq!(output.exit_code, 7);
    assert_eq!(output.stdout, "abcdefgh");
    assert!(output.stderr.contains("qrstuvwx"));
    assert!(
        output
            .stderr
            .contains("stdout truncated by BWRAP_MAX_OUTPUT_BYTES: captured 8 of 16 bytes")
    );
    assert!(
        output
            .stderr
            .contains("stderr truncated by BWRAP_MAX_OUTPUT_BYTES: captured 8 of 10 bytes")
    );

    let child_pid_file = temp.path().join("bwrap-child.pid");
    create_fake_bwrap_script(
        &fake_bwrap,
        &format!(
            "#!/bin/sh\nsleep 30 &\nprintf '%s' \"$!\" > {}\nwait\n",
            child_pid_file.display()
        ),
    );
    configure_fake_bwrap_env(temp.path(), &rootfs, &fake_bwrap);
    std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "1");
    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "exec-timeout"))
        .await
        .unwrap();
    let error = manager
        .exec_command("ignored", None)
        .await
        .err()
        .expect("sleeping bwrap should time out")
        .to_string();
    assert!(error.contains("timed out after 1s"));
    assert!(error.contains("process group"));
    let child_pid = std::fs::read_to_string(child_pid_file)
        .expect("child pid")
        .trim()
        .to_string();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let child_alive = std::process::Command::new("kill")
        .arg("-0")
        .arg(&child_pid)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    assert!(!child_alive, "timeout left child process {child_pid} alive");
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires host bubblewrap support and BWRAP_ROOTFS pointing at a prepared rootfs"]
async fn bwrap_smoke_exec_persists_workspace_and_overlay_rw() {
    let Some(rootfs) = std::env::var_os("BWRAP_ROOTFS").map(PathBuf::from) else {
        eprintln!("skipping ignored bwrap smoke: BWRAP_ROOTFS is not set");
        return;
    };
    if !rootfs.is_dir() {
        eprintln!(
            "skipping ignored bwrap smoke: BWRAP_ROOTFS is not a directory: {}",
            rootfs.display()
        );
        return;
    }
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    configure_real_bwrap_env(temp.path(), &rootfs, BwrapRootMode::OverlayRw);

    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "smoke-overlay-rw"))
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();
    let first = manager
        .exec_command(
            "pwd && printf persisted >/workspace/hello.txt && printf system >/etc/oxide-test && test ! -S /var/run/docker.sock && test ! -e /run/sandboxd",
            None,
        )
        .await
        .unwrap();
    assert_eq!(first.exit_code, 0, "stderr={}", first.stderr);
    assert_eq!(first.stdout.lines().next(), Some("/workspace"));

    let second = manager
        .exec_command(
            "cat /workspace/hello.txt && printf '\\n' && cat /etc/oxide-test",
            None,
        )
        .await
        .unwrap();
    assert_eq!(second.exit_code, 0, "stderr={}", second.stderr);
    assert_eq!(second.stdout, "persisted\nsystem");
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires host bubblewrap support and BWRAP_ROOTFS pointing at a prepared rootfs"]
async fn bwrap_smoke_ro_root_rejects_system_writes() {
    let Some(rootfs) = std::env::var_os("BWRAP_ROOTFS").map(PathBuf::from) else {
        eprintln!("skipping ignored bwrap smoke: BWRAP_ROOTFS is not set");
        return;
    };
    if !rootfs.is_dir() {
        eprintln!(
            "skipping ignored bwrap smoke: BWRAP_ROOTFS is not a directory: {}",
            rootfs.display()
        );
        return;
    }
    let _env_lock = crate::config::test_env_mutex()
        .lock()
        .expect("test env mutex poisoned");
    let _env_guard = EnvGuard::capture(BWRAP_TEST_ENV_KEYS);
    let temp = tempfile::tempdir().expect("temp dir");
    configure_real_bwrap_env(temp.path(), &rootfs, BwrapRootMode::ReadOnly);

    let mut manager = BwrapSandboxManager::new(SandboxScope::new(42, "smoke-ro"))
        .await
        .unwrap();
    manager.create_sandbox().await.unwrap();
    let output = manager
        .exec_command(
            "printf workspace >/workspace/ok.txt && printf system >/etc/oxide-ro-test",
            None,
        )
        .await
        .unwrap();

    assert_ne!(output.exit_code, 0, "system write unexpectedly succeeded");
    assert_eq!(manager.read_file("ok.txt").await.unwrap(), b"workspace");
}

#[cfg(unix)]
fn create_fake_rootfs(rootfs: &Path) {
    for directory in ["bin", "dev", "proc", "tmp", "workspace"] {
        std::fs::create_dir_all(rootfs.join(directory)).expect("fake rootfs dir");
    }
    std::fs::write(rootfs.join("bin/sh"), b"").expect("fake shell");
}

#[cfg(unix)]
fn create_fake_bwrap(path: &PathBuf) {
    create_fake_bwrap_script(path, "#!/bin/sh\nprintf '%s\n' '--disable-userns'\n");
}

#[cfg(unix)]
fn create_fake_bwrap_script(path: &PathBuf, script: &str) {
    let script_body = script.strip_prefix("#!/bin/sh\n").unwrap_or(script);
    let wrapped = format!(
        "#!/bin/sh\nif [ \"${{1:-}}\" = \"--help\" ]; then\n  printf '%s\n' '--disable-userns'\n  exit 0\nfi\n{script_body}"
    );
    std::fs::write(path, wrapped).expect("fake bwrap");
    let mut permissions = std::fs::metadata(path)
        .expect("fake bwrap metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions).expect("fake bwrap permissions");
}

#[cfg(unix)]
fn create_rootfs_tarball(temp: &Path, rootfs: &Path) -> (PathBuf, String) {
    let tarball = temp.join(format!("rootfs-{}.tar.gz", fastrand::u64(..)));
    let output = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&tarball)
        .arg("-C")
        .arg(rootfs)
        .arg(".")
        .output()
        .expect("tar command should run");
    assert!(
        output.status.success(),
        "tar failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let bytes = std::fs::read(&tarball).expect("tarball bytes");
    (tarball, format!("{:x}", Sha256::digest(&bytes)))
}

#[cfg(unix)]
fn write_test_image_manifest(image_dir: &Path, image_id: &str, package_manager: &str) {
    std::fs::create_dir_all(image_dir).expect("image dir");
    std::fs::write(
        image_dir.join("image.json"),
        format!(
            r#"{{
  "schema_version": 1,
  "id": "{image_id}",
  "arch": "{}",
  "package_manager": "{package_manager}",
  "rootfs": "rootfs",
  "default_shell": "/bin/sh",
  "default_workdir": "/workspace"
}}"#,
            host_arch()
        ),
    )
    .expect("image manifest");
}

#[cfg(unix)]
fn configure_bwrap_image_bootstrap_env(
    temp: &Path,
    fake_bwrap: &Path,
    tarball: &Path,
    sha256: &str,
    image_id: &str,
) {
    let file_url = url::Url::from_file_path(tarball)
        .expect("tarball file URL")
        .to_string();
    std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
    std::env::set_var("BWRAP_BIN", fake_bwrap);
    std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "5");
    std::env::set_var("BWRAP_DISABLE_NESTED_USERNS", "false");
    std::env::set_var("BWRAP_IMAGE", image_id);
    std::env::set_var("BWRAP_IMAGE_BOOTSTRAP", "download");
    std::env::set_var("BWRAP_IMAGE_PACKAGE_MANAGER", "apk");
    std::env::set_var("BWRAP_IMAGE_SHA256", sha256);
    std::env::set_var("BWRAP_IMAGE_STORE", temp.join("images"));
    std::env::set_var("BWRAP_IMAGE_URL", file_url);
    std::env::set_var("BWRAP_LOCK_DIR", temp.join("locks"));
    std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1024");
    std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1024");
    std::env::set_var("BWRAP_NET", "none");
    std::env::set_var("BWRAP_RESOLV_CONF", "none");
    std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
    std::env::remove_var("BWRAP_ROOTFS");
    std::env::set_var("BWRAP_STATE_DIR", temp.join("scopes"));
    std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");
}

#[cfg(unix)]
fn configure_fake_bwrap_env(temp: &Path, rootfs: &Path, fake_bwrap: &Path) {
    std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
    std::env::set_var("BWRAP_BIN", fake_bwrap);
    std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "5");
    std::env::set_var("BWRAP_DISABLE_NESTED_USERNS", "false");
    std::env::set_var("BWRAP_IMAGE", "test-dev");
    std::env::set_var("BWRAP_LOCK_DIR", temp.join("locks"));
    std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1024");
    std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1024");
    std::env::set_var("BWRAP_NET", "none");
    std::env::set_var("BWRAP_RESOLV_CONF", "none");
    std::env::set_var("BWRAP_ROOT_MODE", "overlay-rw");
    std::env::set_var("BWRAP_ROOTFS", rootfs);
    std::env::set_var("BWRAP_STATE_DIR", temp.join("scopes"));
    std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");
}

#[cfg(unix)]
fn configure_real_bwrap_env(temp: &Path, rootfs: &Path, root_mode: BwrapRootMode) {
    std::env::set_var("BWRAP_ALLOW_OVERLAY", "true");
    std::env::set_var("BWRAP_COMMAND_TIMEOUT_SECS", "15");
    std::env::set_var("BWRAP_IMAGE", "ignored-test-rootfs");
    std::env::set_var("BWRAP_LOCK_DIR", temp.join("locks"));
    std::env::set_var("BWRAP_MAX_OUTPUT_BYTES", "1048576");
    std::env::set_var("BWRAP_MAX_READ_FILE_BYTES", "1048576");
    std::env::set_var("BWRAP_NET", "host");
    std::env::set_var("BWRAP_RESOLV_CONF", "auto");
    std::env::set_var("BWRAP_ROOT_MODE", root_mode.to_string());
    std::env::set_var("BWRAP_ROOTFS", rootfs);
    std::env::set_var("BWRAP_STATE_DIR", temp.join("scopes"));
    std::env::remove_var("SANDBOX_EXEC_TIMEOUT_SECS");
}

#[cfg(unix)]
fn args_to_strings(args: Vec<OsString>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[cfg(unix)]
fn contains_arg_pair(args: &[String], first: &str, second: &str) -> bool {
    args.windows(2)
        .any(|window| window[0] == first && window[1] == second)
}

#[cfg(unix)]
fn assert_args_do_not_bind_host_control_paths(args: &[String]) {
    let joined = args.join("\n");
    assert!(!joined.contains("/var/run/docker.sock"));
    assert!(!joined.contains("/run/sandboxd"));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let home = home.display().to_string();
        if !home.is_empty() && args.iter().all(|arg| !arg.starts_with(&home)) {
            assert!(!joined.contains(&home));
        }
    }
}
