// oxide-agent-core/src/agent/examples/opencode_integration.rs
//
// Examples of using Opencode integration in practice
//
// This file contains practical examples of how to use the Opencode
// integration in your agent application.

use crate::agent::registry::ToolRegistry;
use crate::agent::session::AgentSession;
use tokio::sync::mpsc;

/// Example 1: Simple Opencode task execution
///
/// This example shows how to execute a simple task through Opencode.
pub async fn example_simple_opencode_task() {
    println!("Example 1: Simple Opencode Task");
    println!("================================\n");

    // Create session with Opencode integration
    let session = AgentSession::new(
        "user-123".to_string(),
        Some("http://127.0.0.1:4096".to_string()),
    );

    // Check Opencode health
    match session.check_opencode_health().await {
        Ok(_) => println!("✓ Opencode server is healthy"),
        Err(e) => {
            eprintln!("✗ Opencode health check failed: {}", e);
            return;
        }
    }

    // Create progress channel
    let (progress_tx, mut progress_rx) = mpsc::channel(100);

    // Execute task
    let task = r#"{"task": "list files in current directory"}"#;

    println!("Executing task: {}", task);

    let result = session
        .registry
        .execute("opencode", task, &progress_tx, None)
        .await;

    match result {
        Ok(output) => {
            println!("✓ Task completed successfully");
            println!("Output:\n{}", output);
        }
        Err(e) => {
            eprintln!("✗ Task failed: {}", e);
        }
    }

    // Print progress events
    println!("\nProgress Events:");
    while let Some(event) = progress_rx.recv().await {
        match event {
            crate::agent::registry::AgentEvent::ToolCall { name, input, .. } => {
                println!("  → Tool call: {} ({})", name, input);
            }
            crate::agent::registry::AgentEvent::ToolResult { name, output } => {
                println!("  ← Tool result: {} ({} chars)", name, output.len());
            }
            _ => {}
        }
    }
}

/// Example 2: Feature implementation with Opencode
///
/// This example shows how to implement a new feature through Opencode.
pub async fn example_implement_feature() {
    println!("\nExample 2: Implement Feature");
    println!("==============================\n");

    let session = AgentSession::new(
        "user-456".to_string(),
        Some("http://127.0.0.1:4096".to_string()),
    );

    let (progress_tx, _progress_rx) = mpsc::channel(100);

    // Task: Add request logging
    let task = r#"{"task": "add request logging for all API endpoints"}"#;

    println!("Executing task: {}", task);

    match session
        .registry
        .execute("opencode", task, &progress_tx, None)
        .await
    {
        Ok(output) => {
            println!("✓ Feature implemented");
            println!("Output:\n{}", output);
        }
        Err(e) => {
            eprintln!("✗ Feature implementation failed: {}", e);
        }
    }
}

/// Example 3: Bug fix with Opencode
///
/// This example shows how to fix a bug through Opencode.
pub async fn example_fix_bug() {
    println!("\nExample 3: Fix Bug");
    println!("=================\n");

    let session = AgentSession::new(
        "user-789".to_string(),
        Some("http://127.0.0.1:4096".to_string()),
    );

    let (progress_tx, _progress_rx) = mpsc::channel(100);

    // Task: Fix login 500 error
    let task = r#"{"task": "investigate and fix the 500 error on login endpoint"}"#;

    println!("Executing task: {}", task);

    match session
        .registry
        .execute("opencode", task, &progress_tx, None)
        .await
    {
        Ok(output) => {
            println!("✓ Bug fixed");
            println!("Output:\n{}", output);
        }
        Err(e) => {
            eprintln!("✗ Bug fix failed: {}", e);
        }
    }
}

/// Example 4: Multi-step workflow with both Sandbox and Opencode
///
/// This example shows a complex workflow that uses both sandbox and Opencode.
pub async fn example_multi_step_workflow() {
    println!("\nExample 4: Multi-step Workflow");
    println!("=============================\n");

    let session = AgentSession::new(
        "user-abc".to_string(),
        Some("http://127.0.0.1:4096".to_string()),
    );

    let (progress_tx, _progress_rx) = mpsc::channel(100);

    // Step 1: Download video in sandbox
    println!("Step 1: Downloading video in sandbox...");
    let download_cmd = r#"yt-dlp -f best https://youtube.com/watch?v=xxx -o video.mp4"#;

    match session
        .registry
        .execute("execute_command", download_cmd, &progress_tx, None)
        .await
    {
        Ok(_) => println!("✓ Video downloaded"),
        Err(e) => {
            eprintln!("✗ Download failed: {}", e);
            return;
        }
    }

    // Step 2: Extract audio in sandbox
    println!("\nStep 2: Extracting audio in sandbox...");
    let extract_cmd = r#"ffmpeg -i video.mp4 -vn -acodec libmp3lame -q:a 2 audio.mp3"#;

    match session
        .registry
        .execute("execute_command", extract_cmd, &progress_tx, None)
        .await
    {
        Ok(_) => println!("✓ Audio extracted"),
        Err(e) => {
            eprintln!("✗ Extraction failed: {}", e);
            return;
        }
    }

    // Step 3: Add audio upload API via Opencode
    println!("\nStep 3: Adding audio upload API via Opencode...");
    let api_task = r#"{"task": "add audio upload API endpoint with validation"}"#;

    match session
        .registry
        .execute("opencode", api_task, &progress_tx, None)
        .await
    {
        Ok(_) => println!("✓ API endpoint added"),
        Err(e) => {
            eprintln!("✗ API addition failed: {}", e);
            return;
        }
    }

    println!("\n✓ Multi-step workflow completed!");
}

