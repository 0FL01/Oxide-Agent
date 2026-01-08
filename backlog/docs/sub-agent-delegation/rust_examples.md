# Sub-Agent Rust Implementation Examples

## Basic Agent Registry

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    Primary,
    Subagent,
    All,
}

#[derive(Debug, Clone)]
pub struct Agent {
    pub name: String,
    pub mode: AgentMode,
    pub description: Option<String>,
    pub permission: PermissionRuleset,
    pub model: Option<ModelConfig>,
    pub prompt: Option<String>,
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub hidden: bool,
}

pub struct AgentRegistry {
    agents: Arc<RwLock<HashMap<String, Agent>>>,
}

impl AgentRegistry {
    pub async fn new() -> Self {
        let mut agents = HashMap::new();

        // Built-in primary agents
        agents.insert("build".to_string(), Agent {
            name: "build".to_string(),
            mode: AgentMode::Primary,
            description: Some("Default agent for development work".to_string()),
            permission: PermissionRuleset::default_with_all_allow(),
            model: None,
            prompt: None,
            temperature: None,
            top_p: None,
            hidden: false,
        });

        // Built-in subagents
        agents.insert("general".to_string(), Agent {
            name: "general".to_string(),
            mode: AgentMode::Subagent,
            description: Some("General-purpose agent for complex tasks".to_string()),
            permission: PermissionRuleset {
                rules: vec![
                    PermissionRule {
                        permission: "todowrite".to_string(),
                        pattern: "*".to_string(),
                        action: PermissionAction::Deny,
                    },
                    PermissionRule {
                        permission: "todoread".to_string(),
                        pattern: "*".to_string(),
                        action: PermissionAction::Deny,
                    },
                ],
            },
            model: None,
            prompt: None,
            temperature: None,
            top_p: None,
            hidden: true,
        });

        agents.insert("explore".to_string(), Agent {
            name: "explore".to_string(),
            mode: AgentMode::Subagent,
            description: Some("Fast agent for codebase exploration".to_string()),
            permission: PermissionRuleset {
                rules: vec![
                    PermissionRule { permission: "grep".to_string(), pattern: "*".to_string(), action: PermissionAction::Allow },
                    PermissionRule { permission: "glob".to_string(), pattern: "*".to_string(), action: PermissionAction::Allow },
                    PermissionRule { permission: "read".to_string(), pattern: "*".to_string(), action: PermissionAction::Allow },
                    PermissionRule { permission: "bash".to_string(), pattern: "*".to_string(), action: PermissionAction::Allow },
                ],
            },
            model: None,
            prompt: None,
            temperature: None,
            top_p: None,
            hidden: false,
        });

        Self {
            agents: Arc::new(RwLock::new(agents)),
        }
    }

    pub async fn get(&self, name: &str) -> Option<Agent> {
        self.agents.read().await.get(name).cloned()
    }

    pub async fn list(&self) -> Vec<Agent> {
        self.agents.read().await.values().cloned().collect()
    }

    pub async fn list_subagents(&self) -> Vec<Agent> {
        self.agents.read()
            .await
            .values()
            .filter(|a| a.mode == AgentMode::Subagent && !a.hidden)
            .cloned()
            .collect()
    }

    pub async fn register(&self, agent: Agent) -> Result<(), String> {
        let mut agents = self.agents.write().await;
        agents.insert(agent.name.clone(), agent);
        Ok(())
    }
}
```

## Session Management

```rust
use uuid::Uuid;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub project_id: String,
    pub title: String,
    pub permission: Option<PermissionRuleset>,
    pub time: SessionTime,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionTime {
    pub created: i64,
    pub updated: i64,
}

pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<String, Session>>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn create(&self, params: CreateSessionParams) -> Result<Session, SessionError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().timestamp();

        let session = Session {
            id: id.clone(),
            parent_id: params.parent_id,
            project_id: params.project_id,
            title: params.title,
            permission: params.permission,
            time: SessionTime { created: now, updated: now },
            messages: Vec::new(),
        };

        let mut sessions = self.sessions.write().await;
        sessions.insert(id.clone(), session.clone());

        Ok(session)
    }

    pub async fn get(&self, id: &str) -> Option<Session> {
        self.sessions.read().await.get(id).cloned()
    }

    pub async fn get_children(&self, parent_id: &str) -> Vec<Session> {
        self.sessions.read()
            .await
            .values()
            .filter(|s| s.parent_id.as_ref().map(|p| p == parent_id).unwrap_or(false))
            .cloned()
            .collect()
    }

    pub async fn get_parent(&self, session_id: &str) -> Option<Session> {
        let session = self.get(session_id).await?;
        session.parent_id.and_then(|parent_id| self.get(&parent_id).await)
    }

    pub async fn navigate_next(&self, current_id: &str) -> Option<Session> {
        // Navigate to child if exists, otherwise to parent's next child
        let current = self.get(current_id).await?;

        // First try to get first child
        let children = self.get_children(current_id).await;
        if let Some(first_child) = children.first() {
            return Some(first_child.clone());
        }

        // If no children, navigate to parent
        if let Some(parent_id) = &current.parent_id {
            self.get(parent_id).await
        } else {
            None
        }
    }

    pub async fn navigate_previous(&self, current_id: &str) -> Option<Session> {
        let current = self.get(current_id).await?;

        // Navigate to parent
        if let Some(parent_id) = &current.parent_id {
            self.get(parent_id).await
        } else {
            None
        }
    }

    pub async fn update(&self, id: &str, updates: SessionUpdates) -> Result<(), SessionError> {
        let mut sessions = self.sessions.write().await;

        let session = sessions.get_mut(id)
            .ok_or_else(|| SessionError::NotFound(id.to_string()))?;

        if let Some(title) = updates.title {
            session.title = title;
        }
        session.time.updated = Utc::now().timestamp();

        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct CreateSessionParams {
    pub parent_id: Option<String>,
    pub project_id: String,
    pub title: String,
    pub permission: Option<PermissionRuleset>,
}

#[derive(Debug, Default)]
pub struct SessionUpdates {
    pub title: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found: {0}")]
    NotFound(String),

    #[error("Invalid session data: {0}")]
    InvalidData(String),
}
```

## Task Tool Implementation

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct TaskParams {
    pub description: String,
    pub prompt: String,
    pub subagent_type: String,
    pub session_id: Option<String>,
    pub command: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TaskResult {
    pub title: String,
    pub metadata: TaskMetadata,
    pub output: String,
}

#[derive(Debug, Serialize)]
pub struct TaskMetadata {
    pub session_id: String,
    pub summary: Vec<ToolCallSummary>,
}

#[derive(Debug, Serialize)]
pub struct ToolCallSummary {
    pub id: String,
    pub tool: String,
    pub state: ToolState,
}

#[derive(Debug, Serialize)]
pub struct ToolState {
    pub status: String,
    pub title: Option<String>,
}

pub struct TaskTool {
    registry: Arc<AgentRegistry>,
    session_manager: Arc<SessionManager>,
    permission_checker: Arc<PermissionChecker>,
}

impl TaskTool {
    pub fn new(
        registry: Arc<AgentRegistry>,
        session_manager: Arc<SessionManager>,
        permission_checker: Arc<PermissionChecker>,
    ) -> Self {
        Self {
            registry,
            session_manager,
            permission_checker,
        }
    }

    pub async fn execute(
        &self,
        params: TaskParams,
        ctx: ToolContext,
    ) -> Result<TaskResult, TaskError> {
        // 1. Check permission to spawn subagent
        self.permission_checker.check(
            &ctx.session_id,
            "task",
            &params.subagent_type,
        ).await?;

        // 2. Get agent configuration
        let agent = self.registry.get(&params.subagent_type)
            .await
            .ok_or_else(|| TaskError::AgentNotFound(params.subagent_type.clone()))?;

        // 3. Create child session with restricted permissions
        let session = self.session_manager.create(CreateSessionParams {
            parent_id: Some(ctx.session_id.clone()),
            project_id: ctx.project_id.clone(),
            title: format!("{} (@{} subagent)", params.description, agent.name),
            permission: Some(self.create_restricted_permission()),
        }).await?;

        // 4. Execute prompt in child session
        let result = self.execute_prompt(
            &session,
            &params.prompt,
            &agent,
            &ctx,
        ).await?;

        // 5. Return result to parent
        Ok(TaskResult {
            title: params.description,
            metadata: TaskMetadata {
                session_id: session.id,
                summary: result.tool_calls,
            },
            output: result.text,
        })
    }

    fn create_restricted_permission(&self) -> PermissionRuleset {
        PermissionRuleset {
            rules: vec![
                PermissionRule {
                    permission: "todowrite".to_string(),
                    pattern: "*".to_string(),
                    action: PermissionAction::Deny,
                },
                PermissionRule {
                    permission: "todoread".to_string(),
                    pattern: "*".to_string(),
                    action: PermissionAction::Deny,
                },
                PermissionRule {
                    permission: "task".to_string(),
                    pattern: "*".to_string(),
                    action: PermissionAction::Deny,
                },
            ],
        }
    }

    async fn execute_prompt(
        &self,
        session: &Session,
        prompt: &str,
        agent: &Agent,
        ctx: &ToolContext,
    ) -> Result<PromptExecutionResult, TaskError> {
        // In a real implementation, this would:
        // 1. Create a message in the session
        // 2. Execute the LLM prompt
        // 3. Process tool calls
        // 4. Return results

        Ok(PromptExecutionResult {
            text: "Task completed successfully".to_string(),
            tool_calls: vec![],
        })
    }
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub session_id: String,
    pub project_id: String,
    pub message_id: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Permission denied: {0}")]
    PermissionDenied(String),

    #[error("Session error: {0}")]
    SessionError(#[from] SessionError),
}

#[derive(Debug)]
struct PromptExecutionResult {
    text: String,
    tool_calls: Vec<ToolCallSummary>,
}
```

## Event Bus Implementation

```rust
use tokio::sync::broadcast;
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SessionEvent {
    #[serde(rename = "session.created")]
    Created { info: Session },

    #[serde(rename = "session.updated")]
    Updated { info: Session },

    #[serde(rename = "message.created")]
    MessageCreated { session_id: String, message: Message },

    #[serde(rename = "part.updated")]
    PartUpdated {
        session_id: String,
        message_id: String,
        part: Part,
    },
}

pub struct EventBus {
    sender: broadcast::Sender<SessionEvent>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1000);
        Self { sender }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SessionEvent> {
        self.sender.subscribe()
    }

    pub fn publish(&self, event: SessionEvent) {
        let _ = self.sender.send(event);
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

// Example: Subscribing to part updates for progress tracking
pub async fn track_task_progress(
    event_bus: Arc<EventBus>,
    session_id: String,
    message_id: String,
) -> tokio::task::JoinHandle<HashMap<String, ToolState>> {
    tokio::spawn(async move {
        let mut rx = event_bus.subscribe();
        let mut tool_states: HashMap<String, ToolState> = HashMap::new();

        loop {
            match rx.recv().await {
                Ok(SessionEvent::PartUpdated { session_id: evt_session_id, message_id: evt_message_id, part }) => {
                    if evt_session_id != session_id || evt_message_id != message_id {
                        continue;
                    }

                    if part.part_type != "tool" {
                        continue;
                    }

                    tool_states.insert(part.id.clone(), ToolState {
                        status: part.state.status.clone(),
                        title: if part.state.status == "completed" {
                            part.state.title.clone()
                        } else {
                            None
                        },
                    });
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // Handle lag if necessary
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
                }
            }
        }

        tool_states
    })
}
```

## Permission System

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PermissionAction {
    #[serde(rename = "allow")]
    Allow,
    #[serde(rename = "deny")]
    Deny,
    #[serde(rename = "ask")]
    Ask,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PermissionRule {
    pub permission: String,
    pub pattern: String,
    pub action: PermissionAction,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PermissionRuleset {
    pub rules: Vec<PermissionRule>,
}

impl PermissionRuleset {
    pub fn default_with_all_allow() -> Self {
        Self {
            rules: vec![
                PermissionRule {
                    permission: "*".to_string(),
                    pattern: "*".to_string(),
                    action: PermissionAction::Allow,
                },
            ],
        }
    }

    pub fn merge(&self, other: &PermissionRuleset) -> PermissionRuleset {
        // Merge rulesets, with other taking precedence
        let mut rules = self.rules.clone();
        rules.extend(other.rules.clone());
        PermissionRuleset { rules }
    }
}

impl Default for PermissionRuleset {
    fn default() -> Self {
        Self::default_with_all_allow()
    }
}

pub struct PermissionChecker {
    rulesets: Arc<RwLock<HashMap<String, PermissionRuleset>>>,
}

impl PermissionChecker {
    pub fn new() -> Self {
        Self {
            rulesets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn set_ruleset(&self, session_id: String, ruleset: PermissionRuleset) {
        let mut rulesets = self.rulesets.write().await;
        rulesets.insert(session_id, ruleset);
    }

    pub async fn check(
        &self,
        session_id: &str,
        permission: &str,
        pattern: &str,
    ) -> Result<bool, PermissionError> {
        let rulesets = self.rulesets.read().await;

        if let Some(ruleset) = rulesets.get(session_id) {
            for rule in &ruleset.rules {
                if self.matches(&rule.permission, permission) && self.matches(&rule.pattern, pattern) {
                    return match rule.action {
                        PermissionAction::Allow => Ok(true),
                        PermissionAction::Deny => Err(PermissionError::Denied {
                            permission: permission.to_string(),
                            pattern: pattern.to_string(),
                        }),
                        PermissionAction::Ask => Err(PermissionError::Ask {
                            permission: permission.to_string(),
                            pattern: pattern.to_string(),
                        }),
                    };
                }
            }
        }

        // Default to allow if no rules match
        Ok(true)
    }

    fn matches(&self, pattern: &str, value: &str) -> bool {
        // Simple glob pattern matching
        if pattern == "*" {
            return true;
        }

        // Handle wildcards
        if pattern.contains('*') {
            let parts: Vec<&str> = pattern.split('*').collect();
            if parts.len() == 2 {
                return value.starts_with(parts[0]) && value.ends_with(parts[1]);
            }
        }

        pattern == value
    }

    pub async fn ask_permission(
        &self,
        session_id: &str,
        permission: &str,
        pattern: &str,
    ) -> Result<bool, PermissionError> {
        // In a real implementation, this would prompt the user
        // For now, we'll just return the check result
        self.check(session_id, permission, pattern).await
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    #[error("Permission denied: {permission} for pattern {pattern}")]
    Denied { permission: String, pattern: String },

    #[error("Permission required: {permission} for pattern {pattern}")]
    Ask { permission: String, pattern: String },
}
```

## Command Integration

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CommandConfig {
    pub name: String,
    pub agent: String,
    pub description: Option<String>,
    pub subtask: Option<bool>,
    pub command: String,
}

pub struct CommandExecutor {
    registry: Arc<AgentRegistry>,
    task_tool: Arc<TaskTool>,
}

impl CommandExecutor {
    pub async fn execute_command(
        &self,
        config: CommandConfig,
        args: Vec<String>,
        ctx: CommandContext,
    ) -> Result<ExecutionResult, CommandError> {
        // Get agent configuration
        let agent = self.registry.get(&config.agent)
            .await
            .ok_or_else(|| CommandError::AgentNotFound(config.agent.clone()))?;

        // Check if this should be executed as a subtask
        let is_subtask = (agent.mode == AgentMode::Subagent && config.subtask != Some(false))
            || config.subtask == Some(true);

        if is_subtask {
            // Execute as Task tool
            let params = TaskParams {
                description: config.description.unwrap_or_else(|| config.name.clone()),
                prompt: format!("{} {}", config.command, args.join(" ")),
                subagent_type: config.agent.clone(),
                session_id: None,
                command: Some(config.name.clone()),
            };

            let tool_ctx = ToolContext {
                session_id: ctx.session_id.clone(),
                project_id: ctx.project_id.clone(),
                message_id: ctx.message_id.clone(),
            };

            self.task_tool.execute(params, tool_ctx).await
                .map_err(|e| CommandError::TaskError(e.to_string()))
                .map(|r| ExecutionResult::Subtask(r))
        } else {
            // Execute as regular prompt
            self.execute_as_prompt(config, args, ctx).await
        }
    }

    async fn execute_as_prompt(
        &self,
        config: CommandConfig,
        args: Vec<String>,
        ctx: CommandContext,
    ) -> Result<ExecutionResult, CommandError> {
        // Implementation for regular command execution
        Ok(ExecutionResult::Prompt("Command executed".to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct CommandContext {
    pub session_id: String,
    pub project_id: String,
    pub message_id: String,
}

#[derive(Debug)]
pub enum ExecutionResult {
    Subtask(TaskResult),
    Prompt(String),
}

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    #[error("Task error: {0}")]
    TaskError(String),
}
```

## Main Application Example

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize components
    let registry = Arc::new(AgentRegistry::new().await);
    let session_manager = Arc::new(SessionManager::new());
    let permission_checker = Arc::new(PermissionChecker::new());
    let event_bus = Arc::new(EventBus::new());

    // Create task tool
    let task_tool = Arc::new(TaskTool::new(
        registry.clone(),
        session_manager.clone(),
        permission_checker.clone(),
    ));

    // Create a parent session
    let parent_session = session_manager.create(CreateSessionParams {
        parent_id: None,
        project_id: "my-project".to_string(),
        title: "Main session".to_string(),
        permission: None,
    }).await?;

    println!("Created parent session: {}", parent_session.id);

    // Create tool context
    let tool_ctx = ToolContext {
        session_id: parent_session.id.clone(),
        project_id: "my-project".to_string(),
        message_id: "msg-001".to_string(),
    };

    // Execute a task using the explore subagent
    let task_params = TaskParams {
        description: "Explore codebase structure".to_string(),
        prompt: "Find all API endpoints in the src/api directory and describe their functionality".to_string(),
        subagent_type: "explore".to_string(),
        session_id: None,
        command: None,
    };

    let result = task_tool.execute(task_params, tool_ctx).await?;
    println!("Task completed: {}", result.title);
    println!("Output: {}", result.output);

    // List all subagents
    let subagents = registry.list_subagents().await;
    println!("Available subagents:");
    for agent in subagents {
        println!("  - {}: {}", agent.name, agent.description.unwrap_or_default());
    }

    // Navigate sessions
    let children = session_manager.get_children(&parent_session.id).await;
    println!("Child sessions: {}", children.len());

    Ok(())
}
```

## Testing Examples

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_agent_registry() {
        let registry = AgentRegistry::new().await;

        let agent = registry.get("explore").await;
        assert!(agent.is_some());
        assert_eq!(agent.unwrap().mode, AgentMode::Subagent);
    }

    #[tokio::test]
    async fn test_session_hierarchy() {
        let manager = SessionManager::new();

        let parent = manager.create(CreateSessionParams {
            parent_id: None,
            project_id: "test".to_string(),
            title: "Parent".to_string(),
            permission: None,
        }).await.unwrap();

        let child = manager.create(CreateSessionParams {
            parent_id: Some(parent.id.clone()),
            project_id: "test".to_string(),
            title: "Child".to_string(),
            permission: None,
        }).await.unwrap();

        let children = manager.get_children(&parent.id).await;
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].id, child.id);

        let retrieved_parent = manager.get_parent(&child.id).await;
        assert_eq!(retrieved_parent.unwrap().id, parent.id);
    }

    #[tokio::test]
    async fn test_permission_checking() {
        let checker = PermissionChecker::new();
        let session_id = "test-session".to_string();

        checker.set_ruleset(session_id.clone(), PermissionRuleset {
            rules: vec![
                PermissionRule {
                    permission: "read".to_string(),
                    pattern: "*.rs".to_string(),
                    action: PermissionAction::Allow,
                },
                PermissionRule {
                    permission: "write".to_string(),
                    pattern: "*".to_string(),
                    action: PermissionAction::Deny,
                },
            ],
        }).await;

        assert!(checker.check(&session_id, "read", "main.rs").await.is_ok());
        assert!(checker.check(&session_id, "write", "main.rs").await.is_err());
    }

    #[tokio::test]
    async fn test_session_navigation() {
        let manager = SessionManager::new();

        let parent = manager.create(CreateSessionParams {
            parent_id: None,
            project_id: "test".to_string(),
            title: "Parent".to_string(),
            permission: None,
        }).await.unwrap();

        let child1 = manager.create(CreateSessionParams {
            parent_id: Some(parent.id.clone()),
            project_id: "test".to_string(),
            title: "Child 1".to_string(),
            permission: None,
        }).await.unwrap();

        let child2 = manager.create(CreateSessionParams {
            parent_id: Some(parent.id.clone()),
            project_id: "test".to_string(),
            title: "Child 2".to_string(),
            permission: None,
        }).await.unwrap();

        // Navigate from child1 to child2
        let next = manager.navigate_next(&child1.id).await;
        assert!(next.is_some());
        assert_eq!(next.unwrap().id, parent.id);

        // Navigate from child2 to parent
        let prev = manager.navigate_previous(&child2.id).await;
        assert!(prev.is_some());
        assert_eq!(prev.unwrap().id, parent.id);
    }
}
```

## Dependencies

Add to `Cargo.toml`:

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
uuid = { version = "1", features = ["v4", "serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
regex = "1"
glob = "0.3"
```

## Key Implementation Notes

1. **Thread Safety**: Use `Arc<RwLock>` for shared state to allow concurrent reads
2. **Async Runtime**: Tokio provides efficient async I/O and task spawning
3. **Event Broadcasting**: Use `tokio::sync::broadcast` for pub/sub pattern
4. **Error Handling**: Use `thiserror` for clean error types
5. **Serialization**: `serde` enables easy JSON/YAML configuration loading
6. **UUID Generation**: Use `uuid` crate for unique session and message IDs
7. **Permission Patterns**: Implement glob-style pattern matching for flexible rules
8. **Session Isolation**: Child sessions should inherit but can modify permission rulesets
