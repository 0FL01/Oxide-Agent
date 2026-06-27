//! Lazy tool protocol integration tests.
//!
//! Verifies the core lazy-tool invariants at the runner level:
//! - **M1/M8**: Hidden (deferred) tools are absent from the turn-1 LLM payload;
//!   after `retrieve_tools` activation, the turn-2 payload contains the
//!   activated tool schemas.
//! - **M2**: Execution resolves via the catalog-backed registry, not the
//!   visible surface — the registry contains all catalog tools regardless of
//!   surface state.
//! - **M5**: `refresh_visible_tools` produces a snapshot containing only
//!   always-visible + activated tools, not the full catalog.

#![cfg(test)]

use super::AgentRunner;
use super::test_support::{build_llm_client, stub_non_chat_methods};
use super::types::{AgentRunResult, AgentRunnerContext};

use crate::agent::memory::AgentMessage;
use crate::agent::tool_runtime::{
    CapabilityGroup, OutputNormalizer, ToolCatalog, ToolCatalogEntry, ToolExecutor, ToolInvocation,
    ToolName, ToolOutput, ToolRegistry as RuntimeToolRegistry, ToolRuntimeConfig, ToolRuntimeError,
    ToolSurfaceHandle, ToolVisibility,
};
use crate::agent::{AgentContext, AgentRunnerConfig as RunnerConfig, EphemeralSession, TodoList};
use crate::capabilities::ModuleId;
use crate::config::ModelInfo;
use crate::llm::{
    ChatResponse, MockLlmProvider, ToolCall, ToolCallCorrelation, ToolCallFunction, ToolDefinition,
    ToolProtocol, ToolTransport,
};
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

// ── Test executor ───────────────────────────────────────────────────────

/// A minimal executor that returns a canned success output.
///
/// Used for deferred tools in lazy-protocol tests.  The executors need to
/// exist in the catalog/registry so that `retrieve_tools` can activate them
/// and the runner can execute them if the model calls them, but in most tests
/// the model only calls `retrieve_tools` and then returns a final answer.
struct CannedExecutor {
    name: ToolName,
}

impl CannedExecutor {
    fn new(name: &str) -> Self {
        Self {
            name: ToolName::from(name),
        }
    }
}

#[async_trait]
impl ToolExecutor for CannedExecutor {
    fn name(&self) -> ToolName {
        self.name.clone()
    }

    fn spec(&self) -> ToolDefinition {
        ToolDefinition {
            name: self.name.as_str().to_string(),
            description: format!("canned {} for lazy tests", self.name),
            parameters: serde_json::json!({"type": "object"}),
        }
    }

    async fn execute(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            ..ToolRuntimeConfig::default()
        });
        Ok(normalizer.success(&invocation, "ok", ""))
    }
}

// ── Test setup ──────────────────────────────────────────────────────────

/// Deferred file tools used in tests.
const FILE_TOOLS: &[&str] = &["read_file", "write_file", "apply_file_edit", "list_files"];

