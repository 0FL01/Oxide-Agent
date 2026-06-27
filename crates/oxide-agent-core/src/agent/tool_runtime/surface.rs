//! Tool catalog and surface: domain split between executable catalog and
//! model-visible surface.
//!
//! The **catalog** is the full set of allowed, executable tools for a run.
//! It is internal — never fully serialized to the model.
//!
//! The **surface** is the live, ordered set of tool names currently visible to
//! the model.  Deferred tools reach the provider `tools[]` array only after
//! the model activates their capability group via `retrieve_tools`.
//!
//! Key invariants:
//! - Surface is monotonic within a run: once activated, a tool stays visible.
//! - Catalog is the single source of truth for execution.
//! - Always-visible (bootstrap) tools are visible from turn 1 and are not
//!   activatable via `retrieve_tools`.

use super::executor::ToolExecutor;
use super::registry::{RegistryError, ToolRegistry};
use super::types::ToolName;
use crate::capabilities::ModuleId;
use crate::llm::ToolDefinition;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, RwLock};

// ---------------------------------------------------------------------------
// CapabilityGroup
// ---------------------------------------------------------------------------

/// Coarse capability class for deferred tool activation.
///
/// Each deferred tool belongs to exactly one group.  The model activates a
/// group via the `retrieve_tools` control tool to make the group's tool schemas
/// visible in subsequent turns.
///
/// Always-visible (bootstrap) tools have `capability_group: None` and are not
/// activatable — they are visible from turn 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityGroup {
    /// Sandbox file operations: `read_file`, `write_file`, `apply_file_edit`,
    /// `list_files`, `send_file`.
    Files,
    /// Sandbox execution and lifecycle: `execute_command`, `recreate_sandbox`.
    Shell,
    /// Web search and fetch: `web_markdown` / `web_crawler`, `web_search`.
    Web,
    /// Autonomous browser: `browser_start`, `browser_observe`,
    /// `browser_execute`, `browser_extract`, `browser_debug`,
    /// `browser_save_screenshot`, `browser_close`.
    Browser,
    /// Wiki memory: `wiki_memory_list`, `wiki_memory_read`,
    /// `wiki_memory_delete`.
    Memory,
    /// Media analysis: `transcribe_audio_file`, `describe_image_file`,
    /// `describe_video_file`.
    Media,
    /// YouTube tools: `yt-dlp` metadata, transcript, download.
    Ytdlp,
    /// Text-to-speech: `tts`.
    Tts,
    /// Sub-agent delegation: `spawn_sub_agents`.
    Delegation,
    /// AGENTS.md self-editing: `agents_md_update`, `agents_md_read`.
    AgentsMd,
    /// Manager control-plane: topic/binding/context/infra/sandbox/profile/
    /// controls management.
    Manager,
    /// SSH MCP: `ssh_exec`, `sudo_exec`, `ssh_read_file`,
    /// `ssh_apply_file_edit`, `ssh_send_file_to_user`, `check_process`.
    Ssh,
    /// Stack logs: Docker Compose log access.
    StackLogs,
    /// Reminders: `reminder_create`, etc.
    Reminders,
    /// Jira MCP integration.
    Jira,
    /// Mattermost MCP integration.
    Mattermost,
}

impl CapabilityGroup {
    /// Stable string identifier used in the `retrieve_tools` schema enum.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Files => "files",
            Self::Shell => "shell",
            Self::Web => "web",
            Self::Browser => "browser",
            Self::Memory => "memory",
            Self::Media => "media",
            Self::Ytdlp => "ytdlp",
            Self::Tts => "tts",
            Self::Delegation => "delegation",
            Self::AgentsMd => "agents_md",
            Self::Manager => "manager",
            Self::Ssh => "ssh",
            Self::StackLogs => "stack_logs",
            Self::Reminders => "reminders",
            Self::Jira => "jira",
            Self::Mattermost => "mattermost",
        }
    }
}

impl std::str::FromStr for CapabilityGroup {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "files" => Ok(Self::Files),
            "shell" => Ok(Self::Shell),
            "web" => Ok(Self::Web),
            "browser" => Ok(Self::Browser),
            "memory" => Ok(Self::Memory),
            "media" => Ok(Self::Media),
            "ytdlp" => Ok(Self::Ytdlp),
            "tts" => Ok(Self::Tts),
            "delegation" => Ok(Self::Delegation),
            "agents_md" => Ok(Self::AgentsMd),
            "manager" => Ok(Self::Manager),
            "ssh" => Ok(Self::Ssh),
            "stack_logs" => Ok(Self::StackLogs),
            "reminders" => Ok(Self::Reminders),
            "jira" => Ok(Self::Jira),
            "mattermost" => Ok(Self::Mattermost),
            _ => Err(()),
        }
    }
}

