//! Web Search Probe orchestration shell.
//!
//! Checkpoint 1 intentionally keeps this module as a no-op lifecycle hook:
//! it parses config, skips non-`Execute` requests, and returns the original
//! request unchanged. Actual probe executor creation starts in the next
//! checkpoint.

use super::{web_bool_env, web_env_value};
use crate::server::task_executor::TaskRunRequest;
use oxide_agent_web_contracts::AgentEffort as WebAgentEffort;
use tracing::debug;

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
    session_id: &str,
    task_id: &str,
    run_request: TaskRunRequest,
) -> TaskRunRequest {
    let config = SearchProbeConfig::from_env();
    maybe_run_search_probe_with_config(session_id, task_id, run_request, &config).await
}

async fn maybe_run_search_probe_with_config(
    session_id: &str,
    task_id: &str,
    run_request: TaskRunRequest,
    config: &SearchProbeConfig,
) -> TaskRunRequest {
    if !config.enabled {
        return run_request;
    }

    match &run_request {
        TaskRunRequest::Execute { .. } => {
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
                "Search Probe shell enabled; probe execution is not implemented in this checkpoint"
            );
        }
        TaskRunRequest::ResumeUserInput { .. } => {
            debug!(
                session_id = %session_id,
                task_id = %task_id,
                "Search Probe skipped for ResumeUserInput"
            );
        }
    }

    run_request
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
    use oxide_agent_core::agent::AgentUserInput;
    use std::sync::{Mutex, MutexGuard};

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
        let config = SearchProbeConfig {
            enabled: false,
            ..SearchProbeConfig::default()
        };
        let run_request = execute_request("original prompt");

        let result =
            maybe_run_search_probe_with_config("session", "task", run_request, &config).await;

        assert!(matches!(result, TaskRunRequest::Execute { .. }));
        assert_eq!(request_content(&result), "original prompt");
    }

    #[tokio::test]
    async fn enabled_shell_returns_execute_request_unchanged() {
        let config = SearchProbeConfig {
            enabled: true,
            ..SearchProbeConfig::default()
        };
        let run_request = execute_request("original prompt");

        let result =
            maybe_run_search_probe_with_config("session", "task", run_request, &config).await;

        assert!(matches!(result, TaskRunRequest::Execute { .. }));
        assert_eq!(request_content(&result), "original prompt");
    }

    #[tokio::test]
    async fn enabled_shell_skips_resume_request() {
        let config = SearchProbeConfig {
            enabled: true,
            ..SearchProbeConfig::default()
        };
        let run_request = resume_request("resume prompt");

        let result =
            maybe_run_search_probe_with_config("session", "task", run_request, &config).await;

        assert!(matches!(result, TaskRunRequest::ResumeUserInput { .. }));
        assert_eq!(request_content(&result), "resume prompt");
    }
}
