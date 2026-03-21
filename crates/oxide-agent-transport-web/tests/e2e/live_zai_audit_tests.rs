//! Live ZAI-backed E2E coverage for heavy sandbox-driven audits.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oxide_agent_transport_web::session::{TaskStatus, WebSessionManager};
use serde_json::Value;

use crate::helpers::{
    create_session_http_with_user, create_task_http_with_body, fetch_task_events,
    fetch_task_progress, spawn_test_server,
};
use crate::setup::{cleanup_web_sandbox, setup_live_zai_test};

const MAX_ATTEMPTS: usize = 3;

const HEAVY_AUDIT_PROMPT: &str = r#"Ты работаешь в Linux sandbox.
Нужно сделать подробный аудит окружения и инструментов, не останавливаясь на кратких выводах:
1. Собери инвентарь CLI-инструментов и пакетов.
2. Исследуй /bin, /usr/bin, /usr/sbin, /etc, /var/lib/dpkg/info и /var/lib/dpkg/status.
3. Посмотри версии shell, coreutils, git, python, pip, node, npm, cargo, go, gcc, clang, make, docker-related CLI, если есть.
4. Прочитай несколько крупных файлов конфигурации и package metadata.
5. Найди дубликаты, конфликты версий и подозрительные несоответствия.
6. Веди todo, отмечай, что уже сделано и что осталось.
7. В конце дай краткий отчет: findings, risks, remaining work.
Важно: сначала собирай сырые данные командами, потом уже делай выводы.
"#;

struct LiveAuditArtifacts {
    terminal_status: TaskStatus,
    progress_status: reqwest::StatusCode,
    progress: Value,
    events: Vec<Value>,
    timeline: Value,
}

#[tokio::test]
#[ignore = "Requires RUN_LLM_E2E_CHECKS=1, ZAI_API_KEY, Docker sandbox, and network access"]
async fn e2e_zai_heavy_sandbox_audit_logs_baselines() {
    if std::env::var("RUN_LLM_E2E_CHECKS").as_deref() != Ok("1") {
        eprintln!("[LIVE-ZAI] Skipping heavy audit test: RUN_LLM_E2E_CHECKS != 1");
        return;
    }

    let app_state = match setup_live_zai_test() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("[LIVE-ZAI] Skipping heavy audit test: {error}");
            return;
        }
    };

    let session_manager = app_state.session_manager();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();
    let overall_start = Instant::now();
    for attempt in 1..=MAX_ATTEMPTS {
        let user_id = unique_test_user_id();
        eprintln!(
            "[LIVE-ZAI] Attempt {attempt}/{MAX_ATTEMPTS}: starting heavy sandbox audit with user_id={user_id}"
        );

        let create_session_start = Instant::now();
        let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
        eprintln!(
            "[LIVE-ZAI] Session created in {}ms: {}",
            create_session_start.elapsed().as_millis(),
            session_id
        );

        let create_task_start = Instant::now();
        let task_id =
            create_task_http_with_body(&client, &base_url, &session_id, HEAVY_AUDIT_PROMPT).await;
        eprintln!(
            "[LIVE-ZAI] Task submitted in {}ms: {}",
            create_task_start.elapsed().as_millis(),
            task_id
        );

        let artifacts =
            run_live_audit_scenario(&client, &base_url, &session_manager, &session_id, &task_id)
                .await;
        let event_names = event_names(&artifacts.events);

        log_live_attempt(&artifacts, &event_names);

        let retryable = is_retryable_live_failure(&artifacts, &event_names);
        let validation_result = validate_successful_live_audit(&artifacts, &event_names);

        cleanup_live_attempt(&client, &base_url, &session_id, user_id).await;

        if matches!(artifacts.terminal_status, TaskStatus::Completed) && validation_result.is_ok() {
            eprintln!(
                "[LIVE-ZAI] Heavy audit completed in {}ms",
                overall_start.elapsed().as_millis()
            );
            server.abort();
            return;
        }

        if retryable {
            if attempt < MAX_ATTEMPTS {
                eprintln!(
                    "[LIVE-ZAI] Transient provider-side failure detected, retrying attempt {}",
                    attempt + 1
                );
                continue;
            }

            server.abort();
            panic!(
                "heavy audit exhausted {} transient retries; last_status={:?}; last_progress_error={}; last_events={:?}",
                MAX_ATTEMPTS,
                artifacts.terminal_status,
                progress_error(&artifacts),
                event_names
            );
        }

        if let Err(message) = validation_result {
            server.abort();
            panic!(
                "heavy audit attempt failed: {message}; status={:?}; progress_error={}; events={:?}",
                artifacts.terminal_status,
                progress_error(&artifacts),
                event_names
            );
        }

        server.abort();
        panic!(
            "heavy audit failed without retry classification; status={:?}; progress_error={}; events={:?}",
            artifacts.terminal_status,
            progress_error(&artifacts),
            event_names
        );
    }
}