impl CapabilityGroup {
    /// All defined group variants in canonical order.
    #[must_use]
    pub const fn all_variants() -> &'static [Self] {
        &[
            Self::Files,
            Self::Shell,
            Self::Web,
            Self::Browser,
            Self::Memory,
            Self::Media,
            Self::Ytdlp,
            Self::Tts,
            Self::Delegation,
            Self::AgentsMd,
            Self::Manager,
            Self::Ssh,
            Self::StackLogs,
            Self::Reminders,
            Self::Jira,
            Self::Mattermost,
        ]
    }
}

// ---------------------------------------------------------------------------
// ToolVisibility
// ---------------------------------------------------------------------------

/// Tool visibility within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolVisibility {
    /// Always in the bootstrap surface; visible to the model from turn 1.
    /// Not activatable via `retrieve_tools`.
    AlwaysVisible,
    /// Hidden until the model activates the tool's capability group via
    /// `retrieve_tools`.
    Deferred,
}

// ---------------------------------------------------------------------------
// ToolCatalogEntry
// ---------------------------------------------------------------------------

/// One entry in the executable tool catalog.
pub struct ToolCatalogEntry {
    /// Canonical tool name (cached from executor).
    pub name: ToolName,
    /// Executor for this tool.
    pub executor: Arc<dyn ToolExecutor>,
    /// Model-visible tool definition (cached from executor).
    pub spec: ToolDefinition,
    /// Compiled module that owns this tool.
    pub module_id: ModuleId,
    /// Capability group for deferred activation, or `None` for always-visible
    /// bootstrap tools.
    pub capability_group: Option<CapabilityGroup>,
    /// Whether this tool is always visible or deferred.
    pub visibility: ToolVisibility,
}

impl ToolCatalogEntry {
    /// Build a catalog entry from an executor and metadata.
    ///
    /// The tool name and spec are cached from the executor at construction
    /// time so the catalog is self-contained for spec queries without
    /// re-calling the executor.
    #[must_use]
    pub fn new(
        executor: Arc<dyn ToolExecutor>,
        module_id: ModuleId,
        capability_group: Option<CapabilityGroup>,
        visibility: ToolVisibility,
    ) -> Self {
        let name = executor.name();
        let spec = executor.spec();
        Self {
            name,
            executor,
            spec,
            module_id,
            capability_group,
            visibility,
        }
    }
}

// ---------------------------------------------------------------------------
// ToolCatalog
// ---------------------------------------------------------------------------

/// Full set of allowed, executable tools for a run.  Internal — never fully
/// serialized to the model.
///
/// Built once per run from the compiled tool modules.  The catalog is the
/// single source of truth for execution and spec queries.
#[derive(Default)]
pub struct ToolCatalog {
    entries: BTreeMap<ToolName, ToolCatalogEntry>,
}

impl ToolCatalog {
    /// Create an empty catalog.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Register one catalog entry.
    ///
    /// # Errors
    ///
    /// Returns `RegistryError::DuplicateTool` if the tool name already exists.
    pub fn register(&mut self, entry: ToolCatalogEntry) -> Result<(), RegistryError> {
        if self.entries.contains_key(&entry.name) {
            return Err(RegistryError::DuplicateTool { name: entry.name });
        }
        self.entries.insert(entry.name.clone(), entry);
        Ok(())
    }

    /// Lookup executor by exact canonical name.
    #[must_use]
    pub fn get_executor(&self, name: &ToolName) -> Option<Arc<dyn ToolExecutor>> {
        self.entries.get(name).map(|e| Arc::clone(&e.executor))
    }

    /// Whether the catalog contains a tool with the given name.
    #[must_use]
    pub fn contains(&self, name: &ToolName) -> bool {
        self.entries.contains_key(name)
    }

    /// Specs for all always-visible tools, deterministically ordered by name.
    #[must_use]
    pub fn always_visible_specs(&self) -> Vec<ToolDefinition> {
        self.entries
            .values()
            .filter(|e| e.visibility == ToolVisibility::AlwaysVisible)
            .map(|e| e.spec.clone())
            .collect()
    }

