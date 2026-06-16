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
fn runner_has_no_legacy_tool_execution_path_labels() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let execution = fs::read_to_string(manifest_dir.join("src/agent/runner/execution.rs"))
        .expect("read runner execution");

    let forbidden_patterns = [
        "call_llm_with_tools_legacy",
        "legacy_tool_execution_disabled_error",
        "legacy path has no routes",
        "legacy tool execution is disabled",
        "legacy fallback disabled",
        "legacy fallback must not write",
    ];
    let offenders = forbidden_patterns
        .iter()
        .copied()
        .filter(|pattern| execution.contains(pattern))
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "runner must describe the current typed-runtime contract without legacy execution path labels; offenders: {offenders:?}"
    );
}

#[test]
fn tool_call_correlation_api_uses_wire_and_invocation_terms() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let targets = [
        "src/llm/types.rs",
        "src/llm/mod.rs",
        "src/agent/memory.rs",
        "src/llm/providers/tool_result_encoder.rs",
    ];
    let forbidden_patterns = [
        "from_legacy_tool_call_id",
        "legacy_tool_call_id",
        "legacy tool call id",
        "persisted for compatibility",
        "legacy_and_canonical",
        "legacy_tool_message",
        "legacy_assistant_tool_batch",
        "legacy-call",
        "call-legacy",
    ];

    let offenders = targets
        .iter()
        .flat_map(|target| {
            let source = fs::read_to_string(manifest_dir.join(target))
                .unwrap_or_else(|error| panic!("read {target}: {error}"));
            forbidden_patterns
                .iter()
                .copied()
                .filter(|pattern| source.contains(pattern))
                .map(|pattern| format!("{target}: {pattern}"))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "tool-call correlation API should use current wire/invocation terminology; offenders: {offenders:?}"
    );
}

#[test]
fn typed_tool_registry_has_single_production_definition() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    let mut files = Vec::new();
    collect_rust_files(&manifest_dir.join("src"), &mut files);

    let mut registry_definitions = Vec::new();
    let mut registry_aliases = Vec::new();
    for path in files {
        let source = fs::read_to_string(&path).expect("read source file");
        let relative = path
            .strip_prefix(manifest_dir)
            .expect("source path under manifest dir")
            .display()
            .to_string();

        if source.contains("pub struct ToolRegistry") {
            registry_definitions.push(relative.clone());
        }
        if source.contains("type ToolRegistry") {
            registry_aliases.push(relative);
        }
    }

    assert_eq!(
        registry_definitions,
        vec!["src/agent/tool_runtime/registry.rs"],
        "typed runtime must keep exactly one production ToolRegistry definition"
    );
    assert!(
        registry_aliases.is_empty(),
        "typed runtime must not add ToolRegistry type aliases or shadow registries; offenders: {registry_aliases:?}"
    );
}

#[test]
fn delegation_sub_agent_tools_use_tool_modules_not_provider_constructors() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let delegation_path = manifest_dir.join("src/agent/providers/delegation.rs");
    let delegation = fs::read_to_string(delegation_path).expect("read delegation provider");

    assert!(
        delegation.contains("ToolModuleContextParts")
            && delegation.contains("push_sub_agent_tool_module"),
        "sub-agent tools must be assembled through ToolModule context and module registration"
    );

    let forbidden_provider_paths = [
        "TodosProvider::new",
        "SandboxExecProvider::new",
        "SandboxFileOpsProvider::",
        "YtdlpProvider::",
        "WebFetchMdProvider::new",
        "TavilyProvider::new",
        "CrwProvider::new",
    ];
    let offenders = forbidden_provider_paths
        .iter()
        .copied()
        .filter(|pattern| delegation.contains(pattern))
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "sub-agent tool registration must not duplicate ToolModule provider construction; offenders: {offenders:?}"
    );
}

#[test]
fn workspace_binaries_expose_capability_manifest_output() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");
    let binary_entrypoints = [
        "crates/oxide-agent-telegram-bot/src/main.rs",
        "crates/oxide-agent-telegram-bot/src/bin/chatgpt-login.rs",
        "crates/oxide-agent-sandboxd/src/main.rs",
    ];

    let offenders = binary_entrypoints
        .iter()
        .filter_map(|target| {
            let source = fs::read_to_string(workspace_root.join(target))
                .unwrap_or_else(|error| panic!("read {target}: {error}"));
            let required_patterns = [
                "compiled_capability_manifest",
                "load_module_runtime_settings",
                "capabilities",
                "--compiled",
                "--enabled",
                "--json",
            ];
            let missing = required_patterns
                .iter()
                .copied()
                .filter(|pattern| !source.contains(pattern))
                .collect::<Vec<_>>();
            (!missing.is_empty()).then(|| format!("{target}: missing {missing:?}"))
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "all workspace binaries must expose deterministic capability manifest output; offenders: {offenders:?}"
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
        !store.contains("SqlxStorage"),
        "wiki memory must use StorageProvider-backed WikiObjectBackend, not a concrete storage backend"
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
    for forbidden in ["SqlxStorage::connect", "SqlxStorageConfig"] {
        assert!(
            !runner.contains(forbidden),
            "Telegram runner must use the storage backend factory instead of concrete backend {forbidden}"
        );
    }
}