async fn run_live_audit_scenario(
    client: &reqwest::Client,
    base_url: &str,
    session_manager: &std::sync::Arc<WebSessionManager>,
    session_id: &str,
    task_id: &str,
) -> LiveAuditArtifacts {
    let terminal_status =
        wait_for_terminal_task_status(session_manager.as_ref(), task_id, Duration::from_secs(480))
            .await;
    let progress_response = fetch_task_progress(client, base_url, session_id, task_id).await;
    let progress_status = progress_response.status();
    let progress = progress_response
        .json()
        .await
        .expect("failed to decode task progress");
    let events = fetch_task_events(client, base_url, session_id, task_id).await;
    let timeline = client
        .get(format!(
            "{base_url}/sessions/{session_id}/tasks/{task_id}/timeline"
        ))
        .send()
        .await
        .expect("failed to fetch task timeline")
        .json()
        .await
        .expect("failed to decode task timeline");

    LiveAuditArtifacts {
        terminal_status,
        progress_status,
        progress,
        events,
        timeline,
    }
}

async fn cleanup_live_attempt(
    client: &reqwest::Client,
    base_url: &str,
    session_id: &str,
    user_id: i64,
) {
    let delete_status = client
        .delete(format!("{base_url}/sessions/{session_id}"))
        .send()
        .await
        .expect("failed to delete live test session")
        .status();
    eprintln!("[LIVE-ZAI] Session delete status: {delete_status}");

    match cleanup_web_sandbox(user_id).await {
        Ok(removed) => eprintln!("[LIVE-ZAI] Sandbox cleanup removed_container={removed}"),
        Err(error) => eprintln!("[LIVE-ZAI] Sandbox cleanup failed: {error}"),
    }
}

fn log_live_attempt(artifacts: &LiveAuditArtifacts, event_names: &[String]) {
    log_timeline(&artifacts.timeline);
    log_tool_calls(&artifacts.timeline);
    eprintln!(
        "[LIVE-ZAI] Terminal status: {:?}",
        artifacts.terminal_status
    );
    eprintln!("[LIVE-ZAI] Progress error: {}", progress_error(artifacts));
    eprintln!("[LIVE-ZAI] Event count: {}", event_names.len());
    eprintln!("[LIVE-ZAI] Events: {:?}", event_names);
}

fn event_names(events: &[Value]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| event["event_name"].as_str())
        .map(str::to_string)
        .collect()
}

fn validate_successful_live_audit(
    artifacts: &LiveAuditArtifacts,
    event_names: &[String],
) -> Result<(), String> {
    if !matches!(artifacts.terminal_status, TaskStatus::Completed) {
        return Err("task did not complete successfully".to_string());
    }
    if !artifacts.progress_status.is_success() {
        return Err("progress endpoint returned non-success status".to_string());
    }
    if !event_names.iter().any(|name| name == "finished") {
        return Err("finished event missing".to_string());
    }
    if !event_names
        .iter()
        .any(|name| name == "tool_call:write_todos")
    {
        return Err("write_todos tool call missing".to_string());
    }
    if !event_names.iter().any(|name| {
        matches!(
            name.as_str(),
            "tool_call:execute_command" | "tool_call:read_file" | "tool_call:list_files"
        )
    }) {
        return Err("sandbox tool call missing".to_string());
    }
    if !event_names
        .iter()
        .any(|name| name.starts_with("tool_result:"))
    {
        return Err("tool result event missing".to_string());
    }
    if event_names.iter().any(|name| name == "error") {
        return Err("unexpected error event present".to_string());
    }

    for (start, end) in [
        ("executor_lock_acquired_ms", "prepare_execution_done_ms"),
        ("prepare_execution_done_ms", "pre_run_compaction_done_ms"),
        ("pre_run_compaction_done_ms", "thinking_sent_ms"),
        // NOTE: thinking_sent_ms vs llm_call_started_ms is intentionally NOT asserted
        // here because multi-round execution emits thinking_sent in a later round
        // after tools have run, while llm_call_started_ms refers to the first round.
        ("llm_call_started_ms", "first_tool_call_ms"),
        ("first_tool_call_ms", "last_tool_call_ms"),
        ("first_tool_call_ms", "first_tool_result_ms"),
        ("first_tool_result_ms", "last_tool_result_ms"),
        ("llm_call_started_ms", "final_response_ms"),
    ] {
        assert_monotonic(&artifacts.timeline, start, end);
    }

    Ok(())
}