/// Build a lazy test setup: catalog, surface handle, and execution registry.
///
/// The catalog contains:
/// - `retrieve_tools` (AlwaysVisible, no group) — bootstrap control tool.
/// - `write_todos` (AlwaysVisible, no group) — bootstrap.
/// - `read_file`, `write_file`, `apply_file_edit`, `list_files` (Deferred, Files).
/// - `execute_command` (Deferred, Shell).
///
/// The surface handle's group map is populated from the catalog.
/// The registry is built from the catalog via `to_registry()`.
fn build_lazy_test_setup() -> (
    Arc<ToolCatalog>,
    Arc<ToolSurfaceHandle>,
    Arc<RuntimeToolRegistry>,
) {
    use crate::agent::tool_runtime::retrieve_tools::RetrieveToolsExecutor;

    let handle = Arc::new(ToolSurfaceHandle::new());

    // retrieve_tools executor — needs the shared handle.
    let retrieve_executor: Arc<dyn ToolExecutor> =
        Arc::new(RetrieveToolsExecutor::new(Arc::clone(&handle)));

    let mut catalog = ToolCatalog::new();

    // AlwaysVisible: retrieve_tools (no group)
    catalog
        .register(ToolCatalogEntry::new(
            Arc::clone(&retrieve_executor),
            ModuleId::new("tool/retrieve-tools"),
            None,
            ToolVisibility::AlwaysVisible,
        ))
        .expect("register retrieve_tools");

    // AlwaysVisible: write_todos (no group)
    let todos_executor: Arc<dyn ToolExecutor> = Arc::new(CannedExecutor::new("write_todos"));
    catalog
        .register(ToolCatalogEntry::new(
            todos_executor,
            ModuleId::new("tool/todos"),
            None,
            ToolVisibility::AlwaysVisible,
        ))
        .expect("register write_todos");

    // Deferred: file tools (Files group)
    for name in FILE_TOOLS {
        let executor: Arc<dyn ToolExecutor> = Arc::new(CannedExecutor::new(name));
        catalog
            .register(ToolCatalogEntry::new(
                executor,
                ModuleId::new("tool/sandbox-fileops"),
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register file tool");
    }

    // Deferred: execute_command (Shell group)
    let exec_executor: Arc<dyn ToolExecutor> = Arc::new(CannedExecutor::new("execute_command"));
    catalog
        .register(ToolCatalogEntry::new(
            exec_executor,
            ModuleId::new("tool/sandbox-exec"),
            Some(CapabilityGroup::Shell),
            ToolVisibility::Deferred,
        ))
        .expect("register execute_command");

    // Populate the handle's group map from the catalog.
    let group_map = catalog.group_map();
    for (group, tool_names) in &group_map {
        handle.record_group_tools(*group, tool_names.iter().cloned().collect());
    }

    let registry = catalog.to_registry();

    (Arc::new(catalog), handle, Arc::new(registry))
}

/// Build a runner context with lazy tool surface enabled.
///
/// `todos_arc` must be created by the caller and live for the duration of the run.
fn build_lazy_runner_context<'a>(
    session: &'a mut EphemeralSession,
    messages: &'a mut Vec<crate::llm::Message>,
    todos_arc: &'a Arc<Mutex<TodoList>>,
    catalog: Arc<ToolCatalog>,
    handle: Arc<ToolSurfaceHandle>,
    registry: Arc<RuntimeToolRegistry>,
    tools: Vec<ToolDefinition>,
) -> AgentRunnerContext<'a> {
    let mut ctx = AgentRunnerContext {
        task: "lazy tool test",
        system_prompt: "system prompt",
        date_suffix: "",
        tools,
        tool_catalog: None,
        tool_surface_handle: None,
        tool_runtime_registry: Some(registry),
        progress_tx: None,
        todos_arc,
        task_id: "lazy-test",
        messages,
        agent: session,
        compaction_controller: None,
        session_id: Some("42".to_string()),
        memory_scope: None,
        memory_behavior: None,
        storage: None,
        config: RunnerConfig::new("deepseek-v4-flash".to_string(), 5, 1, 30, 1024)
            .with_model_provider("opencode-go")
            .with_model_routes(vec![ModelInfo {
                id: "deepseek-v4-flash".to_string(),
                provider: "opencode-go".to_string(),
                max_output_tokens: 1024,
                context_window_tokens: 8192,
                weight: 1,
            }]),
    };
    ctx = ctx.with_tool_surface(catalog, handle);
    ctx
}

/// Collect all tool names from a `ChatWithToolsRequest` capture.
fn tool_names_from_specs(specs: &[ToolDefinition]) -> Vec<String> {
    specs.iter().map(|s| s.name.clone()).collect()
}

// ── Tests ───────────────────────────────────────────────────────────────

