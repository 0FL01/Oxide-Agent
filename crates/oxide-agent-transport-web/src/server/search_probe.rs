//! Web Search Probe orchestration shell.
//!
//! Runs the web-only Search Probe sidecar before the main web executor starts.

use super::{web_bool_env, web_env_value};
use crate::server::task_executor::TaskRunRequest;
use crate::session::{SearchProbeRuntimeOptions, WebSessionManager};
use oxide_agent_core::agent::progress::AgentEventSource;
use oxide_agent_core::agent::{
    AgentEvent, AgentExecutionEffort, AgentExecutionOptions, AgentExecutionOutcome, AgentUserInput,
};
use oxide_agent_web_contracts::AgentEffort as WebAgentEffort;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

const ENV_ENABLED: &str = "OXIDE_SEARCH_PROBE_ENABLED";
const ENV_MAX_GENERATIONS: &str = "OXIDE_SEARCH_PROBE_MAX_GENERATIONS";
const ENV_PER_GENERATION_TIMEOUT_SECS: &str = "OXIDE_SEARCH_PROBE_PER_GENERATION_TIMEOUT_SECS";
const ENV_TOTAL_TIMEOUT_SECS: &str = "OXIDE_SEARCH_PROBE_TOTAL_TIMEOUT_SECS";
const ENV_MIN_EFFORT: &str = "OXIDE_SEARCH_PROBE_MIN_EFFORT";
const ENV_PUBLIC_UPDATES: &str = "OXIDE_SEARCH_PROBE_PUBLIC_UPDATES";
const ENV_FORWARD_TOOL_EVENTS: &str = "OXIDE_SEARCH_PROBE_FORWARD_TOOL_EVENTS";
const ENV_TOOL_ALLOWLIST: &str = "OXIDE_SEARCH_PROBE_TOOL_ALLOWLIST";
const ENV_DOSSIER_MAX_CHARS: &str = "OXIDE_SEARCH_PROBE_DOSSIER_MAX_CHARS";

const DEFAULT_MAX_GENERATIONS: u8 = 2;
const DEFAULT_PER_GENERATION_TIMEOUT_SECS: u64 = 180;
const DEFAULT_TOTAL_TIMEOUT_SECS: u64 = 480;
const DEFAULT_PUBLIC_UPDATES: bool = true;
const DEFAULT_FORWARD_TOOL_EVENTS: bool = true;
const DEFAULT_DOSSIER_MAX_CHARS: usize = 80_000;
const DEFAULT_TOOL_ALLOWLIST: &[&str] = &["searxng_search", "crawl4ai_markdown", "web_markdown"];
const PROBE_PROFILE_PROMPT: &str = "You are Search Probe, a web-only research sidecar. Use only the tools available to you and return compact handoff notes for the main agent.";
const GENERATION_PROMPT_HEADER: &str = r#"You are Search Probe, a web-only research sidecar for the web console.

Use only available web research tools. Do not answer the user directly. Produce a compact handoff for the main agent.

Return this XML-like contract at the end of every generation:

<search_probe_public_update>
Short user-visible TL;DR in the user's language.
</search_probe_public_update>

<search_probe_handoff>
Dense facts, source notes, uncertainties, and next-search hints for the main agent.
</search_probe_handoff>