fn is_retryable_live_failure(artifacts: &LiveAuditArtifacts, event_names: &[String]) -> bool {
    if event_names
        .iter()
        .any(|name| name == "rate_limit_retrying" || name == "llm_retrying")
    {
        return true;
    }

    let error = progress_error(artifacts).to_ascii_lowercase();
    [
        "rate limit",
        "429",
        "too many requests",
        "network",
        "stream error",
        "unexpected upstream payload",
        "timeout",
        "timed out",
        "502",
        "503",
        "504",
        "overloaded",
        "connection reset",
        "temporarily unavailable",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

fn progress_error(artifacts: &LiveAuditArtifacts) -> &str {
    artifacts.progress["error"]
        .as_str()
        .unwrap_or("<no progress error>")
}

async fn wait_for_terminal_task_status(
    session_manager: &WebSessionManager,
    task_id: &str,
    timeout: Duration,
) -> TaskStatus {
    let deadline = Instant::now() + timeout;

    loop {
        let task = session_manager
            .get_task(task_id)
            .await
            .expect("task metadata should exist while waiting for terminal status");

        match task.status {
            TaskStatus::Completed => return TaskStatus::Completed,
            TaskStatus::Cancelled => return TaskStatus::Cancelled,
            TaskStatus::Failed => return TaskStatus::Failed,
            TaskStatus::Running => {}
        }

        assert!(
            Instant::now() < deadline,
            "task {task_id} did not reach a terminal state in time"
        );
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn unique_test_user_id() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before unix epoch")
        .as_millis();
    let bounded = millis % (i64::MAX as u128);
    bounded as i64
}

fn log_timeline(timeline: &Value) {
    let milestones = &timeline["milestones"];

    eprintln!("[LIVE-ZAI] Milestones:");
    for key in [
        "session_ready_ms",
        "executor_lock_acquired_ms",
        "prepare_execution_done_ms",
        "pre_run_compaction_done_ms",
        "thinking_sent_ms",
        "llm_call_started_ms",
        "first_tool_call_ms",
        "last_tool_call_ms",
        "first_tool_result_ms",
        "last_tool_result_ms",
        "first_thinking_ms",
        "final_response_ms",
    ] {
        eprintln!("  - {key}: {:?}", milestone_ms(timeline, key));
    }

    log_phase_breakdown(milestones);
}

fn log_phase_breakdown(milestones: &Value) {
    let bootstrap_ms = diff(
        milestones["executor_lock_acquired_ms"].as_i64(),
        milestones["llm_call_started_ms"].as_i64(),
    );
    let api_to_first_tool_ms = diff(
        milestones["llm_call_started_ms"].as_i64(),
        milestones["first_tool_call_ms"].as_i64(),
    );
    let tool_window_ms = diff(
        milestones["first_tool_call_ms"].as_i64(),
        milestones["last_tool_result_ms"].as_i64(),
    );
    let wrap_up_ms = diff(
        milestones["last_tool_result_ms"].as_i64(),
        milestones["final_response_ms"].as_i64(),
    );

    eprintln!("[LIVE-ZAI] Phase attribution:");
    eprintln!("  - bootstrap/architecture: {:?}ms", bootstrap_ms);
    eprintln!("  - llm/api to first tool: {:?}ms", api_to_first_tool_ms);
    eprintln!("  - sandbox/tool window: {:?}ms", tool_window_ms);
    eprintln!("  - wrap-up/finalization: {:?}ms", wrap_up_ms);
}

fn log_tool_calls(timeline: &Value) {
    let Some(tool_calls) = timeline["tool_calls"].as_array() else {
        eprintln!("[LIVE-ZAI] No tool call timings returned");
        return;
    };

    eprintln!("[LIVE-ZAI] Tool timings ({} entries):", tool_calls.len());
    for (idx, tool_call) in tool_calls.iter().enumerate() {
        eprintln!(
            "  - #{idx}: name={} started_at_ms={:?} finished_at_ms={:?}",
            tool_call["name"].as_str().unwrap_or("<unknown>"),
            tool_call["started_at_ms"].as_i64(),
            tool_call["finished_at_ms"].as_i64()
        );
    }
}

fn milestone_ms(timeline: &Value, key: &str) -> Option<i64> {
    timeline["milestones"][key].as_i64()
}

fn diff(start: Option<i64>, end: Option<i64>) -> Option<i64> {
    match (start, end) {
        (Some(start), Some(end)) if end >= start => Some(end - start),
        _ => None,
    }
}

fn assert_monotonic(timeline: &Value, start_key: &str, end_key: &str) {
    let start = milestone_ms(timeline, start_key);
    let end = milestone_ms(timeline, end_key);

    if let (Some(start), Some(end)) = (start, end) {
        assert!(
            end >= start,
            "expected {end_key} ({end}) to be >= {start_key} ({start})"
        );
    }
}
