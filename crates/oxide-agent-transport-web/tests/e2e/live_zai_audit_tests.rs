//! Live ZAI-backed E2E coverage for heavy sandbox-driven audits.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use oxide_agent_core::agent::memory::{AgentMessage, MessageRole};
use oxide_agent_core::agent::PersistentMemoryStore;
use oxide_agent_core::agent::SessionId;
use oxide_agent_memory::{EpisodeSearchFilter, MemorySearchFilter};
use oxide_agent_transport_web::session::{TaskStatus, WebSessionManager};
use serde_json::Value;

use crate::helpers::{
    create_session_http_with_user, create_session_http_with_user_and_context,
    create_task_http_with_body, fetch_task_events, fetch_task_progress, spawn_test_server,
};
use crate::setup::{cleanup_web_sandbox, setup_live_zai_test, setup_live_zai_test_with_postgres};

const MAX_ATTEMPTS: usize = 3;
const LIVE_ANCHOR_MAX_ATTEMPTS: usize = 2;

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

fn derive_session_id(session_id: &str, user_id: i64) -> SessionId {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    session_id.hash(&mut h);
    user_id.hash(&mut h);
    SessionId::from(h.finish() as i64)
}

async fn seed_history(
    session_manager: &WebSessionManager,
    session_id: &str,
    user_id: i64,
    messages: Vec<AgentMessage>,
) {
    let sid = derive_session_id(session_id, user_id);
    let executor_arc = session_manager
        .session_registry()
        .get(&sid)
        .await
        .expect("session should exist in registry");

    let mut executor = executor_arc.write().await;
    for message in messages {
        executor.session_mut().memory.add_message(message);
    }
}

async fn latest_assistant_response(
    session_manager: &WebSessionManager,
    session_id: &str,
    user_id: i64,
) -> Option<String> {
    let sid = derive_session_id(session_id, user_id);
    let executor_arc = session_manager.session_registry().get(&sid).await?;
    let executor = executor_arc.read().await;
    executor
        .session()
        .memory
        .get_messages()
        .iter()
        .rev()
        .find(|message| message.role == MessageRole::Assistant)
        .map(|message| message.content.clone())
}