    /// Specs for the given names, deterministically ordered by name.
    ///
    /// Does **not** include always-visible tools — use [`visible_specs`](ToolSurface::visible_specs)
    /// on the surface for the full model-visible set.
    #[must_use]
    pub fn specs_for(&self, names: &BTreeSet<ToolName>) -> Vec<ToolDefinition> {
        self.entries
            .values()
            .filter(|e| names.contains(&e.name))
            .map(|e| e.spec.clone())
            .collect()
    }

    /// All entries belonging to a capability group.
    #[must_use]
    pub fn group_entries(&self, group: CapabilityGroup) -> Vec<&ToolCatalogEntry> {
        self.entries
            .values()
            .filter(|e| e.capability_group == Some(group))
            .collect()
    }

    /// Groups that have at least one deferred tool in this catalog.
    ///
    /// This drives the `retrieve_tools` schema enum: only groups present in
    /// the compiled catalog are offered to the model.
    #[must_use]
    pub fn activatable_groups(&self) -> Vec<CapabilityGroup> {
        let mut groups: BTreeSet<CapabilityGroup> = BTreeSet::new();
        for entry in self.entries.values() {
            if entry.visibility == ToolVisibility::Deferred
                && let Some(group) = entry.capability_group
            {
                groups.insert(group);
            }
        }
        groups.into_iter().collect()
    }

    /// All tool names, deterministically ordered.
    #[must_use]
    pub fn tool_names(&self) -> Vec<String> {
        self.entries
            .keys()
            .map(|name| name.as_str().to_string())
            .collect()
    }

    /// Iterate all catalog entries in canonical name order.
    pub fn entries(&self) -> impl Iterator<Item = &ToolCatalogEntry> {
        self.entries.values()
    }

    /// Number of catalog entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the catalog has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Mapping of capability group → tool names for deferred tools.
    ///
    /// Only deferred tools with a capability group are included.  Always-visible
    /// tools (group `None`) are excluded.  Used to populate the
    /// [`ToolSurfaceHandle`] group map during run bootstrap.
    #[must_use]
    pub fn group_map(&self) -> BTreeMap<CapabilityGroup, BTreeSet<ToolName>> {
        let mut map: BTreeMap<CapabilityGroup, BTreeSet<ToolName>> = BTreeMap::new();
        for entry in self.entries.values() {
            if entry.visibility == ToolVisibility::Deferred
                && let Some(group) = entry.capability_group
            {
                map.entry(group).or_default().insert(entry.name.clone());
            }
        }
        map
    }

    /// Build a [`ToolRegistry`] from the catalog's executors for execution.
    ///
    /// The registry is the execution handle; the catalog is the metadata
    /// source.  Both are built once per run.
    ///
    /// # Panics
    ///
    /// Cannot panic — catalog entries have unique names by construction
    /// (enforced by [`register`](Self::register)).
    #[must_use]
    pub fn to_registry(&self) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        for entry in self.entries.values() {
            registry
                .register(Arc::clone(&entry.executor))
                .expect("catalog entries have unique names by construction");
        }
        registry
    }
}

// ---------------------------------------------------------------------------
// ToolSurface
// ---------------------------------------------------------------------------

/// Live, ordered set of tool names currently visible to the model.
///
/// The surface is **monotonic within a run**: once a tool is activated, it
/// stays visible for the rest of the run (or until compaction rewrites
/// history).  The surface grows, never shrinks mid-run.
///
/// Always-visible tools are not tracked here — they are always included in
/// [`visible_specs`](Self::visible_specs) regardless of the active set.
#[derive(Default)]
pub struct ToolSurface {
    active: BTreeSet<ToolName>,
}