<search_probe_decision>
continue or stop
</search_probe_decision>
"#;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchProbeHandoff {
    generation: u8,
    public_update: Option<String>,
    handoff: String,
    decision: SearchProbeDecision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchProbeDecision {
    Continue,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct SearchProbeRunOutcome {
    pub(crate) handoffs: Vec<SearchProbeHandoff>,
    pub(crate) cancelled: bool,
}

struct SearchProbeRunCtx<'a> {
    session_manager: &'a WebSessionManager,
    session_id: &'a str,
    task_id: &'a str,
    original_input: &'a AgentUserInput,
    parent_effort: Option<WebAgentEffort>,
    config: &'a SearchProbeConfig,
    progress_tx: mpsc::Sender<AgentEvent>,
    cancellation_token: CancellationToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchProbeConfig {
    pub(crate) enabled: bool,
    pub(crate) max_generations: u8,
    pub(crate) per_generation_timeout_secs: u64,
    pub(crate) total_timeout_secs: u64,
    pub(crate) min_effort: WebAgentEffort,
    pub(crate) public_updates: bool,
    pub(crate) forward_tool_events: bool,
    pub(crate) tool_allowlist: Vec<String>,
    pub(crate) dossier_max_chars: usize,
}

impl SearchProbeConfig {
    #[must_use]
    pub(crate) fn from_env() -> Self {
        Self {
            enabled: web_bool_env(ENV_ENABLED),
            max_generations: env_u8(ENV_MAX_GENERATIONS, DEFAULT_MAX_GENERATIONS).clamp(1, 3),
            per_generation_timeout_secs: env_u64(
                ENV_PER_GENERATION_TIMEOUT_SECS,
                DEFAULT_PER_GENERATION_TIMEOUT_SECS,
            ),
            total_timeout_secs: env_u64(ENV_TOTAL_TIMEOUT_SECS, DEFAULT_TOTAL_TIMEOUT_SECS),
            min_effort: env_effort(ENV_MIN_EFFORT, WebAgentEffort::Heavy),
            public_updates: env_bool_default(ENV_PUBLIC_UPDATES, DEFAULT_PUBLIC_UPDATES),
            forward_tool_events: env_bool_default(
                ENV_FORWARD_TOOL_EVENTS,
                DEFAULT_FORWARD_TOOL_EVENTS,
            ),
            tool_allowlist: env_tool_allowlist(ENV_TOOL_ALLOWLIST),
            dossier_max_chars: env_usize(ENV_DOSSIER_MAX_CHARS, DEFAULT_DOSSIER_MAX_CHARS),
        }
    }
}

impl Default for SearchProbeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_generations: DEFAULT_MAX_GENERATIONS,
            per_generation_timeout_secs: DEFAULT_PER_GENERATION_TIMEOUT_SECS,
            total_timeout_secs: DEFAULT_TOTAL_TIMEOUT_SECS,
            min_effort: WebAgentEffort::Heavy,
            public_updates: DEFAULT_PUBLIC_UPDATES,
            forward_tool_events: DEFAULT_FORWARD_TOOL_EVENTS,
            tool_allowlist: DEFAULT_TOOL_ALLOWLIST
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect(),
            dossier_max_chars: DEFAULT_DOSSIER_MAX_CHARS,
        }
    }
}

pub(crate) async fn maybe_run_search_probe(
    session_manager: &WebSessionManager,
    session_id: &str,
    task_id: &str,
    run_request: TaskRunRequest,
    progress_tx: mpsc::Sender<AgentEvent>,
    cancellation_token: CancellationToken,
) -> (TaskRunRequest, SearchProbeRunOutcome) {
    let config = SearchProbeConfig::from_env();
    maybe_run_search_probe_with_runtime(
        session_manager,
        session_id,
        task_id,
        run_request,
        &config,
        progress_tx,
        cancellation_token,
    )
    .await
}

#[cfg(test)]
async fn maybe_run_search_probe_with_config(
    session_manager: &WebSessionManager,
    session_id: &str,
    task_id: &str,
    run_request: TaskRunRequest,
    config: &SearchProbeConfig,
) -> (TaskRunRequest, SearchProbeRunOutcome) {
    let (progress_tx, cancellation_token) = (mpsc::channel(1).0, CancellationToken::new());
    maybe_run_search_probe_with_runtime(
        session_manager,
        session_id,
        task_id,
        run_request,
        config,
        progress_tx,
        cancellation_token,
    )
    .await
}

