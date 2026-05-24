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
fn legacy_tool_registry_and_wrappers_are_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        !manifest_dir.join("src/agent/registry.rs").exists(),
        "legacy agent/registry.rs must stay removed; use agent::tool_runtime::ToolRegistry"
    );

    let agent_mod =
        fs::read_to_string(manifest_dir.join("src/agent/mod.rs")).expect("read agent/mod.rs");
    for forbidden in ["pub mod registry;", "pub use registry"] {
        assert!(
            !agent_mod.contains(forbidden),
            "agent module must not export legacy registry symbol {forbidden}"
        );
    }

    let forbidden_patterns = [
        "agent::registry::",
        "crate::agent::registry::",
        "crate::agent::registry;",
        "build_tool_registry",
        "legacy_provider",
        "ToolModule::legacy_provider",
        "FilteredToolProvider",
        "ProviderRuntimeExecutor",
        "provider_runtime_executors",
        "provider_executor",
    ];

    let mut files = Vec::new();
    collect_rust_files(&manifest_dir.join("src"), &mut files);
    let offenders = files
        .into_iter()
        .filter_map(|path| {
            let source = fs::read_to_string(&path).expect("read source file");
            let matches = forbidden_patterns
                .iter()
                .copied()
                .filter(|pattern| source.contains(pattern))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                None
            } else {
                Some(format!(
                    "{}: {matches:?}",
                    path.strip_prefix(manifest_dir)
                        .expect("source path under manifest dir")
                        .display()
                ))
            }
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "legacy tool registry/wrapper references must stay removed; offenders: {offenders:?}"
    );
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
        "SearxngProvider::new",
        "BrowserUseProvider::",
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
fn deprecated_config_compatibility_surfaces_are_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");

    let config = fs::read_to_string(manifest_dir.join("src/config.rs")).expect("read config");
    let forbidden_config_patterns = [
        "#[serde(alias",
        "browser_use_model_max_tokens",
        "chat_model_max_tokens",
        "agent_model_max_tokens",
        "sub_agent_max_tokens",
        "wiki_memory_writer_model_max_tokens",
        "oxide_codex_style_compaction",
        "OXIDE_CODEX_STYLE_COMPACTION",
        "ZAI_CHAT_TEMPERATURE",
        "#[deprecated(",
    ];
    let config_offenders = forbidden_config_patterns
        .iter()
        .copied()
        .filter(|pattern| config.contains(pattern))
        .collect::<Vec<_>>();

    assert!(
        config_offenders.is_empty(),
        "old config aliases, deprecated fields, and migration switches must stay removed; offenders: {config_offenders:?}"
    );

    let telegram_config = fs::read_to_string(
        workspace_root.join("crates/oxide-agent-transport-telegram/src/config.rs"),
    )
    .expect("read telegram config");
    assert!(
        !telegram_config.contains("#[serde(alias") && !telegram_config.contains("alias ="),
        "transport config must not keep serde compatibility aliases"
    );

    let env_example = fs::read_to_string(workspace_root.join(".env.example"))
        .expect("read workspace .env.example");
    for forbidden in [
        "OXIDE_CODEX_STYLE_COMPACTION",
        "BROWSER_USE_BRIDGE_LLM_PROVIDER",
        "BROWSER_USE_BRIDGE_LLM_MODEL",
    ] {
        assert!(
            !env_example.contains(forbidden),
            ".env.example must not document removed temporary migration switches or sidecar LLM fallbacks: {forbidden}"
        );
    }

    let executor = fs::read_to_string(manifest_dir.join("src/agent/executor.rs"))
        .expect("read executor module");
    for forbidden in ["backward compatibility", "public_sanitize_xml_tags"] {
        assert!(
            !executor.contains(forbidden),
            "executor module must not keep backward-compatibility re-exports: {forbidden}"
        );
    }
}

#[test]
fn stale_compatibility_labels_are_removed_from_current_surfaces() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");
    let targets = [
        "crates/oxide-agent-core/src/agent/hooks/memory.rs",
        "crates/oxide-agent-core/src/agent/providers/reminder.rs",
        "crates/oxide-agent-core/src/agent/providers/ssh_mcp.rs",
        "crates/oxide-agent-core/src/agent/runner/execution.rs",
        "crates/oxide-agent-core/src/capabilities/compiled.rs",
        "crates/oxide-agent-core/src/llm/client.rs",
        "crates/oxide-agent-core/src/llm/mod.rs",
        "crates/oxide-agent-core/src/llm/providers/mistral/chat.rs",
        "crates/oxide-agent-core/src/llm/providers/openrouter.rs",
        "crates/oxide-agent-core/src/llm/providers/openrouter/module.rs",
        "crates/oxide-agent-core/src/llm/types.rs",
        "crates/oxide-agent-core/src/storage/control_plane.rs",
        "crates/oxide-agent-core/src/storage/telemetry.rs",
        "crates/oxide-agent-core/src/storage/tests/bindings.rs",
        "crates/oxide-agent-core/src/storage/tests/keys_and_user.rs",
        "crates/oxide-agent-transport-telegram/src/bot/context.rs",
        "crates/oxide-agent-transport-web/src/in_memory_storage.rs",
        "crates/oxide-agent-transport-web/src/server.rs",
    ];
    let forbidden_patterns = [
        "chat_with_tools_once",
        "kept for backwards compatibility",
        "kept for compatibility",
        "Legacy/internal identifier",
        "Legacy version without ID mapping",
        "legacy-provider-id",
        "legacy_chat_history",
        "should_use_legacy_fallback",
        "fall_back_to_legacy_global_state",
        "topic_binding_record_backward_compatible_deserialization_defaults_new_fields",
        "#[serde(default)]\n    pub binding_kind",
        "schedule_args_reject_legacy_fields",
        "legacy episode memory tools",
        "legacy process pattern",
        "Legacy substring",
        "site_url",
        "site_name",
        "OPENROUTER_SITE_URL",
        "OPENROUTER_SITE_NAME",
        "Deprecated: App attribution headers",
    ];

    let offenders = targets
        .iter()
        .flat_map(|target| {
            let source = fs::read_to_string(workspace_root.join(target))
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
        "stale compatibility labels and deprecated config surfaces must stay removed from current paths; offenders: {offenders:?}"
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
        "all workspace binaries must expose deterministic capability manifest output for PRD 8.5/25; offenders: {offenders:?}"
    );
}