impl ToolSurface {
    /// Create an empty surface (no deferred tools activated).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            active: BTreeSet::new(),
        }
    }

    /// Specs for all tools visible to the model this turn.
    ///
    /// Includes all always-visible tools plus all activated deferred tools,
    /// deterministically ordered by tool name (BTreeMap key order).
    #[must_use]
    pub fn visible_specs(&self, catalog: &ToolCatalog) -> Vec<ToolDefinition> {
        catalog
            .entries
            .values()
            .filter(|e| {
                e.visibility == ToolVisibility::AlwaysVisible || self.active.contains(&e.name)
            })
            .map(|e| e.spec.clone())
            .collect()
    }

    /// Activate all tools in a capability group.
    ///
    /// Idempotent: tools already in the surface are reported as
    /// `already_active` and not re-activated.
    pub fn activate_group(
        &mut self,
        group: CapabilityGroup,
        catalog: &ToolCatalog,
    ) -> ActivationResult {
        let mut activated = Vec::new();
        let mut already_active = Vec::new();

        for entry in catalog.group_entries(group) {
            if self.active.contains(&entry.name) {
                already_active.push(entry.name.clone());
            } else {
                self.active.insert(entry.name.clone());
                activated.push(entry.name.clone());
            }
        }

        ActivationResult {
            activated,
            already_active,
            unknown_groups: Vec::new(),
        }
    }

    /// Mark a single tool name as active.
    ///
    /// Returns `true` if the name was newly inserted, `false` if already active.
    pub fn activate_name(&mut self, name: ToolName) -> bool {
        self.active.insert(name)
    }

    /// Whether a deferred tool name has been activated.
    ///
    /// Does **not** check always-visible tools — those are visible by
    /// definition, not by activation.
    #[must_use]
    pub fn contains(&self, name: &ToolName) -> bool {
        self.active.contains(name)
    }

    /// All active deferred tool names (not counting always-visible).
    #[must_use]
    pub fn active_names(&self) -> &BTreeSet<ToolName> {
        &self.active
    }

    /// Number of active deferred tools (not counting always-visible).
    #[must_use]
    pub fn active_len(&self) -> usize {
        self.active.len()
    }

    /// Whether no deferred tools have been activated.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.active.is_empty()
    }
}

// ---------------------------------------------------------------------------
// ActivationResult
// ---------------------------------------------------------------------------

/// Result of activating capability groups via `retrieve_tools`.
///
/// Returned to the model as a compact JSON payload so it knows which tools
/// are now available and which were already active.
pub struct ActivationResult {
    /// Tools that were newly activated in this call.
    pub activated: Vec<ToolName>,
    /// Tools that were already active (idempotent activation).
    pub already_active: Vec<ToolName>,
    /// Group names that don't match any catalog entry.
    pub unknown_groups: Vec<String>,
}

// ---------------------------------------------------------------------------
// ToolSurfaceHandle
// ---------------------------------------------------------------------------

/// Shared mutable state for the lazy tool surface.
///
/// Created at run start and shared between:
/// - The **runner** — reads [`surface`](Self::surface) to compute the
///   model-visible tool specs each iteration.
/// - The **`retrieve_tools` executor** — calls [`activate_group`](Self::activate_group)
///   to make deferred tool schemas visible to the model.
/// - **Module registration** — calls [`record_group_tools`](Self::record_group_tools)
///   during bootstrap to populate the group→tool-names mapping.
///
/// The surface is **monotonic within a run**: once a tool is activated, it
/// stays visible for the rest of the run.
///
/// The group map is populated incrementally during module registration and
/// then frozen.  At execution time, `activate_group` uses the group map to
/// resolve which tool names belong to a requested capability group — it does
/// not need the full [`ToolCatalog`].
pub struct ToolSurfaceHandle {
    /// Live tool surface (mutable, shared).
    surface: RwLock<ToolSurface>,
    /// Group → tool names mapping, populated during module registration.
    group_map: RwLock<BTreeMap<CapabilityGroup, BTreeSet<ToolName>>>,
}

impl ToolSurfaceHandle {
    /// Create an empty handle (no surface, no group map).
    #[must_use]
    pub fn new() -> Self {
        Self {
            surface: RwLock::new(ToolSurface::new()),
            group_map: RwLock::new(BTreeMap::new()),
        }
    }

    /// Record which tool names belong to a capability group.
    ///
    /// Called during module registration for each deferred module.  Accumulates
    /// names — if multiple modules share the same group, their tool names are
    /// merged.
    pub fn record_group_tools(&self, group: CapabilityGroup, names: Vec<ToolName>) {
        let mut map = self.group_map.write().expect("group_map lock poisoned");
        let entry = map.entry(group).or_default();
        for name in names {
            entry.insert(name);
        }
    }

