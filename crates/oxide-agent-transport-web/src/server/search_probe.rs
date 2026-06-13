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
const ENV_SOFT_FINALIZE_SECS: &str = "OXIDE_SEARCH_PROBE_SOFT_FINALIZE_SECS";
const ENV_FORCED_FINALIZE_TIMEOUT_SECS: &str = "OXIDE_SEARCH_PROBE_FORCED_FINALIZE_TIMEOUT_SECS";
const ENV_FORCED_FINALIZE_EFFORT: &str = "OXIDE_SEARCH_PROBE_FORCED_FINALIZE_EFFORT";
const ENV_SEARCH_LIMIT: &str = "OXIDE_SEARCH_PROBE_SEARCH_LIMIT";
const ENV_MIN_EFFORT: &str = "OXIDE_SEARCH_PROBE_MIN_EFFORT";
const ENV_PUBLIC_UPDATES: &str = "OXIDE_SEARCH_PROBE_PUBLIC_UPDATES";
const ENV_FORWARD_TOOL_EVENTS: &str = "OXIDE_SEARCH_PROBE_FORWARD_TOOL_EVENTS";
const ENV_TOOL_ALLOWLIST: &str = "OXIDE_SEARCH_PROBE_TOOL_ALLOWLIST";
const ENV_DOSSIER_MAX_CHARS: &str = "OXIDE_SEARCH_PROBE_DOSSIER_MAX_CHARS";

const DEFAULT_MAX_GENERATIONS: u8 = 2;
const DEFAULT_PER_GENERATION_TIMEOUT_SECS: u64 = 60;
const DEFAULT_TOTAL_TIMEOUT_SECS: u64 = 60;
const DEFAULT_SOFT_FINALIZE_SECS: u64 = 35;
const DEFAULT_FORCED_FINALIZE_TIMEOUT_SECS: u64 = 20;
const DEFAULT_SEARCH_LIMIT: usize = 3;
const DEFAULT_PUBLIC_UPDATES: bool = true;
const DEFAULT_FORWARD_TOOL_EVENTS: bool = true;
const DEFAULT_DOSSIER_MAX_CHARS: usize = 80_000;
const PREVIOUS_FINAL_MESSAGE_MAX_CHARS: usize = 12_000;
const DEFAULT_SPLIT_TOOL_ALLOWLIST: &[&str] = &["searxng_search", "web_markdown"];
const DEFAULT_MERGED_TOOL_ALLOWLIST: &[&str] = &["searxng_search", "web_crawler"];
const SEARCH_PROBE_BLOCKED_TOOL_CRAWL4AI: &str = "crawl4ai_markdown";
const TIMEOUT_REPORT_MAX_ITEMS: usize = 6;
const TIMEOUT_REPORT_SNIPPET_CHARS: usize = 220;
const PROBE_PROFILE_PROMPT: &str = "You are Search Probe, a web-only research sidecar. Use only the tools available to you and return compact handoff notes for the main agent.";
const GENERATION_PROMPT_HEADER: &str = r#"You are Search Probe, a web-only research sidecar for the web console.

Use only available web research tools. Do not answer the user directly. Produce a compact handoff for the main agent.

Do at most 2-3 web tool calls by default, then synthesize. Prefer one broad search plus one targeted fetch over many narrow searches.

You have a strict time and search budget. If tool calls are blocked, the search budget is exceeded, or the runtime reports a timeout, stop searching immediately and synthesize the best possible XML-like handoff from the evidence already available.

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
const FORCED_FINALIZE_PROMPT_HEADER: &str = r#"You are Search Probe in forced finalize mode.

You have no web tools now. Do not attempt more research. Convert the partial Search Probe leads into a compact handoff for the main agent.

Return this XML-like contract:

<search_probe_public_update>
Short user-visible TL;DR in the user's language.
</search_probe_public_update>

<search_probe_handoff>
Dense facts, source notes, uncertainties, failed/dead-end sources, and exact next-search hints for the main agent.
</search_probe_handoff>

<search_probe_decision>
stop
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

