use super::*;

#[test]
fn resume_with_user_input_clears_pending_request_and_queues_context() {
    let mut executor = build_executor();
    executor
        .session_mut()
        .set_pending_user_input(PendingUserInput {
            kind: crate::agent::UserInputKind::Text,
            prompt: "Reply with details".to_string(),
        });

    assert!(executor.resume_with_user_input("Here are the details".to_string()));
    assert!(executor.session().pending_user_input().is_none());

    let pending = executor.session().drain_runtime_context();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "Here are the details");
}

#[test]
fn resume_with_user_input_is_noop_without_pending_request() {
    let mut executor = build_executor();

    assert!(!executor.resume_with_user_input("ignored".to_string()));
    assert!(executor.session().drain_runtime_context().is_empty());
}

#[tokio::test]
async fn resume_after_user_input_continues_saved_task_without_new_user_task() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"resumed ok","awaiting_user_input":null}"#,
    );
    executor.session_mut().remember_task("original task");
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user_task(
            "original task",
        ));
    executor
        .session_mut()
        .set_pending_user_input(PendingUserInput {
            kind: UserInputKind::Text,
            prompt: "Need more details".to_string(),
        });

    let result = executor
        .resume_after_user_input("extra details".to_string(), None)
        .await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "resumed ok"
    ));
    assert!(executor.session().pending_user_input().is_none());

    let user_task_count = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .filter(|message| message.kind == crate::agent::compaction::AgentMessageKind::UserTask)
        .count();
    assert_eq!(user_task_count, 1);

    let runtime_context = executor.session().drain_runtime_context();
    assert!(runtime_context.is_empty());
}

#[tokio::test]
async fn resume_after_user_input_rejects_sessions_without_pending_request() {
    let mut executor = build_executor();
    executor.session_mut().remember_task("original task");

    let error = match executor
        .resume_after_user_input("extra details".to_string(), None)
        .await
    {
        Ok(_) => panic!("resume should fail without pending request"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("not waiting for user input"));
}

#[tokio::test]
async fn continue_after_runtime_context_continues_saved_task_without_new_user_task() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"continued ok","awaiting_user_input":null}"#,
    );
    executor.session_mut().remember_task("original task");
    executor
        .session_mut()
        .memory
        .add_message(crate::agent::memory::AgentMessage::user_task(
            "original task",
        ));
    executor.enqueue_runtime_context("new clarification".to_string());

    let result = executor.continue_after_runtime_context(None).await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "continued ok"
    ));

    let user_task_count = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .filter(|message| message.kind == crate::agent::compaction::AgentMessageKind::UserTask)
        .count();
    assert_eq!(user_task_count, 1);

    let runtime_context = executor.session().drain_runtime_context();
    assert!(runtime_context.is_empty());
}

#[tokio::test]
async fn continue_after_runtime_context_rejects_sessions_without_queued_context() {
    let mut executor = build_executor();
    executor.session_mut().remember_task("original task");

    let error = match executor.continue_after_runtime_context(None).await {
        Ok(_) => panic!("continuation should fail without queued runtime context"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("no queued runtime context"));
}

#[tokio::test]
async fn resume_ssh_approval_rejects_sessions_without_saved_task() {
    let mut executor = build_executor();

    let error = match executor.resume_ssh_approval("req-1", None).await {
        Ok(_) => panic!("resume should fail without a saved task"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("no saved task to resume"));
}

#[tokio::test]
async fn resume_ssh_approval_rejects_missing_replay_payload_after_grant() {
    let mut executor = build_executor();
    executor.session_mut().remember_task("original task");
    executor.set_topic_infra(
        Arc::new(MockStorageProvider::new()),
        9,
        "topic-a".to_string(),
        Some(crate::storage::TopicInfraConfigRecord {
            schema_version: 1,
            version: 1,
            user_id: 9,
            topic_id: "topic-a".to_string(),
            target_name: "stage".to_string(),
            host: "stage.example.com".to_string(),
            port: 22,
            remote_user: "root".to_string(),
            auth_mode: crate::storage::TopicInfraAuthMode::None,
            secret_ref: None,
            sudo_secret_ref: None,
            environment: None,
            tags: Vec::new(),
            allowed_tool_modes: Vec::new(),
            approval_required_modes: Vec::new(),
            created_at: 0,
            updated_at: 0,
        }),
    );

    let request = executor
        .topic_infra
        .as_ref()
        .expect("topic infra should be attached")
        .approvals
        .register(
            "ssh_exec",
            "topic-a",
            "stage",
            "Run uptime".to_string(),
            "fp-1".to_string(),
        )
        .await;

    let error = match executor
        .resume_ssh_approval(&request.request_id, None)
        .await
    {
        Ok(_) => panic!("resume should fail without a replay payload"),
        Err(error) => error,
    };

    assert!(error
        .to_string()
        .contains("pending SSH replay payload not found"));
}