/// Example 5: Error handling
///
/// This example shows how to handle errors properly.
pub async fn example_error_handling() {
    println!("\nExample 5: Error Handling");
    println!("=======================\n");

    let session = AgentSession::new(
        "user-xyz".to_string(),
        Some("http://127.0.0.1:4096".to_string()),
    );

    let (progress_tx, _progress_rx) = mpsc::channel(100);

    // Try to execute with invalid arguments
    let invalid_task = r#"{}"#; // Missing "task" field

    println!("Executing invalid task...");

    match session
        .registry
        .execute("opencode", invalid_task, &progress_tx, None)
        .await
    {
        Ok(_) => {
            println!("✗ Should have failed with invalid arguments");
        }
        Err(e) => {
            println!("✓ Error properly handled: {}", e);
        }
    }

    // Try to execute with unknown tool
    println!("\nExecuting unknown tool...");

    match session
        .registry
        .execute("unknown_tool", "{}", &progress_tx, None)
        .await
    {
        Ok(_) => {
            println!("✗ Should have failed with unknown tool");
        }
        Err(e) => {
            println!("✓ Error properly handled: {}", e);
        }
    }
}

/// Example 6: Progress monitoring
///
/// This example shows how to monitor progress events in real-time.
pub async fn example_progress_monitoring() {
    println!("\nExample 6: Progress Monitoring");
    println!("=============================\n");

    let session = AgentSession::new(
        "user-123".to_string(),
        Some("http://127.0.0.1:4096".to_string()),
    );

    let (progress_tx, mut progress_rx) = mpsc::channel(100);

    // Spawn progress monitor task
    let monitor_task = tokio::spawn(async move {
        println!("Progress Monitor Started\n");
        while let Some(event) = progress_rx.recv().await {
            match event {
                crate::agent::registry::AgentEvent::ToolCall {
                    name,
                    input,
                    command_preview,
                } => {
                    println!("🔧 Calling tool: {}", name);
                    if let Some(preview) = command_preview {
                        println!("   Preview: {}", preview);
                    }
                }
                crate::agent::registry::AgentEvent::ToolResult { name, output } => {
                    println!("✅ Tool completed: {}", name);
                    println!("   Output: {} chars", output.len());
                }
                _ => {}
            }
        }
        println!("\nProgress Monitor Stopped");
    });

    // Execute a complex task
    let task = r#"{"task": "refactor the authentication module to use JWT tokens"}"#;

    println!("Executing task: {}", task);

    match session
        .registry
        .execute("opencode", task, &progress_tx, None)
        .await
    {
        Ok(output) => {
            println!("\n✓ Task completed");
            println!("Output:\n{}", output);
        }
        Err(e) => {
            eprintln!("\n✗ Task failed: {}", e);
        }
    }

    // Wait for monitor task to finish
    monitor_task.await.ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_example_simple_task() {
        example_simple_opencode_task().await;
    }

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_example_feature_implementation() {
        example_implement_feature().await;
    }

    #[tokio::test]
    #[ignore = "Requires running Opencode server"]
    async fn test_example_bug_fix() {
        example_fix_bug().await;
    }

    #[tokio::test]
    #[ignore = "Requires running Opencode server and yt-dlp"]
    async fn test_example_multi_step_workflow() {
        example_multi_step_workflow().await;
    }

    #[tokio::test]
    async fn test_example_error_handling() {
        example_error_handling().await;
    }
}

/// Main function to run all examples
#[allow(dead_code)]
pub async fn run_all_examples() {
    println!("Running All Examples");
    println!("====================\n");

    example_simple_opencode_task().await;
    example_implement_feature().await;
    example_fix_bug().await;
    example_multi_step_workflow().await;
    example_error_handling().await;
    example_progress_monitoring().await;

    println!("\nAll examples completed!");
}