async fn wait_for_durable_memory_hit(
    store: &dyn PersistentMemoryStore,
    user_id: i64,
    context_key: &str,
    token: &str,
    timeout: Duration,
) -> bool {
    let deadline = Instant::now() + timeout;

    loop {
        let memory_hits = PersistentMemoryStore::search_memories_lexical(
            store,
            token,
            &MemorySearchFilter {
                context_key: Some(context_key.to_string()),
                user_id: Some(user_id),
                limit: Some(5),
                ..Default::default()
            },
        )
        .await
        .expect("memory search should succeed");
        if !memory_hits.is_empty() {
            return true;
        }

        let episode_hits = PersistentMemoryStore::search_episodes_lexical(
            store,
            token,
            &EpisodeSearchFilter {
                context_key: Some(context_key.to_string()),
                user_id: Some(user_id),
                limit: Some(5),
                ..Default::default()
            },
        )
        .await
        .expect("episode search should succeed");
        if !episode_hits.is_empty() {
            return true;
        }

        if Instant::now() >= deadline {
            return false;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn durable_memory_hit_exists(
    store: &dyn PersistentMemoryStore,
    user_id: i64,
    context_key: &str,
    token: &str,
) -> bool {
    let memory_hits = PersistentMemoryStore::search_memories_lexical(
        store,
        token,
        &MemorySearchFilter {
            context_key: Some(context_key.to_string()),
            user_id: Some(user_id),
            limit: Some(5),
            ..Default::default()
        },
    )
    .await
    .expect("memory search should succeed");
    if !memory_hits.is_empty() {
        return true;
    }

    let episode_hits = PersistentMemoryStore::search_episodes_lexical(
        store,
        token,
        &EpisodeSearchFilter {
            context_key: Some(context_key.to_string()),
            user_id: Some(user_id),
            limit: Some(5),
            ..Default::default()
        },
    )
    .await
    .expect("episode search should succeed");

    !episode_hits.is_empty()
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

#[tokio::test]
#[ignore = "Requires RUN_LLM_E2E_CHECKS=1, ZAI_API_KEY, Docker sandbox, and network access"]
async fn e2e_zai_seeded_initial_anchor_missing_after_healthy_cleanup() {
    if std::env::var("RUN_LLM_E2E_CHECKS").as_deref() != Ok("1") {
        eprintln!("[LIVE-ZAI] Skipping anchor cleanup test: RUN_LLM_E2E_CHECKS != 1");
        return;
    }

    let app_state = match setup_live_zai_test() {
        Ok(state) => state,
        Err(error) => {
            eprintln!("[LIVE-ZAI] Skipping anchor cleanup test: {error}");
            return;
        }
    };

    let session_manager = app_state.session_manager();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();

    for attempt in 1..=LIVE_ANCHOR_MAX_ATTEMPTS {
        let user_id = unique_test_user_id();
        let anchor = format!("ANCHOR_CTX_{user_id:x}_7f4c2b9d1e6a3c8f");
        let anchor_tail = format!("{user_id:x}_7f4c2b9d1e6a3c8f");
        let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
        let old_payload = format!("{}{}", "x".repeat(1_400), anchor);

        seed_history(
            session_manager.as_ref(),
            &session_id,
            user_id,
            vec![
                AgentMessage::user_task("Recall the exact initial anchor from prior context."),
                AgentMessage::tool("old-anchor", "web_markdown", &old_payload),
                AgentMessage::tool("recent-1", "web_markdown", "short-1"),
                AgentMessage::tool("recent-2", "web_markdown", "short-2"),
                AgentMessage::tool("recent-3", "web_markdown", "short-3"),
                AgentMessage::tool("recent-4", "web_markdown", "short-4"),
            ],
        )
        .await;

        let task_id = create_task_http_with_body(
            &client,
            &base_url,
            &session_id,
            "Return the exact anchor token from the initial session context and nothing else. Do not use tools unless strictly necessary.",
        )
        .await;

        let artifacts =
            run_live_audit_scenario(&client, &base_url, &session_manager, &session_id, &task_id)
                .await;
        let event_names = event_names(&artifacts.events);
        let final_response =
            latest_assistant_response(session_manager.as_ref(), &session_id, user_id)
                .await
                .unwrap_or_default();

        eprintln!(
            "[LIVE-ZAI] Anchor cleanup attempt {attempt}/{LIVE_ANCHOR_MAX_ATTEMPTS}: final_response={:?}",
            final_response
        );
        log_live_attempt(&artifacts, &event_names);

        let retryable = is_retryable_live_failure(&artifacts, &event_names);
        cleanup_live_attempt(&client, &base_url, &session_id, user_id).await;

        if retryable {
            if attempt < LIVE_ANCHOR_MAX_ATTEMPTS {
                eprintln!(
                    "[LIVE-ZAI] Anchor cleanup test saw transient failure, retrying attempt {}",
                    attempt + 1
                );
                continue;
            }

            server.abort();
            panic!(
                "anchor cleanup test exhausted retries; status={:?}; progress_error={}; events={:?}",
                artifacts.terminal_status,
                progress_error(&artifacts),
                event_names
            );
        }

        assert!(matches!(artifacts.terminal_status, TaskStatus::Completed));
        assert!(artifacts.progress_status.is_success());
        assert_eq!(
            artifacts.progress["latest_token_snapshot"]["budget_state"],
            "Healthy"
        );
        assert!(artifacts.progress["last_compaction_status"]
            .as_str()
            .is_some_and(|value| value.contains("Cleanup:")));
        assert!(
            final_response.contains(&anchor_tail),
            "anchor tail missing despite healthy budget and no cleanup; final_response={final_response:?}; anchor={anchor:?}"
        );

        server.abort();
        return;
    }

    server.abort();
}

#[tokio::test]
#[ignore = "Requires RUN_LLM_E2E_CHECKS=1, ZAI_API_KEY, MEMORY_DATABASE_URL, Postgres, Docker sandbox, and network access"]
async fn e2e_zai_postgres_memory_survives_session_and_process_restart() {
    if std::env::var("RUN_LLM_E2E_CHECKS").as_deref() != Ok("1") {
        eprintln!("[LIVE-ZAI] Skipping Postgres durable memory test: RUN_LLM_E2E_CHECKS != 1");
        return;
    }

    let ((app_state, memory_store), user_id, token) = {
        let setup = match setup_live_zai_test_with_postgres().await {
            Ok(value) => value,
            Err(error) => {
                eprintln!("[LIVE-ZAI] Skipping Postgres durable memory test: {error}");
                return;
            }
        };
        let user_id = unique_test_user_id();
        let token = format!("DURABLE_TOKEN_{user_id:x}_kakoune");
        (setup, user_id, token)
    };

    let session_manager = app_state.session_manager();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();

    let session_id = create_session_http_with_user(&client, &base_url, user_id).await;
    let first_sid = derive_session_id(&session_id, user_id);
    let first_executor = session_manager
        .session_registry()
        .get(&first_sid)
        .await
        .expect("first session should exist in registry");
    assert!(
        first_executor.read().await.has_persistent_memory(),
        "first executor missing durable persistent-memory wiring"
    );
    let remember_prompt = format!(
        "Запомни для будущих разговоров дословно: {token}. Это моя важная постоянная заметка. Ответь коротко, что запомнил.",
    );
    let task_id =
        create_task_http_with_body(&client, &base_url, &session_id, &remember_prompt).await;
    let first_status =
        wait_for_terminal_task_status(session_manager.as_ref(), &task_id, Duration::from_secs(240))
            .await;
    let first_progress = fetch_task_progress(&client, &base_url, &session_id, &task_id)
        .await
        .json::<Value>()
        .await
        .expect("failed to decode first task progress");

    assert!(
        matches!(first_status, TaskStatus::Completed),
        "remember task failed: {:?}; progress={first_progress:?}",
        first_status
    );

    let durable_hit = wait_for_durable_memory_hit(
        memory_store.as_ref(),
        user_id,
        "default",
        &token,
        Duration::from_secs(30),
    )
    .await;
    assert!(
        durable_hit,
        "durable memory token was not persisted to Postgres: {token}"
    );

    cleanup_live_attempt(&client, &base_url, &session_id, user_id).await;
    server.abort();

    let (recall_state, _recall_store) = setup_live_zai_test_with_postgres()
        .await
        .expect("second Postgres-backed app state should initialize");
    let recall_session_manager = recall_state.session_manager();
    let (recall_server, recall_base_url) = spawn_test_server(recall_state).await;

    let recall_session_id = create_session_http_with_user(&client, &recall_base_url, user_id).await;
    let recall_sid = derive_session_id(&recall_session_id, user_id);
    let recall_executor = recall_session_manager
        .session_registry()
        .get(&recall_sid)
        .await
        .expect("recall session should exist in registry");
    assert!(
        recall_executor.read().await.has_persistent_memory(),
        "recall executor missing durable persistent-memory wiring"
    );
    let recall_task_id = create_task_http_with_body(
        &client,
        &recall_base_url,
        &recall_session_id,
        "Какую точную постоянную заметку я просил запомнить раньше? Ответь только самой строкой.",
    )
    .await;
    let recall_status = wait_for_terminal_task_status(
        recall_session_manager.as_ref(),
        &recall_task_id,
        Duration::from_secs(240),
    )
    .await;
    let recall_progress = fetch_task_progress(
        &client,
        &recall_base_url,
        &recall_session_id,
        &recall_task_id,
    )
    .await
    .json::<Value>()
    .await
    .expect("failed to decode recall task progress");
    let recall_response =
        latest_assistant_response(recall_session_manager.as_ref(), &recall_session_id, user_id)
            .await
            .unwrap_or_default();

    cleanup_live_attempt(&client, &recall_base_url, &recall_session_id, user_id).await;
    recall_server.abort();

    assert!(
        matches!(recall_status, TaskStatus::Completed),
        "recall task failed: {:?}; progress={recall_progress:?}",
        recall_status
    );
    assert!(
        recall_response.contains(&token),
        "recalled response did not contain durable token; token={token}; response={recall_response:?}; progress={recall_progress:?}"
    );
}

#[tokio::test]
#[ignore = "Requires RUN_LLM_E2E_CHECKS=1, ZAI_API_KEY, MEMORY_DATABASE_URL, Postgres, Docker sandbox, and network access"]
async fn e2e_zai_postgres_memory_keeps_contexts_isolated_across_restart() {
    if std::env::var("RUN_LLM_E2E_CHECKS").as_deref() != Ok("1") {
        eprintln!("[LIVE-ZAI] Skipping cross-context Postgres durable memory test: RUN_LLM_E2E_CHECKS != 1");
        return;
    }

    let ((app_state, memory_store), user_id, context_a, context_b, token_a, token_b) = {
        let setup = match setup_live_zai_test_with_postgres().await {
            Ok(value) => value,
            Err(error) => {
                eprintln!(
                    "[LIVE-ZAI] Skipping cross-context Postgres durable memory test: {error}"
                );
                return;
            }
        };
        let user_id = unique_test_user_id();
        let context_a = format!("daily-work-{:x}", user_id);
        let context_b = format!("daily-home-{:x}", user_id);
        let token_a = format!("CTX_A_TOKEN_{user_id:x}_espresso");
        let token_b = format!("CTX_B_TOKEN_{user_id:x}_notion");
        (setup, user_id, context_a, context_b, token_a, token_b)
    };

    let session_manager = app_state.session_manager();
    let (server, base_url) = spawn_test_server(app_state).await;
    let client = reqwest::Client::new();

    let session_a =
        create_session_http_with_user_and_context(&client, &base_url, user_id, Some(&context_a))
            .await;
    let sid_a = derive_session_id(&session_a, user_id);
    let executor_a = session_manager
        .session_registry()
        .get(&sid_a)
        .await
        .expect("context-a session should exist in registry");
    assert!(
        executor_a.read().await.has_persistent_memory(),
        "context-a executor missing durable persistent-memory wiring"
    );
    let remember_a = format!(
        "Запомни для будущих разговоров в этой теме дословно: {token_a}. Это относится только к этой теме. Ответь коротко, что запомнил.",
    );
    let task_a = create_task_http_with_body(&client, &base_url, &session_a, &remember_a).await;
    let status_a =
        wait_for_terminal_task_status(session_manager.as_ref(), &task_a, Duration::from_secs(240))
            .await;
    let progress_a = fetch_task_progress(&client, &base_url, &session_a, &task_a)
        .await
        .json::<Value>()
        .await
        .expect("failed to decode context-a progress");
    assert!(
        matches!(status_a, TaskStatus::Completed),
        "context-a remember task failed: {:?}; progress={progress_a:?}",
        status_a
    );

    let session_b =
        create_session_http_with_user_and_context(&client, &base_url, user_id, Some(&context_b))
            .await;
    let sid_b = derive_session_id(&session_b, user_id);
    let executor_b = session_manager
        .session_registry()
        .get(&sid_b)
        .await
        .expect("context-b session should exist in registry");
    assert!(
        executor_b.read().await.has_persistent_memory(),
        "context-b executor missing durable persistent-memory wiring"
    );
    let remember_b = format!(
        "Запомни для будущих разговоров в этой теме дословно: {token_b}. Это относится только к этой теме. Ответь коротко, что запомнил.",
    );
    let task_b = create_task_http_with_body(&client, &base_url, &session_b, &remember_b).await;
    let status_b =
        wait_for_terminal_task_status(session_manager.as_ref(), &task_b, Duration::from_secs(240))
            .await;
    let progress_b = fetch_task_progress(&client, &base_url, &session_b, &task_b)
        .await
        .json::<Value>()
        .await
        .expect("failed to decode context-b progress");
    assert!(
        matches!(status_b, TaskStatus::Completed),
        "context-b remember task failed: {:?}; progress={progress_b:?}",
        status_b
    );

    assert!(
        wait_for_durable_memory_hit(
            memory_store.as_ref(),
            user_id,
            &context_a,
            &token_a,
            Duration::from_secs(30),
        )
        .await,
        "context-a durable memory token was not persisted: {token_a}"
    );
    assert!(
        wait_for_durable_memory_hit(
            memory_store.as_ref(),
            user_id,
            &context_b,
            &token_b,
            Duration::from_secs(30),
        )
        .await,
        "context-b durable memory token was not persisted: {token_b}"
    );
    assert!(
        !durable_memory_hit_exists(memory_store.as_ref(), user_id, &context_a, &token_b).await,
        "context-a unexpectedly contains context-b token in durable storage"
    );
    assert!(
        !durable_memory_hit_exists(memory_store.as_ref(), user_id, &context_b, &token_a).await,
        "context-b unexpectedly contains context-a token in durable storage"
    );

    cleanup_live_attempt(&client, &base_url, &session_a, user_id).await;
    cleanup_live_attempt(&client, &base_url, &session_b, user_id).await;
    server.abort();

    let (recall_state, recall_store) = setup_live_zai_test_with_postgres()
        .await
        .expect("second Postgres-backed app state should initialize");
    let recall_session_manager = recall_state.session_manager();
    let (recall_server, recall_base_url) = spawn_test_server(recall_state).await;

    let recall_session_a = create_session_http_with_user_and_context(
        &client,
        &recall_base_url,
        user_id,
        Some(&context_a),
    )
    .await;
    let recall_sid_a = derive_session_id(&recall_session_a, user_id);
    let recall_executor_a = recall_session_manager
        .session_registry()
        .get(&recall_sid_a)
        .await
        .expect("recall context-a session should exist in registry");
    assert!(
        recall_executor_a.read().await.has_persistent_memory(),
        "recall context-a executor missing durable persistent-memory wiring"
    );
    let recall_task_a = create_task_http_with_body(
        &client,
        &recall_base_url,
        &recall_session_a,
        "Какую точную постоянную заметку я просил запомнить раньше в этой теме? Ответь только самой строкой.",
    )
    .await;
    let recall_status_a = wait_for_terminal_task_status(
        recall_session_manager.as_ref(),
        &recall_task_a,
        Duration::from_secs(240),
    )
    .await;
    let recall_progress_a =
        fetch_task_progress(&client, &recall_base_url, &recall_session_a, &recall_task_a)
            .await
            .json::<Value>()
            .await
            .expect("failed to decode recall context-a progress");
    let recall_response_a =
        latest_assistant_response(recall_session_manager.as_ref(), &recall_session_a, user_id)
            .await
            .unwrap_or_default();

    let recall_session_b = create_session_http_with_user_and_context(
        &client,
        &recall_base_url,
        user_id,
        Some(&context_b),
    )
    .await;
    let recall_sid_b = derive_session_id(&recall_session_b, user_id);
    let recall_executor_b = recall_session_manager
        .session_registry()
        .get(&recall_sid_b)
        .await
        .expect("recall context-b session should exist in registry");
    assert!(
        recall_executor_b.read().await.has_persistent_memory(),
        "recall context-b executor missing durable persistent-memory wiring"
    );
    let recall_task_b = create_task_http_with_body(
        &client,
        &recall_base_url,
        &recall_session_b,
        "Какую точную постоянную заметку я просил запомнить раньше в этой теме? Ответь только самой строкой.",
    )
    .await;
    let recall_status_b = wait_for_terminal_task_status(
        recall_session_manager.as_ref(),
        &recall_task_b,
        Duration::from_secs(240),
    )
    .await;
    let recall_progress_b =
        fetch_task_progress(&client, &recall_base_url, &recall_session_b, &recall_task_b)
            .await
            .json::<Value>()
            .await
            .expect("failed to decode recall context-b progress");
    let recall_response_b =
        latest_assistant_response(recall_session_manager.as_ref(), &recall_session_b, user_id)
            .await
            .unwrap_or_default();

    cleanup_live_attempt(&client, &recall_base_url, &recall_session_a, user_id).await;
    cleanup_live_attempt(&client, &recall_base_url, &recall_session_b, user_id).await;
    recall_server.abort();

    assert!(
        matches!(recall_status_a, TaskStatus::Completed),
        "recall context-a task failed: {:?}; progress={recall_progress_a:?}",
        recall_status_a
    );
    assert!(
        matches!(recall_status_b, TaskStatus::Completed),
        "recall context-b task failed: {:?}; progress={recall_progress_b:?}",
        recall_status_b
    );
    assert!(
        recall_response_a.contains(&token_a),
        "context-a recall did not contain its token; token={token_a}; response={recall_response_a:?}; progress={recall_progress_a:?}"
    );
    assert!(
        !recall_response_a.contains(&token_b),
        "context-a recall leaked context-b token; token={token_b}; response={recall_response_a:?}; progress={recall_progress_a:?}"
    );
    assert!(
        recall_response_b.contains(&token_b),
        "context-b recall did not contain its token; token={token_b}; response={recall_response_b:?}; progress={recall_progress_b:?}"
    );
    assert!(
        !recall_response_b.contains(&token_a),
        "context-b recall leaked context-a token; token={token_a}; response={recall_response_b:?}; progress={recall_progress_b:?}"
    );
    assert!(
        durable_memory_hit_exists(recall_store.as_ref(), user_id, &context_a, &token_a).await,
        "restarted store lost context-a token"
    );
    assert!(
        durable_memory_hit_exists(recall_store.as_ref(), user_id, &context_b, &token_b).await,
        "restarted store lost context-b token"
    );
    assert!(
        !durable_memory_hit_exists(recall_store.as_ref(), user_id, &context_a, &token_b).await,
        "restarted store shows context-b token in context-a"
    );
    assert!(
        !durable_memory_hit_exists(recall_store.as_ref(), user_id, &context_b, &token_a).await,
        "restarted store shows context-a token in context-b"
    );
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
    log_compaction_probe(artifacts, event_names);
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

fn log_compaction_probe(artifacts: &LiveAuditArtifacts, event_names: &[String]) {
    let latest_snapshot = &artifacts.progress["latest_token_snapshot"];
    let budget_state = latest_snapshot["budget_state"].as_str();
    let headroom_tokens = latest_snapshot["headroom_tokens"].as_i64();
    let context_window_tokens = latest_snapshot["context_window_tokens"].as_i64();
    let projected_total_tokens = latest_snapshot["projected_total_tokens"].as_i64();
    let last_compaction_status = artifacts.progress["last_compaction_status"].as_str();
    let repeated_compaction_warning = artifacts.progress["repeated_compaction_warning"].as_str();

    let compaction_started = event_names
        .iter()
        .filter(|name| name.as_str() == "compaction_started")
        .count();
    let pruning_applied = event_names
        .iter()
        .filter(|name| name.as_str() == "pruning_applied")
        .count();
    let compaction_completed = event_names
        .iter()
        .filter(|name| name.as_str() == "compaction_completed")
        .count();
    let repeated_compaction = event_names
        .iter()
        .filter(|name| name.as_str() == "repeated_compaction_warning")
        .count();

    eprintln!("[LIVE-ZAI] Compaction probe:");
    eprintln!(
        "  - budget_state={:?}, headroom_tokens={:?}, projected_total_tokens={:?}, context_window_tokens={:?}",
        budget_state, headroom_tokens, projected_total_tokens, context_window_tokens
    );
    eprintln!(
        "  - last_compaction_status={:?}, repeated_compaction_warning={:?}",
        last_compaction_status, repeated_compaction_warning
    );
    eprintln!(
        "  - event_counts: started={}, pruning_applied={}, completed={}, repeated_warning={}",
        compaction_started, pruning_applied, compaction_completed, repeated_compaction
    );

    if latest_snapshot.is_null() {
        eprintln!(
            "[LIVE-ZAI][warn] progress.latest_token_snapshot missing; compaction/budget baseline is incomplete"
        );
    }
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