async fn maybe_run_search_probe_with_runtime(
    session_manager: &WebSessionManager,
    session_id: &str,
    task_id: &str,
    run_request: TaskRunRequest,
    config: &SearchProbeConfig,
    progress_tx: mpsc::Sender<AgentEvent>,
    cancellation_token: CancellationToken,
) -> (TaskRunRequest, SearchProbeRunOutcome) {
    if !config.enabled {
        return (run_request, SearchProbeRunOutcome::default());
    }

    match &run_request {
        TaskRunRequest::Execute { input, effort } => {
            debug!(
                session_id = %session_id,
                task_id = %task_id,
                max_generations = config.max_generations,
                per_generation_timeout_secs = config.per_generation_timeout_secs,
                total_timeout_secs = config.total_timeout_secs,
                ?config.min_effort,
                public_updates = config.public_updates,
                forward_tool_events = config.forward_tool_events,
                tool_allowlist = ?config.tool_allowlist,
                dossier_max_chars = config.dossier_max_chars,
                "Search Probe enabled"
            );
            let outcome = run_search_probe_generations(SearchProbeRunCtx {
                session_manager,
                session_id,
                task_id,
                original_input: input,
                parent_effort: *effort,
                config,
                progress_tx,
                cancellation_token,
            })
            .await;
            return (run_request, outcome);
        }
        TaskRunRequest::ResumeUserInput { .. } => {
            debug!(
                session_id = %session_id,
                task_id = %task_id,
                "Search Probe skipped for ResumeUserInput"
            );
        }
    }

    (run_request, SearchProbeRunOutcome::default())
}

async fn run_search_probe_generations(ctx: SearchProbeRunCtx<'_>) -> SearchProbeRunOutcome {
    let SearchProbeRunCtx {
        session_manager,
        session_id,
        task_id,
        original_input,
        parent_effort,
        config,
        progress_tx,
        cancellation_token,
    } = ctx;

    if cancellation_token.is_cancelled() {
        send_cancelled(&progress_tx).await;
        return SearchProbeRunOutcome {
            cancelled: true,
            ..SearchProbeRunOutcome::default()
        };
    }

    send_milestone(&progress_tx, "search_probe_started").await;
    let started_at = tokio::time::Instant::now();
    let total_timeout = std::time::Duration::from_secs(config.total_timeout_secs.max(1));
    let mut handoffs = Vec::new();

    for generation in 1..=config.max_generations {
        if cancellation_token.is_cancelled() {
            send_cancelled(&progress_tx).await;
            return SearchProbeRunOutcome {
                handoffs,
                cancelled: true,
            };
        }

        let Some(generation_timeout) = next_generation_timeout(
            started_at,
            total_timeout,
            std::time::Duration::from_secs(config.per_generation_timeout_secs.max(1)),
        ) else {
            warn!(session_id = %session_id, task_id = %task_id, "Search Probe total timeout reached");
            break;
        };

        send_milestone(
            &progress_tx,
            &format!("search_probe_generation_{generation}_started"),
        )
        .await;

        let Some(mut executor) = session_manager
            .create_search_probe_executor(
                session_id,
                SearchProbeRuntimeOptions {
                    tool_allowlist: config.tool_allowlist.clone(),
                    prompt_instructions: Some(PROBE_PROFILE_PROMPT.to_string()),
                },
            )
            .await
        else {
            warn!(session_id = %session_id, task_id = %task_id, "Search Probe parent session not found");
            break;
        };
        executor.session_mut().cancellation_token = cancellation_token.child_token();

        let prompt =
            build_generation_prompt(original_input.text_projection(), &handoffs, generation);
        let probe_tx = spawn_probe_event_relay(progress_tx.clone(), config.forward_tool_events);
        let execution = executor.execute_user_input_with_options(
            AgentUserInput::new(prompt),
            Some(probe_tx),
            probe_execution_options(parent_effort, config.min_effort),
        );

        let result = tokio::select! {
            () = cancellation_token.cancelled() => {
                send_cancelled(&progress_tx).await;
                return SearchProbeRunOutcome { handoffs, cancelled: true };
            }
            result = tokio::time::timeout(generation_timeout, execution) => result,
        };

        let text = match result {
            Ok(Ok(AgentExecutionOutcome::Completed(text))) => text,
            Ok(Ok(AgentExecutionOutcome::WaitingForUserInput(_))) => {
                warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe unexpectedly requested user input");
                break;
            }
            Ok(Err(error)) => {
                warn!(session_id = %session_id, task_id = %task_id, generation, error = %error, "Search Probe generation failed");
                break;
            }
            Err(_) => {
                executor.session_mut().cancellation_token.cancel();
                warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe generation timed out");
                break;
            }
        };

        let handoff = parse_search_probe_contract(generation, &text);
        if config.public_updates
            && let Some(update) = handoff.public_update.as_deref()
        {
            send_reasoning(&progress_tx, update).await;
        }
        send_milestone(
            &progress_tx,
            &format!("search_probe_generation_{generation}_completed"),
        )
        .await;
        let should_stop = handoff.decision == SearchProbeDecision::Stop;
        handoffs.push(handoff);
        if should_stop {
            break;
        }
    }

    send_milestone(&progress_tx, "search_probe_completed").await;
    SearchProbeRunOutcome {
        handoffs,
        cancelled: false,
    }
}

