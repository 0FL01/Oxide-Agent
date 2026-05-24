use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

#[test]
fn legacy_tool_bridge_module_is_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        !manifest_dir.join("src/agent/tool_bridge.rs").exists(),
        "legacy tool_bridge.rs must stay removed; use agent::tool_runtime instead"
    );

    let agent_mod =
        fs::read_to_string(manifest_dir.join("src/agent/mod.rs")).expect("read agent/mod.rs");
    assert!(
        !agent_mod.contains("tool_bridge"),
        "agent module must not export the legacy tool bridge"
    );
}

#[test]
fn legacy_tool_provider_trait_is_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        !manifest_dir.join("src/agent/provider.rs").exists(),
        "legacy agent/provider.rs ToolProvider trait must stay removed; use typed tool_runtime executors"
    );

    let agent_mod =
        fs::read_to_string(manifest_dir.join("src/agent/mod.rs")).expect("read agent/mod.rs");
    assert!(
        !agent_mod.contains("pub mod provider;"),
        "agent module must not export the legacy ToolProvider module"
    );

    let mut files = Vec::new();
    collect_rust_files(&manifest_dir.join("src"), &mut files);
    let offenders = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read source file");
            if source.contains("ToolProvider")
                || source.contains("agent::provider::")
                || source.contains("crate::agent::provider::")
                || source.contains("crate::agent::provider;")
            {
                Some(
                    path.strip_prefix(manifest_dir)
                        .expect("source path under manifest dir")
                        .display()
                        .to_string(),
                )
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "legacy ToolProvider trait references must stay removed; offenders: {offenders:?}"
    );
}

#[test]
fn ssh_cleanup_is_owned_by_ssh_module_not_binaries() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");

    for binary_main in [
        "crates/oxide-agent-telegram-bot/src/main.rs",
        "crates/oxide-agent-sandboxd/src/main.rs",
    ] {
        let source =
            fs::read_to_string(workspace_root.join(binary_main)).expect("read binary main");
        assert!(
            !source.contains("cleanup_stale_private_key_tempfiles"),
            "{binary_main} must not run SSH private-key cleanup unconditionally"
        );
    }

    let modules = fs::read_to_string(manifest_dir.join("src/agent/tool_runtime/modules.rs"))
        .expect("read tool runtime modules");
    assert!(
        modules.contains("cleanup_stale_private_key_tempfiles().map_err"),
        "SSH private-key cleanup must be owned by SshMcpToolModule"
    );

    let provider_exports = fs::read_to_string(manifest_dir.join("src/agent/providers/mod.rs"))
        .expect("read provider exports");
    assert!(
        !provider_exports.contains("cleanup_stale_private_key_tempfiles"),
        "SSH cleanup must not be exported through the generic providers surface"
    );
}

#[test]
fn sandbox_manager_usage_stays_inside_sandbox_facades() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let agent_dir = manifest_dir.join("src/agent");
    let allowed_agent_facades = [manifest_dir.join("src/agent/providers/sandbox.rs")];
    let mut files = Vec::new();
    collect_rust_files(&agent_dir, &mut files);

    let offenders = files
        .into_iter()
        .filter(|path| !allowed_agent_facades.iter().any(|allowed| allowed == path))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read agent source file")
                .contains("SandboxManager")
        })
        .map(|path| {
            path.strip_prefix(manifest_dir)
                .expect("source path under manifest dir")
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "SandboxManager usage must stay behind sandbox module/facade boundaries; offenders: {offenders:?}"
    );
}

#[test]
fn wiki_memory_uses_storage_facade_not_concrete_r2() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let store = fs::read_to_string(manifest_dir.join("src/agent/wiki_memory/store.rs"))
        .expect("read wiki memory store");

    assert!(
        !store.contains("R2Storage"),
        "wiki memory must use StorageProvider-backed WikiObjectBackend, not concrete R2Storage"
    );
    assert!(
        store.contains("StorageProviderWikiBackend"),
        "wiki memory store should keep the storage facade adapter"
    );
}

#[test]
fn telegram_runner_uses_storage_module_factory_not_concrete_r2() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");
    let runner_path = workspace_root.join("crates/oxide-agent-transport-telegram/src/runner.rs");
    let runner = fs::read_to_string(runner_path).expect("read Telegram runner");

    assert!(
        runner.contains("storage::build_primary_storage"),
        "Telegram runner must build storage through the storage backend module factory"
    );
    for forbidden in ["R2Storage", "R2StorageConfig", "R2Storage::new"] {
        assert!(
            !runner.contains(forbidden),
            "Telegram runner must not reference concrete storage backend {forbidden}"
        );
    }
}
