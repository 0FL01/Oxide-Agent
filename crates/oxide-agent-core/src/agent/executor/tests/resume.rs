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
