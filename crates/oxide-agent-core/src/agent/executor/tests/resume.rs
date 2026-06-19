use super::*;

fn image_attachment() -> crate::agent::AgentMessageAttachment {
    crate::agent::AgentMessageAttachment::image(
        "screen.png",
        Some("image/png".to_string()),
        42,
        "/workspace/uploads/screen.png",
    )
}

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
    assert!(pending[0].attachments.is_empty());
}

#[test]
fn resume_with_agent_user_input_queues_attachment_refs() {
    let mut executor = build_executor();
    executor
        .session_mut()
        .set_pending_user_input(PendingUserInput {
            kind: crate::agent::UserInputKind::File,
            prompt: "Attach a screenshot".to_string(),
        });
    let attachment = image_attachment();
    let input = AgentUserInput::new("See attached").with_attachments(vec![attachment.clone()]);

    assert!(executor.resume_with_agent_user_input(input));
    assert!(executor.session().pending_user_input().is_none());

    let pending = executor.session().drain_runtime_context();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].content, "See attached");
    assert_eq!(pending[0].attachments, [attachment]);
}

#[test]
fn resume_with_user_input_is_noop_without_pending_request() {
    let mut executor = build_executor();

    assert!(!executor.resume_with_user_input("ignored".to_string()));
    assert!(executor.session().drain_runtime_context().is_empty());
}

#[cfg(feature = "llm-opencode-go")]
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

#[cfg(feature = "llm-opencode-go")]
#[tokio::test]
async fn execute_user_input_with_options_persists_user_task_attachment_refs() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"saw attachment","awaiting_user_input":null}"#,
    );
    let attachment = image_attachment();
    let input = AgentUserInput::new("What is shown?").with_attachments(vec![attachment.clone()]);

    let result = executor
        .execute_user_input_with_options(
            input,
            None,
            crate::agent::AgentExecutionOptions::default(),
        )
        .await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "saw attachment"
    ));
    let user_task = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .find(|message| message.kind == crate::agent::compaction::AgentMessageKind::UserTask)
        .expect("user task should be persisted");
    assert_eq!(user_task.text_projection(), "What is shown?");
    assert_eq!(user_task.user_attachments(), &[attachment]);
}

#[cfg(feature = "llm-opencode-go")]
#[tokio::test]
async fn resume_user_input_with_options_persists_runtime_attachment_refs() {
    let mut executor = build_executor_with_mock_response(
        r#"{"thought":"done","tool_call":null,"final_answer":"resumed with attachment","awaiting_user_input":null}"#,
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
            kind: UserInputKind::File,
            prompt: "Attach a screenshot".to_string(),
        });
    let attachment = image_attachment();
    let input =
        AgentUserInput::new("Screenshot attached").with_attachments(vec![attachment.clone()]);

    let result = executor
        .resume_user_input_with_options(input, None, crate::agent::AgentExecutionOptions::default())
        .await;

    assert!(matches!(
        result,
        Ok(crate::agent::executor::AgentExecutionOutcome::Completed(ref answer)) if answer == "resumed with attachment"
    ));
    let runtime_context = executor
        .session()
        .memory
        .get_messages()
        .iter()
        .find(|message| {
            message.kind == crate::agent::compaction::AgentMessageKind::RuntimeContext
                && message.content == "Screenshot attached"
        })
        .expect("runtime context should be persisted");
    assert_eq!(runtime_context.user_attachments(), &[attachment]);
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

#[cfg(feature = "llm-opencode-go")]
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
