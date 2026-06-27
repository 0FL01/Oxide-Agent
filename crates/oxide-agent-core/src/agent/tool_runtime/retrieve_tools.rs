//! `retrieve_tools` bootstrap tool — typed capability-group activation.
//!
//! This is the control tool that drives the lazy tool surface.  The model
//! calls `retrieve_tools` with a list of capability group names; the executor
//! activates the corresponding deferred tool schemas in the shared
//! [`ToolSurfaceHandle`], making them visible in subsequent turns.
//!
//! Key invariants:
//! - **AlwaysVisible**: `retrieve_tools` itself is in the bootstrap surface
//!   and is never deferred or self-activatable.
//! - **Idempotent**: activating an already-active group reports
//!   `already_active` — no duplicate activation, no error.
//! - **Policy-filtered**: the group→tools mapping is populated during module
//!   registration after build-time policy filtering.  Tools blocked by
//!   `ToolPolicy` are absent from the mapping; their groups may still be
//!   activatable if other tools in the group passed the filter.
//! - **No blocked-tool leakage**: the response lists only activated and
//!   already-active tool names.  Blocked tools are never mentioned.
//! - **No self-reference**: `retrieve_tools` has `capability_group: None` and
//!   cannot appear in any group's tool set.

use super::ToolRuntimeConfig;
use super::executor::ToolExecutor;
use super::invocation::ToolInvocation;
use super::modules::{ToolModule, ToolModuleContext};
use super::normalizer::{OutputNormalizer, ToolRuntimeError};
use super::output::ToolOutput;
use super::surface::{CapabilityGroup, ToolSurfaceHandle, ToolVisibility};
use super::types::ToolName;
use crate::capabilities::ModuleId;
use crate::llm::ToolDefinition;
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;
use tracing::debug;

/// Stable tool name.
const TOOL_RETRIEVE_TOOLS: &str = "retrieve_tools";

// ── Tool description ───────────────────────────────────────────────────

/// Human-readable description for the `retrieve_tools` tool.
fn retrieve_tools_description() -> &'static str {
    "Retrieve (activate) tool schemas for one or more capability groups. Tool schemas are \
     hidden until you activate their group — call this proactively when you know you will need \
     file access, shell execution, web search, or other capabilities. Activation is permanent \
     for the rest of the conversation: once a group is activated, its tools stay visible in all \
     subsequent turns. Idempotent — re-activating an already-active group reports \
     `already_active` without error. Pass the group names you need; the response lists which \
     tools were newly activated, which were already active, and which groups were unknown \
     (not available in this environment)."
}

/// JSON schema for the `retrieve_tools` arguments.
fn retrieve_tools_schema() -> Value {
    let groups: Vec<&'static str> = CapabilityGroup::all_variants()
        .iter()
        .map(|g| g.as_str())
        .collect();

    json!({
        "type": "object",
        "properties": {
            "capabilities": {
                "type": "array",
                "description": "Capability groups to activate. Available groups depend on the \
                                environment — unknown groups are reported in the response.",
                "items": {
                    "type": "string",
                    "enum": groups
                },
                "minItems": 1
            },
            "reason": {
                "type": "string",
                "description": "Brief explanation of why these capabilities are needed for the \
                                current task."
            }
        },
        "required": ["capabilities"],
        "additionalProperties": false
    })
}

// ── Result serialization ───────────────────────────────────────────────

/// Serialize the activation result as a compact JSON object for the model.
fn serialize_result(
    activated: &[ToolName],
    already_active: &[ToolName],
    unknown_groups: &[String],
) -> Value {
    json!({
        "activated": activated.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        "already_active": already_active.iter().map(|n| n.as_str()).collect::<Vec<_>>(),
        "unknown_groups": unknown_groups,
    })
}

// ── Argument parsing ───────────────────────────────────────────────────

/// Parsed arguments from the `retrieve_tools` invocation.
struct RetrieveToolsArgs {
    capabilities: Vec<String>,
    reason: Option<String>,
}