/// M1/M8: Hidden (deferred) tools are absent from the turn-1 LLM payload.
///
/// The model should only see bootstrap tools (retrieve_tools, write_todos)
/// on the first turn.  File and shell tools must NOT appear until activated.
#[cfg(oxide_module_tool_retrieve_tools)]
#[tokio::test]
async fn hidden_deferred_tools_absent_from_turn_1_llm_payload() {
    let (catalog, handle, registry) = build_lazy_test_setup();

    // Compute initial visible specs (what the runner would send on turn 1).
    let initial_tools = handle.visible_specs(&catalog);
    let tool_names = tool_names_from_specs(&initial_tools);

    // Bootstrap tools must be present.
    assert!(
        tool_names.contains(&"retrieve_tools".to_string()),
        "retrieve_tools must be in bootstrap surface; got: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"write_todos".to_string()),
        "write_todos must be in bootstrap surface; got: {tool_names:?}"
    );

    // Deferred tools must be absent.
    for deferred in FILE_TOOLS {
        assert!(
            !tool_names.contains(&deferred.to_string()),
            "{deferred} must NOT be in bootstrap surface; got: {tool_names:?}"
        );
    }
    assert!(
        !tool_names.contains(&"execute_command".to_string()),
        "execute_command must NOT be in bootstrap surface; got: {tool_names:?}"
    );

    // Also verify via a full runner run that the LLM receives only bootstrap tools.
    let captured_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    let captured_clone = Arc::clone(&captured_tools);

    let mut provider = MockLlmProvider::new();
    provider
        .expect_chat_with_tools()
        .return_once(move |request| {
            let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            *captured_clone.lock().expect("lock captured tools") = names;
            Ok(ChatResponse {
                content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
    stub_non_chat_methods(&mut provider);

    let llm_client = build_llm_client(provider);
    let mut runner = AgentRunner::new(llm_client);

    let mut session = EphemeralSession::new(2048);
    session
        .memory_mut()
        .add_message(AgentMessage::user_task("Test lazy tool surface"));

    let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
    let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
    let mut ctx = build_lazy_runner_context(
        &mut session,
        &mut messages,
        &todos_arc,
        catalog,
        handle,
        registry,
        initial_tools,
    );
    let result = runner.run(&mut ctx).await.expect("runner succeeds");
    assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

    let captured = captured_tools.lock().expect("lock captured tools").clone();
    assert!(
        captured.contains(&"retrieve_tools".to_string()),
        "LLM must receive retrieve_tools; got: {captured:?}"
    );
    for deferred in FILE_TOOLS {
        assert!(
            !captured.contains(&deferred.to_string()),
            "LLM must NOT receive {deferred} on turn 1; got: {captured:?}"
        );
    }
}

/// M1/M8: After `retrieve_tools` activation, the turn-2 payload contains
/// the activated tool schemas.
///
/// Turn 1: model calls `retrieve_tools(["files"])` → Files group activated.
/// Turn 2: LLM should now see `read_file`, `write_file`, etc. in addition
/// to the bootstrap tools.
#[cfg(oxide_module_tool_retrieve_tools)]
#[tokio::test]
async fn post_activation_turn_2_payload_contains_activated_schemas() {
    let (catalog, handle, registry) = build_lazy_test_setup();
    let initial_tools = handle.visible_specs(&catalog);

    let turn1_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    let turn2_tools = Arc::new(std::sync::Mutex::new(Vec::new()));
    let turn1_clone = Arc::clone(&turn1_tools);
    let turn2_clone = Arc::clone(&turn2_tools);

    let mut provider = MockLlmProvider::new();
    let mut sequence = mockall::Sequence::new();

    // Turn 1: capture tools, return retrieve_tools call.
    provider
        .expect_chat_with_tools()
        .times(1)
        .in_sequence(&mut sequence)
        .return_once(move |request| {
            let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            *turn1_clone.lock().expect("lock turn1 tools") = names;
            Ok(ChatResponse {
                content: Some(String::new()),
                tool_calls: vec![
                    ToolCall::new(
                        "invoke-rt-1",
                        ToolCallFunction {
                            name: "retrieve_tools".to_string(),
                            arguments: r#"{"capabilities":["files"]}"#.to_string(),
                        },
                        false,
                    )
                    .with_correlation(
                        ToolCallCorrelation::new("invoke-rt-1")
                            .with_provider_tool_call_id("call-rt-1")
                            .with_protocol(ToolProtocol::ChatLike)
                            .with_transport(ToolTransport::ClientRoundTrip),
                    ),
                ],
                finish_reason: "tool_calls".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });

    // Turn 2: capture tools, return final answer.
    provider
        .expect_chat_with_tools()
        .times(1)
        .in_sequence(&mut sequence)
        .return_once(move |request| {
            let names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            *turn2_clone.lock().expect("lock turn2 tools") = names;
            Ok(ChatResponse {
                content: Some(r#"{"thought":"done","final_answer":"done"}"#.to_string()),
                tool_calls: Vec::new(),
                finish_reason: "stop".to_string(),
                reasoning_content: None,
                usage: None,
            })
        });
    stub_non_chat_methods(&mut provider);

    let llm_client = build_llm_client(provider);
    let mut runner = AgentRunner::new(llm_client);

    let mut session = EphemeralSession::new(2048);
    session
        .memory_mut()
        .add_message(AgentMessage::user_task("Activate file tools"));

    let todos_arc = Arc::new(Mutex::new(session.memory().todos.clone()));
    let mut messages = AgentRunner::convert_memory_to_messages(session.memory().get_messages());
    let mut ctx = build_lazy_runner_context(
        &mut session,
        &mut messages,
        &todos_arc,
        catalog,
        handle,
        registry,
        initial_tools,
    );

    let result = runner.run(&mut ctx).await.expect("runner succeeds");
    assert!(matches!(result, AgentRunResult::Final(answer) if answer == "done"));

    // Turn 1: only bootstrap tools.
    let t1 = turn1_tools.lock().expect("lock turn1 tools").clone();
    assert!(
        t1.contains(&"retrieve_tools".to_string()),
        "turn 1 must have retrieve_tools; got: {t1:?}"
    );
    for deferred in FILE_TOOLS {
        assert!(
            !t1.contains(&deferred.to_string()),
            "turn 1 must NOT have {deferred}; got: {t1:?}"
        );
    }

    // Turn 2: bootstrap + activated file tools.
    let t2 = turn2_tools.lock().expect("lock turn2 tools").clone();
    assert!(
        t2.contains(&"retrieve_tools".to_string()),
        "turn 2 must still have retrieve_tools; got: {t2:?}"
    );
    for file_tool in FILE_TOOLS {
        assert!(
            t2.contains(&file_tool.to_string()),
            "turn 2 must have {file_tool} after activation; got: {t2:?}"
        );
    }
    // Shell tools should still be absent (not activated).
    assert!(
        !t2.contains(&"execute_command".to_string()),
        "turn 2 must NOT have execute_command (not activated); got: {t2:?}"
    );
}

/// M2: Execution registry contains all catalog tools regardless of surface.
///
/// The `ToolRegistry` (execution handle) is built from the full catalog.
/// Even though the visible surface only shows bootstrap tools, the registry
/// can execute any catalog tool.  This is the "catalog = single execution
/// source of truth" invariant.
#[cfg(oxide_module_tool_retrieve_tools)]
#[test]
fn execution_registry_contains_all_catalog_tools_regardless_of_surface() {
    let (catalog, _handle, registry) = build_lazy_test_setup();

    // Registry must have ALL catalog tools.
    let catalog_names: Vec<String> = catalog
        .tool_names()
        .iter()
        .map(|n| n.as_str().to_string())
        .collect();
    let registry_names: Vec<String> = registry.specs().iter().map(|s| s.name.clone()).collect();

    assert_eq!(
        catalog_names, registry_names,
        "registry must contain exactly the catalog tools"
    );

    // Surface has only bootstrap, but registry has everything.
    assert!(registry_names.contains(&"read_file".to_string()));
    assert!(registry_names.contains(&"execute_command".to_string()));
    assert!(registry_names.contains(&"retrieve_tools".to_string()));
}

/// M5: `refresh_visible_tools` produces a snapshot with only visible tools.
///
/// Before activation: snapshot = bootstrap only (retrieve_tools + write_todos).
/// After activation: snapshot = bootstrap + activated group tools.
#[cfg(oxide_module_tool_retrieve_tools)]
#[test]
fn refresh_visible_tools_produces_bootstrap_only_snapshot() {
    let (catalog, handle, _registry) = build_lazy_test_setup();

    // Before activation: only bootstrap tools.
    let before = handle.visible_specs(&catalog);
    let before_names: Vec<String> = before.iter().map(|s| s.name.clone()).collect();
    assert_eq!(
        before_names.len(),
        2,
        "bootstrap surface should have exactly 2 tools; got: {before_names:?}"
    );
    assert!(before_names.contains(&"retrieve_tools".to_string()));
    assert!(before_names.contains(&"write_todos".to_string()));

    // Activate Files group.
    handle
        .activate_group(CapabilityGroup::Files)
        .expect("Files group should be activatable");

    // After activation: bootstrap + 4 file tools.
    let after = handle.visible_specs(&catalog);
    let after_names: Vec<String> = after.iter().map(|s| s.name.clone()).collect();
    assert_eq!(
        after_names.len(),
        6,
        "after activation surface should have 6 tools; got: {after_names:?}"
    );
    for file_tool in FILE_TOOLS {
        assert!(
            after_names.contains(&file_tool.to_string()),
            "{file_tool} should be visible after Files activation"
        );
    }
    // Shell still absent.
    assert!(!after_names.contains(&"execute_command".to_string()));

    // Activate Shell group — surface should grow.
    handle
        .activate_group(CapabilityGroup::Shell)
        .expect("Shell group should be activatable");
    let after2 = handle.visible_specs(&catalog);
    let after2_names: Vec<String> = after2.iter().map(|s| s.name.clone()).collect();
    assert_eq!(after2_names.len(), 7);
    assert!(after2_names.contains(&"execute_command".to_string()));
}

/// M8: `retrieve_tools` activation is idempotent — re-activating an
/// already-active group reports `already_active` without error.
#[cfg(oxide_module_tool_retrieve_tools)]
#[test]
fn retrieve_tools_activation_is_idempotent_and_monotonic() {
    let (_catalog, handle, _registry) = build_lazy_test_setup();

    // First activation.
    let result1 = handle
        .activate_group(CapabilityGroup::Files)
        .expect("first activation");
    assert!(
        !result1.activated.is_empty(),
        "first activation should activate tools"
    );
    assert!(
        result1.already_active.is_empty(),
        "nothing should be already active on first call"
    );

    // Second activation of same group.
    let result2 = handle
        .activate_group(CapabilityGroup::Files)
        .expect("second activation");
    assert!(
        result2.activated.is_empty(),
        "second activation should not activate new tools"
    );
    assert!(
        !result2.already_active.is_empty(),
        "second activation should report already_active"
    );

    // Surface did not grow beyond the first activation.
    let surface = handle.surface();
    let active = surface.active_names();
    let files_count = active
        .iter()
        .filter(|n| FILE_TOOLS.contains(&n.as_str()))
        .count();
    assert_eq!(
        files_count,
        FILE_TOOLS.len(),
        "surface should have exactly the file tools, no duplicates"
    );
}