    /// Activate all tools in a capability group.
    ///
    /// Uses the internal group map to resolve tool names — does not need the
    /// full catalog.  Idempotent: tools already active are reported as
    /// `already_active`.
    ///
    /// Returns [`None`] if the group is not in the group map (no tools
    /// registered for this group in this run).  The caller should treat this
    /// as an unknown/unavailable group.
    #[must_use]
    pub fn activate_group(&self, group: CapabilityGroup) -> Option<ActivationResult> {
        let names = {
            let map = self.group_map.read().expect("group_map lock poisoned");
            map.get(&group).cloned()
        };

        let names = names?;

        let mut surface = self.surface.write().expect("surface lock poisoned");
        let mut activated = Vec::new();
        let mut already_active = Vec::new();

        for name in &names {
            if surface.activate_name(name.clone()) {
                activated.push(name.clone());
            } else {
                already_active.push(name.clone());
            }
        }

        Some(ActivationResult {
            activated,
            already_active,
            unknown_groups: Vec::new(),
        })
    }

    /// Read access to the live surface.
    ///
    /// The guard allows callers to call [`ToolSurface::visible_specs`] with a
    /// catalog reference.
    pub fn surface(&self) -> std::sync::RwLockReadGuard<'_, ToolSurface> {
        self.surface.read().expect("surface lock poisoned")
    }

    /// Compute model-visible tool specs from the surface state and catalog.
    ///
    /// Convenience method: reads the surface guard, delegates to
    /// [`ToolSurface::visible_specs`].  The catalog provides the actual
    /// `ToolDefinition` for each visible tool name.
    #[must_use]
    pub fn visible_specs(&self, catalog: &ToolCatalog) -> Vec<ToolDefinition> {
        let surface = self.surface();
        surface.visible_specs(catalog)
    }

    /// Check whether a deferred tool name has been activated.
    #[must_use]
    pub fn contains(&self, name: &ToolName) -> bool {
        self.surface
            .read()
            .expect("surface lock poisoned")
            .contains(name)
    }

    /// All activatable groups (keys of the group map).
    #[must_use]
    pub fn activatable_groups(&self) -> Vec<CapabilityGroup> {
        self.group_map
            .read()
            .expect("group_map lock poisoned")
            .keys()
            .copied()
            .collect()
    }
}

impl Default for ToolSurfaceHandle {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::tool_runtime::invocation::ToolInvocation;
    use crate::agent::tool_runtime::normalizer::ToolRuntimeError;
    use crate::agent::tool_runtime::output::ToolOutput;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockExecutor {
        tool_name: ToolName,
    }

    #[async_trait]
    impl ToolExecutor for MockExecutor {
        fn name(&self) -> ToolName {
            self.tool_name.clone()
        }

        fn spec(&self) -> ToolDefinition {
            ToolDefinition {
                name: self.tool_name.as_str().to_string(),
                description: format!("mock {}", self.tool_name),
                parameters: json!({"type": "object"}),
            }
        }

        async fn execute(
            &self,
            _invocation: ToolInvocation,
        ) -> Result<ToolOutput, ToolRuntimeError> {
            unimplemented!("mock executor is not for execution")
        }
    }

    fn make_entry(
        name: &str,
        group: Option<CapabilityGroup>,
        visibility: ToolVisibility,
    ) -> ToolCatalogEntry {
        ToolCatalogEntry::new(
            Arc::new(MockExecutor {
                tool_name: ToolName::from(name),
            }),
            ModuleId::new("test/module"),
            group,
            visibility,
        )
    }

    // -- CapabilityGroup ---------------------------------------------------

    #[test]
    fn capability_group_round_trip() {
        for &group in CapabilityGroup::all_variants() {
            let s = group.as_str();
            assert_eq!(
                s.parse::<CapabilityGroup>().ok(),
                Some(group),
                "round-trip failed for {s}"
            );
        }
    }

    #[test]
    fn capability_group_from_str_unknown_returns_none() {
        assert_eq!("nonexistent".parse::<CapabilityGroup>().ok(), None);
        assert_eq!("".parse::<CapabilityGroup>().ok(), None);
    }

    // -- ToolCatalog -------------------------------------------------------