fn next_generation_timeout(
    started_at: tokio::time::Instant,
    total_timeout: std::time::Duration,
    per_generation_timeout: std::time::Duration,
) -> Option<std::time::Duration> {
    let elapsed = started_at.elapsed();
    (elapsed < total_timeout).then(|| {
        total_timeout
            .saturating_sub(elapsed)
            .min(per_generation_timeout)
    })
}

fn build_generation_prompt(
    original_prompt: &str,
    prior_handoffs: &[SearchProbeHandoff],
    generation: u8,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(GENERATION_PROMPT_HEADER);
    prompt.push_str("\n<original_user_prompt>\n");
    prompt.push_str(original_prompt);
    prompt.push_str("\n</original_user_prompt>\n");
    prompt.push_str("\n<search_probe_generation>\n");
    prompt.push_str(&generation.to_string());
    prompt.push_str("\n</search_probe_generation>\n");
    if !prior_handoffs.is_empty() {
        prompt.push_str("\n<previous_search_probe_handoffs>\n");
        for handoff in prior_handoffs {
            prompt.push_str("\n<generation index=\"");
            prompt.push_str(&handoff.generation.to_string());
            prompt.push_str("\">\n");
            prompt.push_str(&handoff.handoff);
            prompt.push_str("\n</generation>\n");
        }
        prompt.push_str("</previous_search_probe_handoffs>\n");
    }
    prompt
}

fn parse_search_probe_contract(generation: u8, text: &str) -> SearchProbeHandoff {
    let public_update = extract_tag(text, "search_probe_public_update");
    let handoff = extract_tag(text, "search_probe_handoff")
        .or_else(|| public_update.clone())
        .unwrap_or_else(|| text.trim().to_owned());
    let decision = match extract_tag(text, "search_probe_decision")
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "stop" => SearchProbeDecision::Stop,
        _ => SearchProbeDecision::Continue,
    };

    SearchProbeHandoff {
        generation,
        public_update,
        handoff,
        decision,
    }
}

fn extract_tag(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn probe_execution_options(
    parent_effort: Option<WebAgentEffort>,
    min_effort: WebAgentEffort,
) -> AgentExecutionOptions {
    AgentExecutionOptions::with_effort(web_effort_to_core(max_effort(
        parent_effort.unwrap_or(WebAgentEffort::Standard),
        min_effort,
    )))
}

const fn max_effort(left: WebAgentEffort, right: WebAgentEffort) -> WebAgentEffort {
    if effort_rank(left) >= effort_rank(right) {
        left
    } else {
        right
    }
}

const fn effort_rank(effort: WebAgentEffort) -> u8 {
    match effort {
        WebAgentEffort::Standard => 0,
        WebAgentEffort::Extended => 1,
        WebAgentEffort::Heavy => 2,
    }
}

const fn web_effort_to_core(effort: WebAgentEffort) -> AgentExecutionEffort {
    match effort {
        WebAgentEffort::Standard => AgentExecutionEffort::Standard,
        WebAgentEffort::Extended => AgentExecutionEffort::Extended,
        WebAgentEffort::Heavy => AgentExecutionEffort::Heavy,
    }
}

fn spawn_probe_event_relay(
    parent_tx: mpsc::Sender<AgentEvent>,
    forward_tool_events: bool,
) -> mpsc::Sender<AgentEvent> {
    let (tx, mut rx) = mpsc::channel(100);
    tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            if let Some(event) = relayable_probe_event(event, forward_tool_events) {
                let _ = parent_tx.send(event).await;
            }
        }
    });
    tx
}

fn relayable_probe_event(event: AgentEvent, forward_tool_events: bool) -> Option<AgentEvent> {
    if !forward_tool_events {
        return None;
    }
    match event {
        AgentEvent::ToolCall {
            id,
            source,
            name,
            input,
            command_preview,
        } => Some(AgentEvent::ToolCall {
            id,
            source,
            name,
            input,
            command_preview,
        }),
        AgentEvent::ToolResult {
            id,
            source,
            name,
            output,
            success,
        } => Some(AgentEvent::ToolResult {
            id,
            source,
            name,
            output,
            success,
        }),
        _ => None,
    }
}