/// Parse the normalized arguments into typed `RetrieveToolsArgs`.
///
/// Uses `normalized_arguments` (parsed JSON object) when available, falling
/// back to parsing `raw_arguments` (string) for robustness.
fn parse_arguments(invocation: &ToolInvocation) -> Result<RetrieveToolsArgs, ToolRuntimeError> {
    let obj: Value = if !invocation.normalized_arguments.is_null() {
        invocation.normalized_arguments.clone()
    } else {
        serde_json::from_str(&invocation.raw_arguments).map_err(|e| {
            ToolRuntimeError::InvalidArguments(format!("failed to parse arguments: {e}"))
        })?
    };

    let obj = obj.as_object().ok_or_else(|| {
        ToolRuntimeError::InvalidArguments("arguments must be a JSON object".into())
    })?;

    let capabilities = obj
        .get("capabilities")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            ToolRuntimeError::InvalidArguments("missing required 'capabilities' array".into())
        })?;

    if capabilities.is_empty() {
        return Err(ToolRuntimeError::InvalidArguments(
            "'capabilities' must contain at least one group name".into(),
        ));
    }

    let caps: Vec<String> = capabilities
        .iter()
        .map(|v| {
            v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                ToolRuntimeError::InvalidArguments("'capabilities' entries must be strings".into())
            })
        })
        .collect::<Result<_, _>>()?;

    let reason = obj
        .get("reason")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(RetrieveToolsArgs {
        capabilities: caps,
        reason,
    })
}

// ── Executor ───────────────────────────────────────────────────────────

/// Tool executor for `retrieve_tools`.
///
/// Holds a shared [`ToolSurfaceHandle`] and activates capability groups in the
/// surface at execution time.  The handle's group map is populated during
/// module registration (before any tool executes).
pub struct RetrieveToolsExecutor {
    handle: Arc<ToolSurfaceHandle>,
}