    #[test]
    fn catalog_register_and_lookup() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        assert!(catalog.contains(&ToolName::from("read_file")));
        assert!(catalog.get_executor(&ToolName::from("read_file")).is_some());
        assert!(!catalog.contains(&ToolName::from("missing")));
    }

    #[test]
    fn catalog_duplicate_fails() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("first");
        let err = catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect_err("duplicate");

        assert_eq!(
            err,
            RegistryError::DuplicateTool {
                name: ToolName::from("read_file")
            }
        );
    }

    #[test]
    fn always_visible_specs_excludes_deferred() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let specs = catalog.always_visible_specs();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["retrieve_tools"]);
    }

    #[test]
    fn specs_for_returns_only_requested() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let mut names = BTreeSet::new();
        names.insert(ToolName::from("read_file"));
        let specs = catalog.specs_for(&names);
        let spec_names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(spec_names, vec!["read_file"]);
    }

    #[test]
    fn group_entries_filters_by_group() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "execute_command",
                Some(CapabilityGroup::Shell),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");

        let files = catalog.group_entries(CapabilityGroup::Files);
        let names: Vec<&str> = files.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["read_file", "write_file"]);

        let shell = catalog.group_entries(CapabilityGroup::Shell);
        assert_eq!(shell.len(), 1);
    }

    #[test]
    fn activatable_groups_excludes_always_visible() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "execute_command",
                Some(CapabilityGroup::Shell),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let groups = catalog.activatable_groups();
        assert_eq!(groups, vec![CapabilityGroup::Files, CapabilityGroup::Shell]);
    }

    #[test]
    fn catalog_to_registry_preserves_executors() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let registry = catalog.to_registry();
        let names = registry.tool_names();
        assert_eq!(names, vec!["read_file", "write_file"]);
    }

    // -- ToolSurface -------------------------------------------------------

    #[test]
    fn surface_visible_specs_starts_with_always_visible_only() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let surface = ToolSurface::new();
        let specs = surface.visible_specs(&catalog);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["retrieve_tools"]);
    }

    #[test]
    fn surface_activate_group_adds_tools() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let mut surface = ToolSurface::new();
        let result = surface.activate_group(CapabilityGroup::Files, &catalog);

        assert_eq!(result.activated.len(), 2);
        assert!(result.already_active.is_empty());
        assert!(surface.contains(&ToolName::from("read_file")));
        assert!(surface.contains(&ToolName::from("write_file")));

        let specs = surface.visible_specs(&catalog);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["read_file", "retrieve_tools", "write_file"]);
    }

    #[test]
    fn surface_activate_group_is_idempotent() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let mut surface = ToolSurface::new();
        let first = surface.activate_group(CapabilityGroup::Files, &catalog);
        assert_eq!(first.activated.len(), 2);
        assert!(first.already_active.is_empty());

        let second = surface.activate_group(CapabilityGroup::Files, &catalog);
        assert!(second.activated.is_empty());
        assert_eq!(second.already_active.len(), 2);
    }

    #[test]
    fn surface_activate_empty_group_returns_empty() {
        let catalog = ToolCatalog::new();
        let mut surface = ToolSurface::new();
        let result = surface.activate_group(CapabilityGroup::Jira, &catalog);
        assert!(result.activated.is_empty());
        assert!(result.already_active.is_empty());
    }

    #[test]
    fn surface_is_monotonic() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "execute_command",
                Some(CapabilityGroup::Shell),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let mut surface = ToolSurface::new();
        surface.activate_group(CapabilityGroup::Files, &catalog);
        let visible_after_first: BTreeSet<String> = surface
            .visible_specs(&catalog)
            .iter()
            .map(|s| s.name.clone())
            .collect();

        surface.activate_group(CapabilityGroup::Shell, &catalog);
        let visible_after_second: BTreeSet<String> = surface
            .visible_specs(&catalog)
            .iter()
            .map(|s| s.name.clone())
            .collect();

        // Surface only grows — first activation's tools are still visible.
        assert!(
            visible_after_first.is_subset(&visible_after_second),
            "surface is not monotonic: {visible_after_first:?} not subset of {visible_after_second:?}"
        );
    }

    #[test]
    fn visible_specs_are_deterministic_by_name() {
        let mut catalog = ToolCatalog::new();
        // Register in non-alphabetical order.
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let mut surface = ToolSurface::new();
        surface.activate_group(CapabilityGroup::Files, &catalog);

        let specs = surface.visible_specs(&catalog);
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        // BTreeMap orders by name: read_file, retrieve_tools, write_file.
        assert_eq!(names, vec!["read_file", "retrieve_tools", "write_file"]);
    }

    // -- ToolSurface::activate_name ----------------------------------------

    #[test]
    fn activate_name_returns_true_for_new_false_for_existing() {
        let mut surface = ToolSurface::new();
        assert!(surface.activate_name(ToolName::from("read_file")));
        assert!(!surface.activate_name(ToolName::from("read_file")));
        assert!(surface.contains(&ToolName::from("read_file")));
    }

    // -- ToolCatalog::group_map --------------------------------------------

    #[test]
    fn catalog_group_map_excludes_always_visible() {
        let mut catalog = ToolCatalog::new();
        catalog
            .register(make_entry(
                "retrieve_tools",
                None,
                ToolVisibility::AlwaysVisible,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "read_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "write_file",
                Some(CapabilityGroup::Files),
                ToolVisibility::Deferred,
            ))
            .expect("register");
        catalog
            .register(make_entry(
                "execute_command",
                Some(CapabilityGroup::Shell),
                ToolVisibility::Deferred,
            ))
            .expect("register");

        let map = catalog.group_map();
        assert_eq!(map.len(), 2);
        let files = map.get(&CapabilityGroup::Files).expect("files group");
        assert_eq!(files.len(), 2);
        assert!(files.contains(&ToolName::from("read_file")));
        assert!(files.contains(&ToolName::from("write_file")));
        let shell = map.get(&CapabilityGroup::Shell).expect("shell group");
        assert_eq!(shell.len(), 1);
        assert!(shell.contains(&ToolName::from("execute_command")));
    }

    // -- ToolSurfaceHandle -------------------------------------------------

    #[test]
    fn handle_record_and_activate_group() {
        let handle = ToolSurfaceHandle::new();
        handle.record_group_tools(
            CapabilityGroup::Files,
            vec![ToolName::from("read_file"), ToolName::from("write_file")],
        );

        let result = handle
            .activate_group(CapabilityGroup::Files)
            .expect("group exists");
        assert_eq!(result.activated.len(), 2);
        assert!(result.already_active.is_empty());
        assert!(handle.contains(&ToolName::from("read_file")));
        assert!(handle.contains(&ToolName::from("write_file")));
    }

    #[test]
    fn handle_activate_group_idempotent() {
        let handle = ToolSurfaceHandle::new();
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);

        let first = handle
            .activate_group(CapabilityGroup::Files)
            .expect("first");
        assert_eq!(first.activated.len(), 1);
        assert!(first.already_active.is_empty());

        let second = handle
            .activate_group(CapabilityGroup::Files)
            .expect("second");
        assert!(second.activated.is_empty());
        assert_eq!(second.already_active.len(), 1);
    }

    #[test]
    fn handle_activate_unknown_group_returns_none() {
        let handle = ToolSurfaceHandle::new();
        assert!(handle.activate_group(CapabilityGroup::Files).is_none());
    }

    #[test]
    fn handle_activatable_groups() {
        let handle = ToolSurfaceHandle::new();
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);
        handle.record_group_tools(
            CapabilityGroup::Shell,
            vec![ToolName::from("execute_command")],
        );

        let groups = handle.activatable_groups();
        assert_eq!(groups, vec![CapabilityGroup::Files, CapabilityGroup::Shell]);
    }

    #[test]
    fn handle_record_merges_multiple_modules_same_group() {
        let handle = ToolSurfaceHandle::new();
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);
        handle.record_group_tools(
            CapabilityGroup::Files,
            vec![ToolName::from("write_file"), ToolName::from("list_files")],
        );

        let result = handle
            .activate_group(CapabilityGroup::Files)
            .expect("group");
        assert_eq!(result.activated.len(), 3);
    }

    #[test]
    fn handle_surface_monotonic_across_groups() {
        let handle = ToolSurfaceHandle::new();
        handle.record_group_tools(CapabilityGroup::Files, vec![ToolName::from("read_file")]);
        handle.record_group_tools(
            CapabilityGroup::Shell,
            vec![ToolName::from("execute_command")],
        );

        handle
            .activate_group(CapabilityGroup::Files)
            .expect("files");
        assert!(handle.contains(&ToolName::from("read_file")));

        handle
            .activate_group(CapabilityGroup::Shell)
            .expect("shell");
        assert!(handle.contains(&ToolName::from("read_file")));
        assert!(handle.contains(&ToolName::from("execute_command")));
    }
}