#[test]
fn legacy_compaction_archive_compatibility_surfaces_are_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let targets = [
        "src/agent/compaction/archive.rs",
        "src/agent/compaction/history.rs",
        "src/agent/compaction/mod.rs",
        "src/agent/compaction/types.rs",
        "src/agent/memory.rs",
    ];
    let forbidden_patterns = [
        "ArchiveChunk",
        "ArchiveRecord",
        "BreadcrumbCard",
        "CompactionSummary",
        "LEGACY_COMPACTION_SUMMARY_PREFIX",
        "LEGACY_BREADCRUMB_PREFIX",
        "AgentMessageKind::Breadcrumb",
        "AgentMessageKind::ArchiveReference",
        "AgentMessageKind::Legacy",
        "Legacy,",
        "kind: AgentMessageKind::Legacy",
        "#[serde(default)]\n    pub kind",
        "structured_summary",
        "breadcrumb_card",
        "archive_ref_payload",
        "summary_payload",
        "breadcrumb_payload",
        "from_compaction_summary",
        "from_breadcrumb_card",
        "archive_reference_with_ref",
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
        "legacy staged-compaction archive/breadcrumb compatibility surfaces must stay removed; offenders: {offenders:?}"
    );
}

#[test]
fn startup_persisted_tool_drift_cleanup_is_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");

    assert!(
        !workspace_root
            .join("crates/oxide-agent-transport-telegram/src/startup_maintenance.rs")
            .exists(),
        "startup persisted-memory drift cleanup was a deployment migration path and must stay removed"
    );

    let targets = [
        "crates/oxide-agent-transport-telegram/src/lib.rs",
        "crates/oxide-agent-transport-telegram/src/runner.rs",
        "crates/oxide-agent-core/src/storage/mod.rs",
        "crates/oxide-agent-core/src/storage/modules.rs",
        "crates/oxide-agent-core/src/storage/provider.rs",
        "crates/oxide-agent-core/src/storage/r2_memory.rs",
    ];
    let forbidden_patterns = [
        "startup_maintenance",
        "run_startup_tool_drift_prune",
        "startup-tool-drift-prune",
        "STARTUP_TOOL_DRIFT_PRUNE",
        "PersistedAgentMemoryStore",
        "PersistedAgentMemoryRef",
        "list_persisted_agent_memories",
        "persisted_agent_memory",
    ];

    let offenders = targets
        .iter()
        .flat_map(|target| {
            let source = fs::read_to_string(workspace_root.join(target))
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
        "startup persisted-memory migration cleanup surfaces must stay removed; offenders: {offenders:?}"
    );
}

#[test]
fn transport_flow_memory_migration_path_is_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");
    let session = fs::read_to_string(
        workspace_root
            .join("crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs"),
    )
    .expect("read Telegram agent session handlers");

    let forbidden_patterns = [
        "migrate_legacy_agent_memory_into_flow",
        "Migrated legacy agent memory",
        "Failed to migrate legacy agent memory",
        "load_agent_memory_for_context(ctx.user_id",
    ];
    let offenders = forbidden_patterns
        .iter()
        .copied()
        .filter(|pattern| session.contains(pattern))
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "flow bootstrap must not migrate old context-level memory into flow-scoped storage; offenders: {offenders:?}"
    );
}

#[test]
fn telegram_agent_sessions_use_only_scoped_primary_identity() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("core crate lives under workspace/crates");
    let targets = [
        "crates/oxide-agent-transport-telegram/src/bot/agent_handlers/session.rs",
        "crates/oxide-agent-transport-telegram/src/bot/agent_handlers/callbacks.rs",
        "crates/oxide-agent-transport-telegram/src/bot/agent_handlers/controls.rs",
    ];
    let forbidden_patterns = [
        "legacy: SessionId",
        "distinct_legacy",
        "keys.legacy",
        "SessionId::from(user_id)",
        "reset_sessions_with_compat",
        "cancel_and_clear_with_compat",
        "remove_sessions_with_compat",
        "migrate_to_primary",
    ];

    let offenders = targets
        .iter()
        .flat_map(|target| {
            let source = fs::read_to_string(workspace_root.join(target))
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
        "Telegram Agent Mode sessions must not fall back to unscoped legacy user-id sessions; offenders: {offenders:?}"
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