async fn send_milestone(progress_tx: &mpsc::Sender<AgentEvent>, name: &str) {
    let _ = progress_tx
        .send(AgentEvent::Milestone {
            name: name.to_owned(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
        })
        .await;
}

async fn send_reasoning(progress_tx: &mpsc::Sender<AgentEvent>, summary: &str) {
    let _ = progress_tx
        .send(AgentEvent::Reasoning {
            source: AgentEventSource::Root,
            summary: format!("Search Probe: {summary}"),
        })
        .await;
}

async fn send_cancelled(progress_tx: &mpsc::Sender<AgentEvent>) {
    let _ = progress_tx.send(AgentEvent::Cancelled).await;
}

fn env_bool_default(key: &str, default: bool) -> bool {
    web_env_value(key)
        .map(|value| super::parse_web_bool(&value))
        .unwrap_or(default)
}

fn env_u8(key: &str, default: u8) -> u8 {
    web_env_value(key)
        .and_then(|value| value.trim().parse::<u8>().ok())
        .unwrap_or(default)
}

fn env_u64(key: &str, default: u64) -> u64 {
    web_env_value(key)
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn env_usize(key: &str, default: usize) -> usize {
    web_env_value(key)
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

fn env_effort(key: &str, default: WebAgentEffort) -> WebAgentEffort {
    web_env_value(key)
        .and_then(|value| parse_effort(&value))
        .unwrap_or(default)
}

fn parse_effort(value: &str) -> Option<WebAgentEffort> {
    match value.trim().to_ascii_lowercase().as_str() {
        "standard" => Some(WebAgentEffort::Standard),
        "extended" => Some(WebAgentEffort::Extended),
        "heavy" => Some(WebAgentEffort::Heavy),
        _ => None,
    }
}

fn env_tool_allowlist(key: &str) -> Vec<String> {
    web_env_value(key)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|tool| !tool.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|tools| !tools.is_empty())
        .unwrap_or_else(|| {
            DEFAULT_TOOL_ALLOWLIST
                .iter()
                .map(|tool| (*tool).to_owned())
                .collect()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_memory_storage::InMemoryStorage;
    use oxide_agent_core::config::AgentSettings;
    use oxide_agent_core::llm::LlmClient;
    use oxide_agent_runtime::SessionRegistry;
    use std::sync::Arc;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_KEYS: &[&str] = &[
        ENV_ENABLED,
        ENV_MAX_GENERATIONS,
        ENV_PER_GENERATION_TIMEOUT_SECS,
        ENV_TOTAL_TIMEOUT_SECS,
        ENV_MIN_EFFORT,
        ENV_PUBLIC_UPDATES,
        ENV_FORWARD_TOOL_EVENTS,
        ENV_TOOL_ALLOWLIST,
        ENV_DOSSIER_MAX_CHARS,
    ];

    struct EnvGuard(MutexGuard<'static, ()>);

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            let _guard = &self.0;
            for key in ENV_KEYS {
                test_remove_env(key);
            }
        }
    }

    fn lock_env() -> EnvGuard {
        let guard = ENV_LOCK.lock().expect("search probe env lock poisoned");
        for key in ENV_KEYS {
            test_remove_env(key);
        }
        EnvGuard(guard)
    }

    #[track_caller]
    fn test_set_env(key: impl AsRef<std::ffi::OsStr>, value: impl AsRef<std::ffi::OsStr>) {
        unsafe { std::env::set_var(key, value) };
    }

    #[track_caller]
    fn test_remove_env(key: impl AsRef<std::ffi::OsStr>) {
        unsafe { std::env::remove_var(key) };
    }

    fn execute_request(content: &str) -> TaskRunRequest {
        TaskRunRequest::Execute {
            input: AgentUserInput::new(content),
            effort: None,
        }
    }

    fn resume_request(content: &str) -> TaskRunRequest {
        TaskRunRequest::ResumeUserInput {
            input: AgentUserInput::new(content),
            effort: None,
        }
    }

    fn test_session_manager() -> WebSessionManager {
        let settings = Arc::new(AgentSettings::default());
        let llm = Arc::new(LlmClient::new(settings.as_ref()));
        WebSessionManager::new_with_storage(
            SessionRegistry::new(),
            llm,
            settings,
            Arc::new(InMemoryStorage::new()),
        )
    }

    async fn create_parent_session(session_manager: &WebSessionManager) {
        session_manager
            .create_session_with_id(
                1,
                "session".to_string(),
                "web-session-search-probe-test".to_string(),
                "main".to_string(),
            )
            .await;
    }

    fn request_content(run_request: &TaskRunRequest) -> &str {
        match run_request {
            TaskRunRequest::Execute { input, .. }
            | TaskRunRequest::ResumeUserInput { input, .. } => &input.content,
        }
    }

    #[test]
    fn config_defaults_are_disabled_and_web_only() {
        let _guard = lock_env();

        let config = SearchProbeConfig::from_env();

        assert!(!config.enabled);
        assert_eq!(config.max_generations, 2);
        assert_eq!(config.per_generation_timeout_secs, 180);
        assert_eq!(config.total_timeout_secs, 480);
        assert_eq!(config.min_effort, WebAgentEffort::Heavy);
        assert!(config.public_updates);
        assert!(config.forward_tool_events);
        assert_eq!(
            config.tool_allowlist,
            vec!["searxng_search", "crawl4ai_markdown", "web_markdown"]
        );
        assert_eq!(config.dossier_max_chars, 80_000);
    }

    #[test]
    fn config_clamps_generation_count_and_parses_env() {
        let _guard = lock_env();
        test_set_env(ENV_ENABLED, "true");
        test_set_env(ENV_MAX_GENERATIONS, "9");
        test_set_env(ENV_PER_GENERATION_TIMEOUT_SECS, "11");
        test_set_env(ENV_TOTAL_TIMEOUT_SECS, "22");
        test_set_env(ENV_MIN_EFFORT, "extended");
        test_set_env(ENV_PUBLIC_UPDATES, "false");
        test_set_env(ENV_FORWARD_TOOL_EVENTS, "false");
        test_set_env(
            ENV_TOOL_ALLOWLIST,
            " searxng_search, crawl4ai_markdown ,, web_markdown ",
        );
        test_set_env(ENV_DOSSIER_MAX_CHARS, "12345");

        let config = SearchProbeConfig::from_env();

        assert!(config.enabled);
        assert_eq!(config.max_generations, 3);
        assert_eq!(config.per_generation_timeout_secs, 11);
        assert_eq!(config.total_timeout_secs, 22);
        assert_eq!(config.min_effort, WebAgentEffort::Extended);
        assert!(!config.public_updates);
        assert!(!config.forward_tool_events);
        assert_eq!(
            config.tool_allowlist,
            vec!["searxng_search", "crawl4ai_markdown", "web_markdown"]
        );
        assert_eq!(config.dossier_max_chars, 12_345);
    }

    #[tokio::test]
    async fn disabled_shell_returns_execute_request_unchanged() {
        let session_manager = test_session_manager();
        let config = SearchProbeConfig {
            enabled: false,
            ..SearchProbeConfig::default()
        };
        let run_request = execute_request("original prompt");

        let (result, outcome) = maybe_run_search_probe_with_config(
            &session_manager,
            "session",
            "task",
            run_request,
            &config,
        )
        .await;

        assert!(matches!(result, TaskRunRequest::Execute { .. }));
        assert_eq!(request_content(&result), "original prompt");
        assert!(outcome.handoffs.is_empty());
        assert!(!outcome.cancelled);
    }

    #[tokio::test]
    async fn enabled_shell_returns_execute_request_unchanged() {
        let session_manager = test_session_manager();
        create_parent_session(&session_manager).await;
        let config = SearchProbeConfig {
            enabled: true,
            ..SearchProbeConfig::default()
        };
        let run_request = execute_request("original prompt");

        let (result, _outcome) = maybe_run_search_probe_with_config(
            &session_manager,
            "session",
            "task",
            run_request,
            &config,
        )
        .await;

        assert!(matches!(result, TaskRunRequest::Execute { .. }));
        assert_eq!(request_content(&result), "original prompt");
    }

    #[tokio::test]
    async fn enabled_shell_skips_resume_request() {
        let session_manager = test_session_manager();
        let config = SearchProbeConfig {
            enabled: true,
            ..SearchProbeConfig::default()
        };
        let run_request = resume_request("resume prompt");

        let (result, outcome) = maybe_run_search_probe_with_config(
            &session_manager,
            "session",
            "task",
            run_request,
            &config,
        )
        .await;

        assert!(matches!(result, TaskRunRequest::ResumeUserInput { .. }));
        assert_eq!(request_content(&result), "resume prompt");
        assert!(outcome.handoffs.is_empty());
        assert!(!outcome.cancelled);
    }

    #[test]
    fn parser_extracts_contract_fields() {
        let parsed = parse_search_probe_contract(
            2,
            r#"
before
<search_probe_public_update>
TL;DR: found source shape.
</search_probe_public_update>
<search_probe_handoff>
Use source A and compare latency numbers.
</search_probe_handoff>
<search_probe_decision>
stop
</search_probe_decision>
after
"#,
        );

        assert_eq!(parsed.generation, 2);
        assert_eq!(
            parsed.public_update.as_deref(),
            Some("TL;DR: found source shape.")
        );
        assert_eq!(parsed.handoff, "Use source A and compare latency numbers.");
        assert_eq!(parsed.decision, SearchProbeDecision::Stop);
    }

    #[test]
    fn parser_falls_back_to_raw_text_without_contract() {
        let parsed = parse_search_probe_contract(1, "plain handoff without tags");

        assert_eq!(parsed.public_update, None);
        assert_eq!(parsed.handoff, "plain handoff without tags");
        assert_eq!(parsed.decision, SearchProbeDecision::Continue);
    }

    #[test]
    fn probe_effort_uses_configured_minimum() {
        assert_eq!(
            probe_execution_options(Some(WebAgentEffort::Standard), WebAgentEffort::Heavy).effort,
            AgentExecutionEffort::Heavy
        );
        assert_eq!(
            probe_execution_options(Some(WebAgentEffort::Heavy), WebAgentEffort::Extended).effort,
            AgentExecutionEffort::Heavy
        );
    }

    #[tokio::test]
    async fn public_update_uses_reasoning_event() {
        let (tx, mut rx) = mpsc::channel(2);

        send_reasoning(&tx, "TL;DR: checked the docs.").await;
        drop(tx);

        let event = rx.recv().await.expect("reasoning event");
        match event {
            AgentEvent::Reasoning { source, summary } => {
                assert_eq!(source, AgentEventSource::Root);
                assert!(summary.contains("Search Probe: TL;DR: checked the docs."));
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancelled_probe_does_not_start_generations() {
        let session_manager = test_session_manager();
        let config = SearchProbeConfig {
            enabled: true,
            ..SearchProbeConfig::default()
        };
        let token = CancellationToken::new();
        token.cancel();
        let (tx, mut rx) = mpsc::channel(4);

        let (_result, outcome) = maybe_run_search_probe_with_runtime(
            &session_manager,
            "session",
            "task",
            execute_request("original"),
            &config,
            tx,
            token,
        )
        .await;

        assert!(outcome.cancelled);
        assert!(outcome.handoffs.is_empty());
        assert!(matches!(rx.recv().await, Some(AgentEvent::Cancelled)));
    }

    #[test]
    fn generation_timeout_uses_total_remaining_budget() {
        let now = tokio::time::Instant::now();
        let timeout = next_generation_timeout(now, Duration::from_secs(10), Duration::from_secs(3))
            .expect("timeout");
        assert!(timeout <= Duration::from_secs(3));
        assert!(timeout > Duration::ZERO);

        let almost_expired = tokio::time::Instant::now()
            .checked_sub(Duration::from_secs(9))
            .expect("valid instant");
        let timeout = next_generation_timeout(
            almost_expired,
            Duration::from_secs(10),
            Duration::from_secs(5),
        )
        .expect("timeout");
        assert!(timeout <= Duration::from_secs(1));

        let expired = tokio::time::Instant::now()
            .checked_sub(Duration::from_secs(11))
            .expect("valid instant");
        assert_eq!(
            next_generation_timeout(expired, Duration::from_secs(10), Duration::from_secs(5)),
            None
        );
    }
}