impl RetrieveToolsExecutor {
    /// Create a new executor with the given surface handle.
    #[must_use]
    pub fn new(handle: Arc<ToolSurfaceHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl ToolExecutor for RetrieveToolsExecutor {
    fn name(&self) -> ToolName {
        ToolName::from(TOOL_RETRIEVE_TOOLS)
    }

    fn spec(&self) -> ToolDefinition {
        ToolDefinition {
            name: TOOL_RETRIEVE_TOOLS.to_string(),
            description: retrieve_tools_description().to_string(),
            parameters: retrieve_tools_schema(),
        }
    }

    async fn execute(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolRuntimeError> {
        let normalizer = OutputNormalizer::new(ToolRuntimeConfig {
            timeout: invocation.timeout.clone(),
            artifact_dir: invocation.execution_context.artifact_dir.clone(),
            ..ToolRuntimeConfig::default()
        });

        let args = parse_arguments(&invocation)?;

        if let Some(reason) = &args.reason {
            debug!(
                tool = TOOL_RETRIEVE_TOOLS,
                capabilities = ?args.capabilities,
                reason = %reason,
                "retrieve_tools called"
            );
        } else {
            debug!(
                tool = TOOL_RETRIEVE_TOOLS,
                capabilities = ?args.capabilities,
                "retrieve_tools called"
            );
        }

        let mut all_activated: Vec<ToolName> = Vec::new();
        let mut all_already_active: Vec<ToolName> = Vec::new();
        let mut unknown_groups: Vec<String> = Vec::new();

        for cap_str in &args.capabilities {
            match cap_str.parse::<CapabilityGroup>() {
                Ok(group) => {
                    match self.handle.activate_group(group) {
                        Some(result) => {
                            all_activated.extend(result.activated);
                            all_already_active.extend(result.already_active);
                        }
                        None => {
                            // Group exists as an enum variant but no tools are
                            // registered for it in this run.
                            unknown_groups.push(cap_str.clone());
                        }
                    }
                }
                Err(()) => {
                    unknown_groups.push(cap_str.clone());
                }
            }
        }

        let result = serialize_result(&all_activated, &all_already_active, &unknown_groups);
        let stdout = serde_json::to_string(&result)
            .map_err(|e| ToolRuntimeError::Internal(e.to_string()))?;

        let mut output = normalizer.success(&invocation, &stdout, "");
        output.structured_payload = Some(result);
        Ok(output)
    }
}

// ── Tool module ────────────────────────────────────────────────────────

/// Capability module for the `retrieve_tools` bootstrap tool.
///
/// This is an always-visible control tool.  It reads the shared
/// [`ToolSurfaceHandle`] from the module context and passes it to the
/// executor.
pub struct RetrieveToolsToolModule;

impl ToolModule for RetrieveToolsToolModule {
    fn module_id(&self) -> ModuleId {
        ModuleId::new("tool/retrieve-tools")
    }

    fn capability_group(&self) -> Option<CapabilityGroup> {
        None
    }

    fn visibility(&self) -> ToolVisibility {
        ToolVisibility::AlwaysVisible
    }

    fn tool_runtime_executors(&self, ctx: &ToolModuleContext) -> Vec<Arc<dyn ToolExecutor>> {
        vec![Arc::new(RetrieveToolsExecutor::new(
            ctx.tool_surface_handle(),
        ))]
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::identity::SessionId;
    use crate::agent::tool_runtime::{
        ModelMetadata, ProviderMetadata, ToolBatchId, ToolCallId, ToolExecutionContext,
        ToolInvocation, ToolOutputStatus, ToolTimeoutConfig, TurnId,
    };
    use crate::llm::InvocationId;
    use chrono::Utc;
    use tokio_util::sync::CancellationToken;

    fn make_invocation(args: Value) -> ToolInvocation {
        let now = Utc::now();
        ToolInvocation {
            session_id: SessionId::from(1),
            turn_id: TurnId::from("turn-test"),
            batch_id: ToolBatchId::from("batch-test"),
            batch_index: 0,
            invocation_id: InvocationId::from("invoke-test"),
            tool_call_id: ToolCallId::from("call-test"),
            provider_tool_call_id: None,
            tool_name: ToolName::from(TOOL_RETRIEVE_TOOLS),
            raw_provider_payload: json!({}),
            raw_arguments: serde_json::to_string(&args).unwrap_or_default(),
            normalized_arguments: args,
            cancellation_token: CancellationToken::new(),
            timeout: ToolTimeoutConfig::default(),
            execution_context: ToolExecutionContext::new(std::env::temp_dir()),
            provider_metadata: ProviderMetadata {
                provider: "test".to_string(),
                protocol: "chat_like".to_string(),
            },
            model_metadata: ModelMetadata {
                model: "test-model".to_string(),
            },
            working_directory: None,
            environment_metadata: None,
            created_at: now,
            started_at: Some(now),
        }
    }

    #[test]
    fn spec_has_correct_name() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        let executor = RetrieveToolsExecutor::new(handle);
        let spec = executor.spec();
        assert_eq!(spec.name, TOOL_RETRIEVE_TOOLS);
        assert!(!spec.description.is_empty());
    }

    #[test]
    fn schema_contains_all_group_enums() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        let executor = RetrieveToolsExecutor::new(handle);
        let spec = executor.spec();
        let params = &spec.parameters;
        let enum_vals = params
            .get("properties")
            .and_then(|p| p.get("capabilities"))
            .and_then(|c| c.get("items"))
            .and_then(|i| i.get("enum"))
            .and_then(|e| e.as_array())
            .expect("schema enum");
        let enum_strs: Vec<&str> = enum_vals.iter().filter_map(|v| v.as_str()).collect();
        for &group in CapabilityGroup::all_variants() {
            assert!(
                enum_strs.contains(&group.as_str()),
                "schema enum missing group '{}'",
                group.as_str()
            );
        }
    }

    #[tokio::test]
    async fn activate_files_group() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        handle.record_group_tools(
            CapabilityGroup::Files,
            vec![
                ToolName::from("read_file"),
                ToolName::from("write_file"),
                ToolName::from("apply_file_edit"),
                ToolName::from("list_files"),
            ],
        );

        let executor = RetrieveToolsExecutor::new(Arc::clone(&handle));
        let invocation = make_invocation(json!({
            "capabilities": ["files"],
            "reason": "need file access"
        }));

        let output = executor.execute(invocation).await.expect("execute");
        assert_eq!(output.status, ToolOutputStatus::Success);
        assert!(output.success);

        let payload = output.structured_payload.expect("payload");
        let activated = payload
            .get("activated")
            .and_then(|v| v.as_array())
            .expect("activated");
        assert_eq!(activated.len(), 4);
        let already = payload
            .get("already_active")
            .and_then(|v| v.as_array())
            .expect("already_active");
        assert!(already.is_empty());
        let unknown = payload
            .get("unknown_groups")
            .and_then(|v| v.as_array())
            .expect("unknown_groups");
        assert!(unknown.is_empty());

        // Surface now contains the activated tools.
        assert!(handle.contains(&ToolName::from("read_file")));
        assert!(handle.contains(&ToolName::from("write_file")));
    }

    #[tokio::test]
    async fn activate_already_active_group() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);