struct SearchProbeFinalizeCtx<'a> {
    session_manager: &'a WebSessionManager,
    session_id: &'a str,
    task_id: &'a str,
    original_prompt: &'a str,
    previous_final_message: Option<&'a str>,
    generation: u8,
    partial_handoff: &'a SearchProbeHandoff,
    config: &'a SearchProbeConfig,
    progress_tx: &'a mpsc::Sender<AgentEvent>,
    cancellation_token: &'a CancellationToken,
    started_at: tokio::time::Instant,
    total_timeout: std::time::Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchProbeConfig {
    pub(crate) enabled: bool,
    pub(crate) max_generations: u8,
    pub(crate) per_generation_timeout_secs: u64,
    pub(crate) total_timeout_secs: u64,
    pub(crate) soft_finalize_secs: u64,
    pub(crate) forced_finalize_timeout_secs: u64,
    pub(crate) forced_finalize_effort: WebAgentEffort,
    pub(crate) search_limit: usize,
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
            soft_finalize_secs: env_u64(ENV_SOFT_FINALIZE_SECS, DEFAULT_SOFT_FINALIZE_SECS),
            forced_finalize_timeout_secs: env_u64(
                ENV_FORCED_FINALIZE_TIMEOUT_SECS,
                DEFAULT_FORCED_FINALIZE_TIMEOUT_SECS,
            ),
            forced_finalize_effort: env_effort(
                ENV_FORCED_FINALIZE_EFFORT,
                WebAgentEffort::Standard,
            ),
            search_limit: env_usize(ENV_SEARCH_LIMIT, DEFAULT_SEARCH_LIMIT),
            min_effort: env_effort(ENV_MIN_EFFORT, WebAgentEffort::Standard),
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
            soft_finalize_secs: DEFAULT_SOFT_FINALIZE_SECS,
            forced_finalize_timeout_secs: DEFAULT_FORCED_FINALIZE_TIMEOUT_SECS,
            forced_finalize_effort: WebAgentEffort::Standard,
            search_limit: DEFAULT_SEARCH_LIMIT,
            min_effort: WebAgentEffort::Standard,
            public_updates: DEFAULT_PUBLIC_UPDATES,
            forward_tool_events: DEFAULT_FORWARD_TOOL_EVENTS,
            tool_allowlist: default_tool_allowlist(),
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
    let (progress_tx, cancellation_token) = (mpsc::channel(100).0, CancellationToken::new());
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

    match run_request {
        TaskRunRequest::Execute { input, effort } => {
            debug!(
                session_id = %session_id,
                task_id = %task_id,
                max_generations = config.max_generations,
                per_generation_timeout_secs = config.per_generation_timeout_secs,
                total_timeout_secs = config.total_timeout_secs,
                soft_finalize_secs = config.soft_finalize_secs,
                forced_finalize_timeout_secs = config.forced_finalize_timeout_secs,
                ?config.forced_finalize_effort,
                search_limit = config.search_limit,
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
                original_input: &input,
                parent_effort: effort,
                config,
                progress_tx,
                cancellation_token,
            })
            .await;
            let input = if outcome.cancelled {
                input
            } else {
                inject_search_probe_dossier(input, &outcome.handoffs, config.dossier_max_chars)
            };
            (TaskRunRequest::Execute { input, effort }, outcome)
        }
        TaskRunRequest::ResumeUserInput { input, effort } => {
            debug!(
                session_id = %session_id,
                task_id = %task_id,
                "Search Probe skipped for ResumeUserInput"
            );
            (
                TaskRunRequest::ResumeUserInput { input, effort },
                SearchProbeRunOutcome::default(),
            )
        }
    }
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
    let previous_final_message = session_manager
        .last_main_agent_final_message(session_id)
        .await
        .and_then(|message| normalize_previous_final_message(&message));
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
        if config.public_updates {
            send_probe_update(
                &progress_tx,
                generation,
                "Starting web research before the main answer.",
            )
            .await;
        }

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
            if config.public_updates {
                send_probe_update(
                    &progress_tx,
                    generation,
                    "Search Probe could not start; launching the main runtime without probe handoff.",
                )
                .await;
            }
            break;
        };
        executor.session_mut().cancellation_token = cancellation_token.child_token();

        let prompt = build_generation_prompt(
            original_input.text_projection(),
            previous_final_message.as_deref(),
            &handoffs,
            generation,
        );
        let probe_tx = spawn_probe_event_relay(progress_tx.clone(), config.forward_tool_events);
        let execution = executor.execute_user_input_with_options(
            AgentUserInput::new(prompt),
            Some(probe_tx),
            probe_execution_options(
                parent_effort,
                config.min_effort,
                config.search_limit,
                config.soft_finalize_secs,
                generation_timeout,
            ),
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
                if config.public_updates {
                    send_probe_update(
                        &progress_tx,
                        generation,
                        "Search Probe requested extra input unexpectedly; launching the main runtime without a completed probe handoff.",
                    )
                    .await;
                }
                break;
            }
            Ok(Err(error)) => {
                warn!(session_id = %session_id, task_id = %task_id, generation, error = %error, "Search Probe generation failed");
                if config.public_updates {
                    send_probe_update(
                        &progress_tx,
                        generation,
                        "Search Probe could not complete this web pass; the main runtime will continue without a completed probe handoff.",
                    )
                    .await;
                }
                break;
            }
            Err(_) => {
                executor.session_mut().cancellation_token.cancel();
                warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe generation timed out");
                if config.public_updates {
                    send_probe_update(
                        &progress_tx,
                        generation,
                        "Search Probe did not finish before its timeout; launching the main runtime now.",
                    )
                    .await;
                }
                break;
            }
        };

        let mut handoff = handoff_from_generation_text(generation, &text);
        if is_timeout_report(&text) {
            warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe soft finalize timeout report produced");
            if let Some(finalized) = run_forced_finalize(SearchProbeFinalizeCtx {
                session_manager,
                session_id,
                task_id,
                original_prompt: original_input.text_projection(),
                previous_final_message: previous_final_message.as_deref(),
                generation,
                partial_handoff: &handoff,
                config,
                progress_tx: &progress_tx,
                cancellation_token: &cancellation_token,
                started_at,
                total_timeout,
            })
            .await
            {
                handoff = finalized;
            }
        }
        if config.public_updates
            && let Some(update) = handoff.public_update.as_deref()
        {
            send_probe_update(&progress_tx, generation, update).await;
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

async fn run_forced_finalize(ctx: SearchProbeFinalizeCtx<'_>) -> Option<SearchProbeHandoff> {
    let SearchProbeFinalizeCtx {
        session_manager,
        session_id,
        task_id,
        original_prompt,
        previous_final_message,
        generation,
        partial_handoff,
        config,
        progress_tx,
        cancellation_token,
        started_at,
        total_timeout,
    } = ctx;

    if cancellation_token.is_cancelled() {
        return None;
    }

    let total_remaining = remaining_timeout(started_at, total_timeout)?;
    let finalize_timeout = total_remaining.min(std::time::Duration::from_secs(
        config.forced_finalize_timeout_secs.max(1),
    ));
    if finalize_timeout <= std::time::Duration::from_secs(1) {
        warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe skipped forced finalize because no time remains");
        return None;
    }

    let Some(mut executor) = session_manager
        .create_search_probe_executor(
            session_id,
            SearchProbeRuntimeOptions {
                tool_allowlist: Vec::new(),
                prompt_instructions: Some(PROBE_PROFILE_PROMPT.to_string()),
            },
        )
        .await
    else {
        warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe forced finalize parent session not found");
        return None;
    };
    executor.session_mut().cancellation_token = cancellation_token.child_token();

    send_milestone(
        progress_tx,
        &format!("search_probe_generation_{generation}_forced_finalize_started"),
    )
    .await;

    let prompt = build_forced_finalize_prompt(
        original_prompt,
        previous_final_message,
        generation,
        partial_handoff,
    );
    let probe_tx = spawn_probe_event_relay(progress_tx.clone(), false);
    let execution = executor.execute_user_input_with_options(
        AgentUserInput::new(prompt),
        Some(probe_tx),
        forced_finalize_execution_options(config.forced_finalize_effort, finalize_timeout),
    );

    let result = tokio::select! {
        () = cancellation_token.cancelled() => return None,
        result = tokio::time::timeout(finalize_timeout, execution) => result,
    };

    let text = match result {
        Ok(Ok(AgentExecutionOutcome::Completed(text))) => text,
        Ok(Ok(AgentExecutionOutcome::WaitingForUserInput(_))) => {
            warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe forced finalize unexpectedly requested user input");
            return None;
        }
        Ok(Err(error)) => {
            warn!(session_id = %session_id, task_id = %task_id, generation, error = %error, "Search Probe forced finalize failed");
            return None;
        }
        Err(_) => {
            executor.session_mut().cancellation_token.cancel();
            warn!(session_id = %session_id, task_id = %task_id, generation, "Search Probe forced finalize timed out");
            return None;
        }
    };

    let mut handoff = parse_search_probe_contract(generation, &text);
    handoff.decision = SearchProbeDecision::Stop;
    send_milestone(
        progress_tx,
        &format!("search_probe_generation_{generation}_forced_finalize_completed"),
    )
    .await;
    Some(handoff)
}

fn remaining_timeout(
    started_at: tokio::time::Instant,
    total_timeout: std::time::Duration,
) -> Option<std::time::Duration> {
    let elapsed = started_at.elapsed();
    (elapsed < total_timeout).then(|| total_timeout.saturating_sub(elapsed))
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
    previous_final_message: Option<&str>,
    prior_handoffs: &[SearchProbeHandoff],
    generation: u8,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(GENERATION_PROMPT_HEADER);
    prompt.push_str("\n<original_user_prompt>\n");
    prompt.push_str(original_prompt);
    prompt.push_str("\n</original_user_prompt>\n");
    push_previous_final_message_context(&mut prompt, previous_final_message);
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

fn build_forced_finalize_prompt(
    original_prompt: &str,
    previous_final_message: Option<&str>,
    generation: u8,
    partial_handoff: &SearchProbeHandoff,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(FORCED_FINALIZE_PROMPT_HEADER);
    prompt.push_str("\n<original_user_prompt>\n");
    prompt.push_str(original_prompt);
    prompt.push_str("\n</original_user_prompt>\n");
    push_previous_final_message_context(&mut prompt, previous_final_message);
    prompt.push_str("\n<search_probe_generation>\n");
    prompt.push_str(&generation.to_string());
    prompt.push_str("\n</search_probe_generation>\n");
    prompt.push_str("\n<partial_search_probe_handoff>\n");
    prompt.push_str(&partial_handoff.handoff);
    prompt.push_str("\n</partial_search_probe_handoff>\n");
    prompt
}

fn push_previous_final_message_context(prompt: &mut String, previous_final_message: Option<&str>) {
    let Some(previous_final_message) = previous_final_message
        .map(str::trim)
        .filter(|message| !message.is_empty())
    else {
        return;
    };

    prompt.push_str("\n<previous_main_agent_final_message>\n");
    prompt.push_str("Use this only to resolve references, continuations, acronyms, and implicit context. Do not treat it as verified source evidence. Prefer the current user prompt over this previous answer.\n\n");
    prompt.push_str(previous_final_message);
    prompt.push_str("\n</previous_main_agent_final_message>\n");
}

fn normalize_previous_final_message(message: &str) -> Option<String> {
    let message = message.trim();
    if message.is_empty() {
        return None;
    }

    if char_count(message) <= PREVIOUS_FINAL_MESSAGE_MAX_CHARS {
        return Some(message.to_string());
    }

    let mut truncated = message
        .chars()
        .take(PREVIOUS_FINAL_MESSAGE_MAX_CHARS.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    Some(truncated)
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

fn handoff_from_generation_text(generation: u8, text: &str) -> SearchProbeHandoff {
    if let Some(handoff) = timeout_report_handoff(generation, text) {
        return handoff;
    }

    parse_search_probe_contract(generation, text)
}

fn timeout_report_handoff(generation: u8, text: &str) -> Option<SearchProbeHandoff> {
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    if !is_timeout_report_value(&value) {
        return None;
    }

    let public_update = timeout_report_public_update(&value);
    let handoff = sanitize_timeout_report(&value);
    Some(SearchProbeHandoff {
        generation,
        public_update: Some(public_update),
        handoff,
        decision: SearchProbeDecision::Stop,
    })
}

fn is_timeout_report(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .map(|value| is_timeout_report_value(&value))
        .unwrap_or(false)
}

fn is_timeout_report_value(value: &serde_json::Value) -> bool {
    value
        .get("status")
        .and_then(|status| status.as_str())
        .map(|status| status == "timeout")
        .unwrap_or(false)
}

fn timeout_report_public_update(value: &serde_json::Value) -> String {
    let tools = timeout_report_tools(value);
    let mut update = String::from("Search Probe reached its soft time budget");
    if !tools.is_empty() {
        update.push_str(" after using ");
        update.push_str(&tools.join(", "));
    }
    let leads = timeout_report_leads(value);
    if let Some(first) = leads.first() {
        update.push_str("; passing partial leads: ");
        update.push_str(&compact_text(first, 140));
    } else {
        update.push_str("; passing compact partial findings to the main runtime");
    }
    update.push('.');
    update
}

fn sanitize_timeout_report(value: &serde_json::Value) -> String {
    let mut handoff = String::from(
        "Search Probe stopped at its soft time budget. Treat the following as partial leads only.\n",
    );
    push_timeout_report_meta(&mut handoff, value);

    let leads = timeout_report_leads(value);
    if !leads.is_empty() {
        handoff.push_str("\nPartial leads:\n");
        push_bullets(&mut handoff, &leads);
    }

    let failures = timeout_report_failures(value);
    if !failures.is_empty() {
        handoff.push_str("\nTool failures / dead ends:\n");
        push_bullets(&mut handoff, &failures);
    }

    handoff.push_str("\nNext for main runtime:\n");
    handoff.push_str("- Verify source-backed claims before final answer.\n");
    handoff.push_str(
        "- Continue web research only for exact numbers not covered by the partial leads.\n",
    );
    handoff
}

fn push_timeout_report_meta(handoff: &mut String, value: &serde_json::Value) {
    if let Some(reason) = value.get("termination_reason").and_then(|v| v.as_str()) {
        handoff.push_str("Termination: ");
        handoff.push_str(reason.trim());
        handoff.push('\n');
    }
    if let Some(note) = value.get("note").and_then(|v| v.as_str()) {
        handoff.push_str("Note: ");
        handoff.push_str(note.trim());
        handoff.push('\n');
    }
    if let Some(stats) = timeout_report_stats(value) {
        handoff.push_str("Stats: ");
        handoff.push_str(&stats);
        handoff.push('\n');
    }
}

fn timeout_report_stats(value: &serde_json::Value) -> Option<String> {
    let stats = value.get("stats")?;
    let mut parts = Vec::new();
    if let Some(iterations) = stats.get("iterations").and_then(|v| v.as_u64()) {
        parts.push(format!("iterations={iterations}"));
    }
    if let Some(tokens) = stats.get("tokens_used").and_then(|v| v.as_u64()) {
        parts.push(format!("tokens_used={tokens}"));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

fn timeout_report_tools(value: &serde_json::Value) -> Vec<String> {
    let mut tools = Vec::new();
    for message in timeout_report_messages(value) {
        if let Some(tool) = message.get("tool_name").and_then(|v| v.as_str())
            && !tool.trim().is_empty()
            && !tools.iter().any(|existing| existing == tool)
        {
            tools.push(tool.to_owned());
        }
    }
    tools
}

fn timeout_report_leads(value: &serde_json::Value) -> Vec<String> {
    let mut leads = Vec::new();
    for message in timeout_report_messages(value) {
        if leads.len() >= TIMEOUT_REPORT_MAX_ITEMS {
            break;
        }
        let Some(tool) = message.get("tool_name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(content) = message.get("content").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(lead) = summarize_tool_success(tool, content) {
            leads.push(lead);
        }
    }
    leads
}

fn timeout_report_failures(value: &serde_json::Value) -> Vec<String> {
    let mut failures = Vec::new();
    for message in timeout_report_messages(value) {
        if failures.len() >= TIMEOUT_REPORT_MAX_ITEMS {
            break;
        }
        let Some(tool) = message.get("tool_name").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(content) = message.get("content").and_then(|v| v.as_str()) else {
            continue;
        };
        if let Some(failure) = summarize_tool_failure(tool, content) {
            failures.push(failure);
        }
    }
    failures
}

fn timeout_report_messages(value: &serde_json::Value) -> impl Iterator<Item = &serde_json::Value> {
    value
        .get("recent_messages")
        .and_then(|messages| messages.as_array())
        .into_iter()
        .flatten()
}

fn summarize_tool_success(tool: &str, content: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(content).ok();
    if parsed
        .as_ref()
        .and_then(|value| value.get("status"))
        .and_then(|status| status.as_str())
        == Some("failure")
    {
        return None;
    }

    let output = parsed
        .as_ref()
        .and_then(extract_stdout_text)
        .map(str::to_owned)
        .or_else(|| extract_stdout_text_lossy(content));

    let summary = if let Some(text) = output.as_deref() {
        summarize_tool_text(text)
    } else if looks_like_tool_wrapper(content) {
        Some(String::from(
            "tool returned data, but the timeout report truncated it before it could be compacted safely; re-check this source if needed",
        ))
    } else {
        summarize_tool_text(content)
    }?;
    Some(format!("{tool}: {summary}"))
}

fn summarize_tool_failure(tool: &str, content: &str) -> Option<String> {
    let value = serde_json::from_str::<serde_json::Value>(content).ok()?;
    if value.get("status").and_then(|status| status.as_str()) != Some("failure") {
        return None;
    }

    let kind = value
        .get("failure_kind")
        .and_then(|failure_kind| failure_kind.as_str())
        .unwrap_or("failure");
    let summary = value
        .get("summary")
        .and_then(|summary| summary.as_str())
        .unwrap_or("tool call failed");
    Some(format!("{tool}: {kind}: {}", compact_text(summary, 180)))
}

fn extract_stdout_text(value: &serde_json::Value) -> Option<&str> {
    value
        .get("stdout")
        .and_then(|stdout| stdout.get("text"))
        .and_then(|text| text.as_str())
}

fn extract_stdout_text_lossy(content: &str) -> Option<String> {
    let stdout_index = content.find("\"stdout\"")?;
    let after_stdout = &content[stdout_index..];
    let text_index = after_stdout.find("\"text\"")?;
    let after_text = &after_stdout[text_index + "\"text\"".len()..];
    let colon_index = after_text.find(':')?;
    let after_colon = after_text[colon_index + 1..].trim_start();
    let json_string = slice_json_string(after_colon)?;
    serde_json::from_str::<String>(json_string).ok()
}

fn slice_json_string(value: &str) -> Option<&str> {
    let bytes = value.as_bytes();
    if bytes.first().copied() != Some(b'\"') {
        return None;
    }

    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(1) {
        if escaped {
            escaped = false;
            continue;
        }
        match byte {
            b'\\' => escaped = true,
            b'\"' => return value.get(..=index),
            _ => {}
        }
    }
    None
}

fn looks_like_tool_wrapper(content: &str) -> bool {
    let trimmed = content.trim_start();
    trimmed.starts_with("{\"artifacts\"")
        || trimmed.contains("\"cleanup_status\"")
        || trimmed.contains("\"stdout\"")
        || trimmed.contains("\"stderr\"")
}

fn summarize_tool_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(summary) = summarize_crawl_json(trimmed) {
        return Some(summary);
    }
    Some(compact_text(
        &first_useful_lines(trimmed),
        TIMEOUT_REPORT_SNIPPET_CHARS,
    ))
}

fn summarize_crawl_json(text: &str) -> Option<String> {
    let parsed = serde_json::from_str::<serde_json::Value>(text).ok();
    let final_url = parsed
        .as_ref()
        .and_then(|value| value.get("final_url"))
        .and_then(|url| url.as_str())
        .map(str::to_owned)
        .or_else(|| extract_json_string_field_lossy(text, "final_url"))?;
    let chars = parsed
        .as_ref()
        .and_then(|value| value.get("chars"))
        .and_then(|chars| chars.as_u64())
        .or_else(|| extract_json_u64_field_lossy(text, "chars"));
    let mode = parsed
        .as_ref()
        .and_then(|value| value.get("content_mode"))
        .and_then(|mode| mode.as_str())
        .map(str::to_owned)
        .or_else(|| extract_json_string_field_lossy(text, "content_mode"));
    let mut summary = format!("fetched {final_url}");
    if let Some(mode) = mode.as_deref() {
        summary.push_str(" via ");
        summary.push_str(mode);
    }
    if let Some(chars) = chars {
        summary.push_str(&format!(" ({chars} chars)"));
    }
    Some(summary)
}

fn extract_json_string_field_lossy(content: &str, field: &str) -> Option<String> {
    let quoted_field = format!("\"{field}\"");
    let field_index = content.find(&quoted_field)?;
    let after_field = &content[field_index + quoted_field.len()..];
    let colon_index = after_field.find(':')?;
    let after_colon = after_field[colon_index + 1..].trim_start();
    let json_string = slice_json_string(after_colon)?;
    serde_json::from_str::<String>(json_string).ok()
}

fn extract_json_u64_field_lossy(content: &str, field: &str) -> Option<u64> {
    let quoted_field = format!("\"{field}\"");
    let field_index = content.find(&quoted_field)?;
    let after_field = &content[field_index + quoted_field.len()..];
    let colon_index = after_field.find(':')?;
    let digits = after_field[colon_index + 1..]
        .trim_start()
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn first_useful_lines(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" / ")
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let normalized = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if char_count(&normalized) <= max_chars {
        return normalized;
    }
    let mut truncated: String = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    truncated.push('…');
    truncated
}

fn push_bullets(output: &mut String, items: &[String]) {
    for item in items {
        output.push_str("- ");
        output.push_str(item);
        output.push('\n');
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

fn inject_search_probe_dossier(
    mut input: AgentUserInput,
    handoffs: &[SearchProbeHandoff],
    dossier_max_chars: usize,
) -> AgentUserInput {
    let Some(dossier) = render_search_probe_dossier(handoffs, dossier_max_chars) else {
        return input;
    };

    input.content = format!("{}\n\n{}", input.content, dossier);
    input
}

fn render_search_probe_dossier(
    handoffs: &[SearchProbeHandoff],
    dossier_max_chars: usize,
) -> Option<String> {
    let handoffs = handoffs
        .iter()
        .filter(|handoff| !handoff.handoff.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    if handoffs.is_empty() {
        return None;
    }

    let max_chars = dossier_max_chars.max(1);
    let full = render_dossier_from_handoffs(&handoffs, false);
    if char_count(&full) <= max_chars {
        return Some(full);
    }

    for start in 1..handoffs.len() {
        let rendered = render_dossier_from_handoffs(&handoffs[start..], true);
        if char_count(&rendered) <= max_chars {
            return Some(rendered);
        }
    }

    Some(render_truncated_latest_handoff(
        handoffs.last().expect("handoffs is not empty"),
        max_chars,
    ))
}

fn render_truncated_latest_handoff(handoff: &SearchProbeHandoff, max_chars: usize) -> String {
    let original_handoff = handoff.handoff.clone();
    let mut candidate = handoff.clone();
    candidate.public_update = None;
    candidate.handoff.clear();
    let mut best = render_dossier_from_handoffs(std::slice::from_ref(&candidate), true);

    let total_chars = char_count(&original_handoff);
    let mut low = 0usize;
    let mut high = total_chars;
    while low <= high {
        let mid = low + ((high - low) / 2);
        candidate.handoff = if mid == total_chars {
            original_handoff.clone()
        } else {
            format!(
                "[truncated to preserve newest Search Probe handoff]\n{}",
                last_chars(&original_handoff, mid)
            )
        };
        let rendered = render_dossier_from_handoffs(std::slice::from_ref(&candidate), true);
        if char_count(&rendered) <= max_chars {
            best = rendered;
            low = mid.saturating_add(1);
        } else if mid == 0 {
            break;
        } else {
            high = mid - 1;
        }
    }

    best
}

fn render_dossier_from_handoffs(handoffs: &[SearchProbeHandoff], truncated: bool) -> String {
    let mut rendered = String::new();
    rendered.push_str("<search_probe_dossier>\n");
    push_tag(
        &mut rendered,
        "generated_by",
        "web Search Probe before main agent runtime",
    );
    push_tag(
        &mut rendered,
        "guidance",
        "Treat this as research grounding and leads, not final truth. Verify important claims before final answer.",
    );

    for handoff in handoffs {
        rendered.push_str("<generation index=\"");
        rendered.push_str(&handoff.generation.to_string());
        rendered.push_str("\">\n");
        if let Some(update) = handoff.public_update.as_deref()
            && !update.trim().is_empty()
        {
            push_tag(&mut rendered, "public_update", update);
        }
        push_tag(&mut rendered, "handoff", &handoff.handoff);
        push_tag(&mut rendered, "decision", handoff.decision.as_str());
        rendered.push_str("</generation>\n");
    }

    if let Some(latest) = handoffs.last() {
        push_tag(&mut rendered, "final_synthesis", &latest.handoff);
        push_tag(&mut rendered, "decision", latest.decision.as_str());
    }
    push_tag(
        &mut rendered,
        "truncated",
        if truncated { "true" } else { "false" },
    );
    push_tag(
        &mut rendered,
        "instructions_for_main_runtime",
        "Use this as starting context, not proof. Verify source-backed claims before final answer. Do not repeat unsupported assumptions. If sources are insufficient, say so explicitly.",
    );
    rendered.push_str("</search_probe_dossier>");
    rendered
}

fn push_tag(rendered: &mut String, tag: &str, value: &str) {
    rendered.push('<');
    rendered.push_str(tag);
    rendered.push_str(">\n");
    rendered.push_str(&escape_dossier_text(value));
    rendered.push('\n');
    rendered.push_str("</");
    rendered.push_str(tag);
    rendered.push_str(">\n");
}

fn escape_dossier_text(value: &str) -> String {
    value
        .trim()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn char_count(value: &str) -> usize {
    value.chars().count()
}

fn last_chars(value: &str, max_chars: usize) -> String {
    let total = value.chars().count();
    value
        .chars()
        .skip(total.saturating_sub(max_chars))
        .collect()
}

impl SearchProbeDecision {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Continue => "continue",
            Self::Stop => "stop",
        }
    }
}

fn probe_execution_options(
    parent_effort: Option<WebAgentEffort>,
    min_effort: WebAgentEffort,
    search_limit: usize,
    soft_finalize_secs: u64,
    generation_timeout: std::time::Duration,
) -> AgentExecutionOptions {
    let hard_secs = generation_timeout.as_secs().max(1);
    let soft_secs = soft_finalize_secs
        .max(1)
        .min(hard_secs.saturating_sub(1).max(1));

    AgentExecutionOptions::with_effort(web_effort_to_core(max_effort(
        parent_effort.unwrap_or(WebAgentEffort::Standard),
        min_effort,
    )))
    .with_timeout_secs(soft_secs)
    .with_search_limit(search_limit.max(1))
    .with_reasoning_effort("medium")
}

fn forced_finalize_execution_options(
    effort: WebAgentEffort,
    timeout: std::time::Duration,
) -> AgentExecutionOptions {
    AgentExecutionOptions::with_effort(web_effort_to_core(effort))
        .with_timeout_secs(timeout.as_secs().max(1))
        .with_search_limit(1)
        .with_reasoning_effort("medium")
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

async fn send_probe_update(progress_tx: &mpsc::Sender<AgentEvent>, generation: u8, summary: &str) {
    let _ = progress_tx
        .send(AgentEvent::Reasoning {
            source: AgentEventSource::Root,
            summary: format!("Search Probe #{generation}: {summary}"),
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
                .filter(|tool| *tool != SEARCH_PROBE_BLOCKED_TOOL_CRAWL4AI)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|tools| !tools.is_empty())
        .unwrap_or_else(default_tool_allowlist)
}

fn default_tool_allowlist() -> Vec<String> {
    let tools = if oxide_agent_core::config::is_web_crawler_merge_enabled() {
        DEFAULT_MERGED_TOOL_ALLOWLIST
    } else {
        DEFAULT_SPLIT_TOOL_ALLOWLIST
    };

    tools.iter().map(|tool| (*tool).to_owned()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_memory_storage::InMemoryStorage;
    #[cfg(feature = "profile-web-embedded-opencode-local")]
    use async_trait::async_trait;
    use oxide_agent_core::agent::AgentMessageAttachment;
    use oxide_agent_core::config::AgentSettings;
    #[cfg(feature = "profile-web-embedded-opencode-local")]
    use oxide_agent_core::config::ModelInfo;
    use oxide_agent_core::llm::LlmClient;
    #[cfg(feature = "profile-web-embedded-opencode-local")]
    use oxide_agent_core::llm::{
        ChatResponse, ChatWithToolsRequest, LlmError, LlmProvider, Message, TokenUsage,
    };
    use oxide_agent_runtime::SessionRegistry;
    #[cfg(feature = "profile-web-embedded-opencode-local")]
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::{Mutex, MutexGuard};
    use std::time::Duration;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const ENV_KEYS: &[&str] = &[
        ENV_ENABLED,
        ENV_MAX_GENERATIONS,
        ENV_PER_GENERATION_TIMEOUT_SECS,
        ENV_TOTAL_TIMEOUT_SECS,
        ENV_SOFT_FINALIZE_SECS,
        ENV_FORCED_FINALIZE_TIMEOUT_SECS,
        ENV_FORCED_FINALIZE_EFFORT,
        ENV_SEARCH_LIMIT,
        ENV_MIN_EFFORT,
        ENV_PUBLIC_UPDATES,
        ENV_FORWARD_TOOL_EVENTS,
        ENV_TOOL_ALLOWLIST,
        ENV_DOSSIER_MAX_CHARS,
        "OXIDE_WEB_CRAWLER_MERGE",
    ];

    struct EnvGuard(MutexGuard<'static, ()>);

    #[cfg(feature = "profile-web-embedded-opencode-local")]
    struct SequencedTestProvider {
        responses: tokio::sync::Mutex<VecDeque<ChatResponse>>,
    }

    #[cfg(feature = "profile-web-embedded-opencode-local")]
    impl SequencedTestProvider {
        fn new(responses: Vec<ChatResponse>) -> Self {
            Self {
                responses: tokio::sync::Mutex::new(responses.into()),
            }
        }
    }

    #[cfg(feature = "profile-web-embedded-opencode-local")]
    #[async_trait]
    impl LlmProvider for SequencedTestProvider {
        async fn complete_internal_text(
            &self,
            _system_prompt: &str,
            _history: &[Message],
            _user_message: &str,
            _model_id: &str,
            _max_tokens: u32,
        ) -> Result<String, LlmError> {
            Ok("test internal summary".to_string())
        }

        async fn chat_with_tools<'a>(
            &self,
            _request: ChatWithToolsRequest<'a>,
        ) -> Result<ChatResponse, LlmError> {
            self.responses
                .lock()
                .await
                .pop_front()
                .ok_or_else(|| LlmError::ApiError("No test response available".to_string()))
        }

        async fn transcribe_audio(
            &self,
            _audio_bytes: Vec<u8>,
            _mime_type: &str,
            _model_id: &str,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown("transcribe not implemented".to_string()))
        }

        async fn analyze_image(
            &self,
            _image_bytes: Vec<u8>,
            _text_prompt: &str,
            _system_prompt: &str,
            _model_id: &str,
        ) -> Result<String, LlmError> {
            Err(LlmError::Unknown(
                "analyze_image not implemented".to_string(),
            ))
        }
    }

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

    #[cfg(feature = "profile-web-embedded-opencode-local")]
    fn test_session_manager_with_responses(responses: Vec<ChatResponse>) -> WebSessionManager {
        let model_id = "opencode-go/deepseek-v4-flash".to_string();
        let settings = Arc::new(AgentSettings {
            agent_model_id: Some(model_id.clone()),
            agent_model_provider: Some("opencode-go".to_string()),
            agent_model_routes: Some(vec![ModelInfo {
                id: model_id,
                provider: "opencode-go".to_string(),
                max_output_tokens: 32_000,
                context_window_tokens: 200_000,
                weight: 1,
            }]),
            agent_timeout_secs: Some(5),
            ..AgentSettings::default()
        });
        let scripted = Arc::new(SequencedTestProvider::new(responses));
        let mut llm = LlmClient::new(settings.as_ref());
        llm.register_provider("opencode_go".to_string(), scripted.clone());
        llm.register_provider("opencode-go".to_string(), scripted.clone());
        llm.register_provider("llm-provider/opencode-go".to_string(), scripted);

        WebSessionManager::new_with_storage(
            SessionRegistry::new(),
            Arc::new(llm),
            settings,
            Arc::new(InMemoryStorage::new()),
        )
    }

    #[cfg(feature = "profile-web-embedded-opencode-local")]
    fn structured_final_answer_response(final_answer: &str) -> ChatResponse {
        ChatResponse {
            content: Some(
                serde_json::json!({
                    "thought": "done",
                    "tool_call": null,
                    "final_answer": final_answer,
                    "awaiting_user_input": null,
                })
                .to_string(),
            ),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            reasoning_content: None,
            usage: Some(TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
                ..TokenUsage::default()
            }),
        }
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

    fn handoff(generation: u8, handoff: &str, decision: SearchProbeDecision) -> SearchProbeHandoff {
        SearchProbeHandoff {
            generation,
            public_update: Some(format!("public update {generation}")),
            handoff: handoff.to_owned(),
            decision,
        }
    }

    async fn collect_probe_events(mut rx: mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(50), rx.recv()).await
        {
            events.push(event);
        }
        events
    }

    fn milestone_names(events: &[AgentEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Milestone { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    fn reasoning_summaries(events: &[AgentEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::Reasoning { summary, .. } => Some(summary.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn config_defaults_are_disabled_and_web_only() {
        let _guard = lock_env();

        let config = SearchProbeConfig::from_env();

        assert!(!config.enabled);
        assert_eq!(config.max_generations, 2);
        assert_eq!(config.per_generation_timeout_secs, 60);
        assert_eq!(config.total_timeout_secs, 60);
        assert_eq!(config.soft_finalize_secs, 35);
        assert_eq!(config.forced_finalize_timeout_secs, 20);
        assert_eq!(config.forced_finalize_effort, WebAgentEffort::Standard);
        assert_eq!(config.search_limit, 3);
        assert_eq!(config.min_effort, WebAgentEffort::Standard);
        assert!(config.public_updates);
        assert!(config.forward_tool_events);
        assert_eq!(
            config.tool_allowlist,
            vec!["searxng_search", "web_markdown"]
        );
        assert_eq!(config.dossier_max_chars, 80_000);
    }

    #[test]
    fn config_defaults_use_web_crawler_in_merge_mode() {
        let _guard = lock_env();
        test_set_env("OXIDE_WEB_CRAWLER_MERGE", "true");

        let config = SearchProbeConfig::from_env();

        assert_eq!(config.tool_allowlist, vec!["searxng_search", "web_crawler"]);
    }

    #[test]
    fn generation_prompt_includes_previous_final_message_as_context_only() {
        let prompt = build_generation_prompt(
            "How do I configure it remotely?",
            Some("Previous final answer about fastCRW and OpenCode."),
            &[],
            1,
        );

        assert!(prompt.contains("<original_user_prompt>\nHow do I configure it remotely?"));
        assert!(prompt.contains("<previous_main_agent_final_message>"));
        assert!(prompt.contains("Previous final answer about fastCRW and OpenCode."));
        assert!(prompt.contains("Do not treat it as verified source evidence"));
        assert!(prompt.contains("Prefer the current user prompt"));
    }

    #[test]
    fn generation_prompt_omits_empty_previous_final_message() {
        let prompt = build_generation_prompt("fresh question", Some("   \n"), &[], 1);

        assert!(!prompt.contains("<previous_main_agent_final_message>"));
    }

    #[test]
    fn forced_finalize_prompt_includes_previous_final_message_context() {
        let prompt = build_forced_finalize_prompt(
            "current question",
            Some("Previous final answer context."),
            2,
            &handoff(2, "partial lead", SearchProbeDecision::Continue),
        );

        assert!(prompt.contains("<previous_main_agent_final_message>"));
        assert!(prompt.contains("Previous final answer context."));
        assert!(prompt.contains("<partial_search_probe_handoff>\npartial lead"));
    }

    #[test]
    fn previous_final_message_is_trimmed_and_capped() {
        let long = format!("  {}  ", "x".repeat(PREVIOUS_FINAL_MESSAGE_MAX_CHARS + 10));
        let normalized = normalize_previous_final_message(&long).expect("normalized message");

        assert_eq!(char_count(&normalized), PREVIOUS_FINAL_MESSAGE_MAX_CHARS);
        assert!(normalized.ends_with('…'));
    }

    #[test]
    fn config_clamps_generation_count_and_parses_env() {
        let _guard = lock_env();
        test_set_env(ENV_ENABLED, "true");
        test_set_env(ENV_MAX_GENERATIONS, "9");
        test_set_env(ENV_PER_GENERATION_TIMEOUT_SECS, "11");
        test_set_env(ENV_TOTAL_TIMEOUT_SECS, "22");
        test_set_env(ENV_SOFT_FINALIZE_SECS, "7");
        test_set_env(ENV_FORCED_FINALIZE_TIMEOUT_SECS, "8");
        test_set_env(ENV_FORCED_FINALIZE_EFFORT, "extended");
        test_set_env(ENV_SEARCH_LIMIT, "4");
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
        assert_eq!(config.soft_finalize_secs, 7);
        assert_eq!(config.forced_finalize_timeout_secs, 8);
        assert_eq!(config.forced_finalize_effort, WebAgentEffort::Extended);
        assert_eq!(config.search_limit, 4);
        assert_eq!(config.min_effort, WebAgentEffort::Extended);
        assert!(!config.public_updates);
        assert!(!config.forward_tool_events);
        assert_eq!(
            config.tool_allowlist,
            vec!["searxng_search", "web_markdown"]
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

    #[tokio::test]
    #[cfg(feature = "profile-web-embedded-opencode-local")]
    async fn enabled_probe_runs_generations_emits_events_and_injects_dossier() {
        let session_manager = test_session_manager_with_responses(vec![
            structured_final_answer_response(
                r#"<search_probe_public_update>
TL;DR: first pass found source shape.
</search_probe_public_update>
<search_probe_handoff>
generation one handoff with source leads
</search_probe_handoff>
<search_probe_decision>
continue
</search_probe_decision>"#,
            ),
            structured_final_answer_response(
                r#"<search_probe_public_update>
TL;DR: second pass has enough context.
</search_probe_public_update>
<search_probe_handoff>
generation two final synthesis
</search_probe_handoff>
<search_probe_decision>
stop
</search_probe_decision>"#,
            ),
        ]);
        create_parent_session(&session_manager).await;
        let config = SearchProbeConfig {
            enabled: true,
            max_generations: 3,
            per_generation_timeout_secs: 5,
            total_timeout_secs: 10,
            forward_tool_events: false,
            ..SearchProbeConfig::default()
        };
        let (tx, rx) = mpsc::channel(32);

        let (result, outcome) = maybe_run_search_probe_with_runtime(
            &session_manager,
            "session",
            "task",
            execute_request("original prompt"),
            &config,
            tx,
            CancellationToken::new(),
        )
        .await;
        let events = collect_probe_events(rx).await;

        assert!(!outcome.cancelled);
        assert_eq!(outcome.handoffs.len(), 2);
        assert!(matches!(result, TaskRunRequest::Execute { .. }));
        assert!(request_content(&result).starts_with("original prompt\n\n<search_probe_dossier>"));
        assert!(request_content(&result).contains("generation one handoff with source leads"));
        assert!(request_content(&result).contains("generation two final synthesis"));
        assert!(
            request_content(&result).contains("<final_synthesis>\ngeneration two final synthesis")
        );

        let milestones = milestone_names(&events);
        assert!(milestones.contains(&"search_probe_started".to_string()));
        assert!(milestones.contains(&"search_probe_generation_1_started".to_string()));
        assert!(milestones.contains(&"search_probe_generation_1_completed".to_string()));
        assert!(milestones.contains(&"search_probe_generation_2_started".to_string()));
        assert!(milestones.contains(&"search_probe_generation_2_completed".to_string()));
        assert!(milestones.contains(&"search_probe_completed".to_string()));

        let reasoning = reasoning_summaries(&events);
        assert!(
            reasoning
                .iter()
                .any(|summary| summary.contains("Search Probe #1: TL;DR: first pass"))
        );
        assert!(
            reasoning
                .iter()
                .any(|summary| summary.contains("Search Probe #2: TL;DR: second pass"))
        );
    }

    #[tokio::test]
    #[cfg(feature = "profile-web-embedded-opencode-local")]
    async fn timeout_report_runs_forced_finalize_without_tools() {
        let timeout_report = serde_json::json!({
            "status": "timeout",
            "note": "Partial results included.",
            "termination_reason": "Soft timeout reached",
            "recent_messages": [
                {
                    "role": "tool",
                    "tool_name": "searxng_search",
                    "content": "{\"status\":\"success\",\"stdout\":{\"text\":\"## SearXNG results for: Step 3.7 Flash GGUF Q4 Q5\"}}",
                }
            ],
            "stats": {"iterations": 3, "tokens_used": 11970}
        })
        .to_string();
        let session_manager = test_session_manager_with_responses(vec![
            structured_final_answer_response(&timeout_report),
            structured_final_answer_response(
                r#"<search_probe_public_update>
TL;DR: forced finalize compressed partial leads.
</search_probe_public_update>
<search_probe_handoff>
forced finalize handoff without additional web tools
</search_probe_handoff>
<search_probe_decision>
stop
</search_probe_decision>"#,
            ),
        ]);
        create_parent_session(&session_manager).await;
        let config = SearchProbeConfig {
            enabled: true,
            max_generations: 1,
            per_generation_timeout_secs: 20,
            total_timeout_secs: 30,
            forced_finalize_timeout_secs: 10,
            forward_tool_events: false,
            ..SearchProbeConfig::default()
        };
        let (tx, rx) = mpsc::channel(32);

        let (result, outcome) = maybe_run_search_probe_with_runtime(
            &session_manager,
            "session",
            "task",
            execute_request("original prompt"),
            &config,
            tx,
            CancellationToken::new(),
        )
        .await;
        let events = collect_probe_events(rx).await;

        assert!(!outcome.cancelled);
        assert_eq!(outcome.handoffs.len(), 1);
        assert_eq!(outcome.handoffs[0].decision, SearchProbeDecision::Stop);
        assert_eq!(
            outcome.handoffs[0].public_update.as_deref(),
            Some("TL;DR: forced finalize compressed partial leads.")
        );
        assert!(
            outcome.handoffs[0]
                .handoff
                .contains("forced finalize handoff without additional web tools")
        );
        assert!(
            request_content(&result)
                .contains("forced finalize handoff without additional web tools")
        );
        assert!(!request_content(&result).contains("Partial results included."));

        let milestones = milestone_names(&events);
        assert!(
            milestones.contains(&"search_probe_generation_1_forced_finalize_started".to_string())
        );
        assert!(
            milestones.contains(&"search_probe_generation_1_forced_finalize_completed".to_string())
        );
    }

    #[tokio::test]
    async fn enabled_probe_generation_failure_leaves_input_unchanged() {
        let session_manager = test_session_manager();
        create_parent_session(&session_manager).await;
        let config = SearchProbeConfig {
            enabled: true,
            max_generations: 2,
            per_generation_timeout_secs: 5,
            total_timeout_secs: 10,
            ..SearchProbeConfig::default()
        };
        let (tx, rx) = mpsc::channel(16);

        let (result, outcome) = maybe_run_search_probe_with_runtime(
            &session_manager,
            "session",
            "task",
            execute_request("original prompt"),
            &config,
            tx,
            CancellationToken::new(),
        )
        .await;
        let events = collect_probe_events(rx).await;

        assert!(!outcome.cancelled);
        assert!(outcome.handoffs.is_empty());
        assert_eq!(request_content(&result), "original prompt");

        let milestones = milestone_names(&events);
        assert!(milestones.contains(&"search_probe_started".to_string()));
        assert!(milestones.contains(&"search_probe_generation_1_started".to_string()));
        assert!(milestones.contains(&"search_probe_completed".to_string()));

        let reasoning = reasoning_summaries(&events);
        assert!(
            reasoning
                .iter()
                .any(|summary| summary.contains("Search Probe #1: Starting web research"))
        );
        assert!(reasoning.iter().any(|summary| {
            summary.contains("Search Probe #1: Search Probe could not complete this web pass")
        }));
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
    fn timeout_report_becomes_stopping_handoff_with_public_update() {
        let parsed = handoff_from_generation_text(
            1,
            r###"{
  "status": "timeout",
  "note": "Partial results included.",
  "termination_reason": "Soft timeout reached",
  "recent_messages": [
    {
      "role":"tool",
      "tool_name":"searxng_search",
      "content":"{\"status\":\"success\",\"stdout\":{\"text\":\"## SearXNG results for: Step 3.7 Flash GGUF Q4 Q5\\n\\n### Results\\n\\n1. StepFun Step-3.7-Flash-GGUF model page\"}}"
    },
    {
      "role":"assistant",
      "reasoning":"private chain-of-thought should not leak",
      "content":""
    },
    {
      "role":"tool",
      "tool_name":"crawl4ai_markdown",
      "content":"{\"status\":\"failure\",\"failure_kind\":\"anti_bot\",\"summary\":\"Blocked by anti-bot protection\"}"
    }
  ],
  "stats": {"iterations": 3, "tokens_used": 11970}
}"###,
        );

        assert_eq!(parsed.generation, 1);
        assert_eq!(parsed.decision, SearchProbeDecision::Stop);
        assert!(parsed.handoff.contains("Partial results included"));
        assert!(parsed.handoff.contains("Termination: Soft timeout reached"));
        assert!(
            parsed
                .handoff
                .contains("Stats: iterations=3, tokens_used=11970")
        );
        assert!(parsed.handoff.contains("Partial leads:"));
        assert!(
            parsed
                .handoff
                .contains("searxng_search: ## SearXNG results for: Step 3.7 Flash GGUF Q4 Q5")
        );
        assert!(parsed.handoff.contains("Tool failures / dead ends:"));
        assert!(
            parsed
                .handoff
                .contains("crawl4ai_markdown: anti_bot: Blocked by anti-bot protection")
        );
        assert!(!parsed.handoff.contains("recent_messages"));
        assert!(!parsed.handoff.contains("private chain-of-thought"));
        let update = parsed.public_update.as_deref().unwrap_or_default();
        assert!(update.contains("Search Probe reached its soft time budget after using searxng_search, crawl4ai_markdown"));
        assert!(update.contains("passing partial leads"));
    }

    #[test]
    fn timeout_report_sanitizer_unwraps_tool_output_wrappers() {
        let truncated_crawl_wrapper = r#"{"artifacts":[],"cancellation_reason":null,"cleanup_status":"not_needed","duration_ms":5894,"error_message":null,"exit_code":null,"status":"success","stderr":{"text":""},"stdout":{"text":"{\"chars\":5223,\"content_mode\":\"crawl4ai_fit_markdown\",\"final_url\":\"https://huggingface.co/unsloth/Step-3.7-Flash-GGUF\"}"},"#;
        let searxng_wrapper = r###"{"artifacts":[],"cleanup_status":"not_needed","status":"success","stdout":{"text":"## SearXNG results for: DDR3 ECC 2133 quad channel RAM bandwidth\n\n### Results\n\n1. llama.cpp / ik_llama MoE Expert Offloading"},"stderr":{"text":""}}"###;
        let report = serde_json::json!({
            "status": "timeout",
            "note": "Partial results included.",
            "termination_reason": "Soft timeout reached",
            "recent_messages": [
                {
                    "role": "tool",
                    "tool_name": "crawl4ai_markdown",
                    "content": truncated_crawl_wrapper,
                },
                {
                    "role": "tool",
                    "tool_name": "searxng_search",
                    "content": searxng_wrapper,
                },
                {
                    "role": "assistant",
                    "reasoning": "private reasoning must not leak",
                    "content": "",
                }
            ],
            "stats": {"iterations": 3, "tokens_used": 13667}
        });

        let parsed = handoff_from_generation_text(1, &report.to_string());

        assert_eq!(parsed.decision, SearchProbeDecision::Stop);
        assert!(parsed.handoff.contains(
            "crawl4ai_markdown: fetched https://huggingface.co/unsloth/Step-3.7-Flash-GGUF via crawl4ai_fit_markdown (5223 chars)"
        ));
        assert!(parsed.handoff.contains(
            "searxng_search: ## SearXNG results for: DDR3 ECC 2133 quad channel RAM bandwidth"
        ));
        assert!(!parsed.handoff.contains("{\"artifacts\""));
        assert!(!parsed.handoff.contains("cleanup_status"));
        assert!(!parsed.handoff.contains("\"stdout\""));
        assert!(!parsed.handoff.contains("\"stderr\""));
        assert!(!parsed.handoff.contains("private reasoning"));

        let update = parsed.public_update.as_deref().unwrap_or_default();
        assert!(update.contains("fetched https://huggingface.co/unsloth/Step-3.7-Flash-GGUF"));
        assert!(!update.contains("{\"artifacts\""));
        assert!(!update.contains("cleanup_status"));
    }

    #[test]
    fn dossier_renderer_returns_none_without_handoffs() {
        assert_eq!(render_search_probe_dossier(&[], 80_000), None);

        let empty = SearchProbeHandoff {
            generation: 1,
            public_update: None,
            handoff: "   ".to_owned(),
            decision: SearchProbeDecision::Continue,
        };
        assert_eq!(render_search_probe_dossier(&[empty], 80_000), None);
    }

    #[test]
    fn dossier_renderer_uses_xml_like_envelope_and_escapes_content() {
        let rendered = render_search_probe_dossier(
            &[SearchProbeHandoff {
                generation: 1,
                public_update: Some("TL;DR: A < B & C".to_owned()),
                handoff: "Use <source> & verify > claims".to_owned(),
                decision: SearchProbeDecision::Stop,
            }],
            80_000,
        )
        .expect("dossier");

        assert!(rendered.starts_with("<search_probe_dossier>"));
        assert!(rendered.contains("<generation index=\"1\">"));
        assert!(rendered.contains("TL;DR: A &lt; B &amp; C"));
        assert!(rendered.contains("Use &lt;source&gt; &amp; verify &gt; claims"));
        assert!(rendered.contains("<decision>\nstop\n</decision>"));
        assert!(rendered.contains("<truncated>\nfalse\n</truncated>"));
        assert!(!rendered.contains("<original_user_prompt>"));
    }

    #[test]
    fn dossier_injection_appends_after_original_prompt_and_preserves_attachments() {
        let attachment = AgentMessageAttachment::image(
            "shot.png",
            Some("image/png".to_owned()),
            123,
            "/workspace/uploads/shot.png",
        );
        let input = AgentUserInput::new("original task").with_attachments(vec![attachment.clone()]);

        let injected = inject_search_probe_dossier(
            input,
            &[handoff(1, "compact handoff", SearchProbeDecision::Stop)],
            80_000,
        );

        assert!(
            injected
                .content
                .starts_with("original task\n\n<search_probe_dossier>")
        );
        assert!(injected.content.contains("compact handoff"));
        assert_eq!(injected.attachments, vec![attachment]);
    }

    #[test]
    fn dossier_injection_noops_without_handoffs() {
        let input = AgentUserInput::new("original task");

        let injected = inject_search_probe_dossier(input, &[], 80_000);

        assert_eq!(injected.content, "original task");
        assert!(injected.attachments.is_empty());
    }

    #[test]
    fn dossier_truncation_preserves_newest_handoff_first() {
        let rendered = render_search_probe_dossier(
            &[
                handoff(1, &"old ".repeat(2_000), SearchProbeDecision::Continue),
                handoff(2, "newest critical handoff", SearchProbeDecision::Stop),
            ],
            1_200,
        )
        .expect("dossier");

        assert!(rendered.contains("<truncated>\ntrue\n</truncated>"));
        assert!(!rendered.contains("old old old"));
        assert!(rendered.contains("newest critical handoff"));
        assert!(char_count(&rendered) <= 1_200);
    }

    #[test]
    fn probe_effort_uses_configured_minimum() {
        assert_eq!(
            probe_execution_options(
                Some(WebAgentEffort::Standard),
                WebAgentEffort::Heavy,
                3,
                35,
                Duration::from_secs(60),
            )
            .effort,
            AgentExecutionEffort::Heavy
        );
        assert_eq!(
            probe_execution_options(
                Some(WebAgentEffort::Heavy),
                WebAgentEffort::Extended,
                3,
                35,
                Duration::from_secs(60),
            )
            .effort,
            AgentExecutionEffort::Heavy
        );
        assert_eq!(
            probe_execution_options(
                Some(WebAgentEffort::Heavy),
                WebAgentEffort::Standard,
                3,
                35,
                Duration::from_secs(60),
            )
            .reasoning_effort_override,
            Some("medium")
        );
    }

    #[test]
    fn probe_execution_options_apply_soft_timeout_and_search_limit() {
        let options = probe_execution_options(
            Some(WebAgentEffort::Standard),
            WebAgentEffort::Heavy,
            3,
            35,
            Duration::from_secs(60),
        );

        assert_eq!(options.timeout_secs, Some(35));
        assert_eq!(options.search_limit, Some(3));
        assert_eq!(options.reasoning_effort_override, Some("medium"));

        let options = probe_execution_options(
            Some(WebAgentEffort::Standard),
            WebAgentEffort::Heavy,
            0,
            35,
            Duration::from_secs(10),
        );

        assert_eq!(options.timeout_secs, Some(9));
        assert_eq!(options.search_limit, Some(1));
    }

    #[test]
    fn forced_finalize_options_disable_search_by_policy() {
        let options =
            forced_finalize_execution_options(WebAgentEffort::Standard, Duration::from_secs(20));

        assert_eq!(options.effort, AgentExecutionEffort::Standard);
        assert_eq!(options.timeout_secs, Some(20));
        assert_eq!(options.search_limit, Some(1));
        assert_eq!(options.reasoning_effort_override, Some("medium"));
    }

    #[tokio::test]
    async fn public_update_uses_reasoning_event() {
        let (tx, mut rx) = mpsc::channel(2);

        send_probe_update(&tx, 2, "TL;DR: checked the docs.").await;
        drop(tx);

        let event = rx.recv().await.expect("reasoning event");
        match event {
            AgentEvent::Reasoning { source, summary } => {
                assert_eq!(source, AgentEventSource::Root);
                assert_eq!(summary, "Search Probe #2: TL;DR: checked the docs.");
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