        // First activation.
        let executor = RetrieveToolsExecutor::new(Arc::clone(&handle));
        let inv1 = make_invocation(json!({"capabilities": ["files"]}));
        let out1 = executor.execute(inv1).await.expect("execute");
        let p1 = out1.structured_payload.expect("payload");
        assert_eq!(
            p1.get("activated")
                .and_then(|v| v.as_array())
                .expect("activated")
                .len(),
            1
        );
        assert!(
            p1.get("already_active")
                .and_then(|v| v.as_array())
                .expect("already_active")
                .is_empty()
        );

        // Second activation — idempotent.
        let inv2 = make_invocation(json!({"capabilities": ["files"]}));
        let out2 = executor.execute(inv2).await.expect("execute");
        let p2 = out2.structured_payload.expect("payload");
        assert!(
            p2.get("activated")
                .and_then(|v| v.as_array())
                .expect("activated")
                .is_empty()
        );
        assert_eq!(
            p2.get("already_active")
                .and_then(|v| v.as_array())
                .expect("already_active")
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn activate_multiple_groups() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);
        handle.record_group_tools(
            CapabilityGroup::Shell,
            vec![ToolName::from("execute_command")],
        );

        let executor = RetrieveToolsExecutor::new(Arc::clone(&handle));
        let invocation = make_invocation(json!({"capabilities": ["files", "shell"]}));
        let output = executor.execute(invocation).await.expect("execute");
        let payload = output.structured_payload.expect("payload");
        let activated = payload
            .get("activated")
            .and_then(|v| v.as_array())
            .expect("activated");
        assert_eq!(activated.len(), 2);
    }

    #[tokio::test]
    async fn unknown_group_string() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        let executor = RetrieveToolsExecutor::new(handle);
        let invocation = make_invocation(json!({"capabilities": ["nonexistent_group"]}));
        let output = executor.execute(invocation).await.expect("execute");
        let payload = output.structured_payload.expect("payload");
        let unknown = payload
            .get("unknown_groups")
            .and_then(|v| v.as_array())
            .expect("unknown_groups");
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0], "nonexistent_group");
    }

    #[tokio::test]
    async fn valid_group_not_in_map_is_unknown() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        // No tools recorded for any group.
        let executor = RetrieveToolsExecutor::new(handle);
        let invocation = make_invocation(json!({"capabilities": ["files"]}));
        let output = executor.execute(invocation).await.expect("execute");
        let payload = output.structured_payload.expect("payload");
        let unknown = payload
            .get("unknown_groups")
            .and_then(|v| v.as_array())
            .expect("unknown_groups");
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0], "files");
    }

    #[tokio::test]
    async fn missing_capabilities_field() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        let executor = RetrieveToolsExecutor::new(handle);
        let invocation = make_invocation(json!({"reason": "test"}));
        let result = executor.execute(invocation).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ToolRuntimeError::InvalidArguments(_))
        ));
    }

    #[tokio::test]
    async fn empty_capabilities_array() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        let executor = RetrieveToolsExecutor::new(handle);
        let invocation = make_invocation(json!({"capabilities": []}));
        let result = executor.execute(invocation).await;
        assert!(result.is_err());
        assert!(matches!(
            result.err(),
            Some(ToolRuntimeError::InvalidArguments(_))
        ));
    }

    #[tokio::test]
    async fn mixed_known_and_unknown_groups() {
        let handle = Arc::new(ToolSurfaceHandle::new());
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);

        let executor = RetrieveToolsExecutor::new(Arc::clone(&handle));
        let invocation = make_invocation(json!({
            "capabilities": ["files", "nonexistent", "shell"]
        }));
        let output = executor.execute(invocation).await.expect("execute");
        let payload = output.structured_payload.expect("payload");

        // files was activated, shell has no tools (unknown), nonexistent is not a valid group.
        let activated = payload
            .get("activated")
            .and_then(|v| v.as_array())
            .expect("activated");
        assert_eq!(activated.len(), 1);

        let unknown = payload
            .get("unknown_groups")
            .and_then(|v| v.as_array())
            .expect("unknown_groups");
        assert_eq!(unknown.len(), 2);
        // BTreeSet ordering in unknown_groups: "nonexistent" < "shell"
        let unknown_strs: Vec<&str> = unknown.iter().filter_map(|v| v.as_str()).collect();
        assert!(unknown_strs.contains(&"nonexistent"));
        assert!(unknown_strs.contains(&"shell"));
    }
}
