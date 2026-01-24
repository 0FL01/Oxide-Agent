//! Agent thought inference from tool calls
//!
//! Generates human-readable descriptions of what the agent is doing
//! based on tool names and their arguments.

use lazy_regex::regex_replace_all;

/// Templates for generating thoughts from tool calls
const THOUGHT_TEMPLATES: &[(&str, &str)] = &[
    ("read_file", "Reading file {path}"),
    ("write_file", "Writing changes to {path}"),
    ("execute_command", "Executing command"),
    ("list_files", "Viewing directory contents {directory}"),
    ("tavily_search", "Searching for information: {query}"),
    ("tavily_extract", "Extracting content from {url}"),
    ("tavily_crawl", "Analyzing website structure {url}"),
    ("download_file", "Downloading file from {url}"),
    ("ytdlp_download", "Downloading video from {url}"),
    ("ytdlp_info", "Getting video information {url}"),
    ("upload_to_gofile", "Uploading file to filehosting"),
    ("write_todos", "Updating todo list"),
    ("complete_todo", "Marking todo as completed"),
];

/// Generate a human-readable thought from a tool call
#[must_use]
pub fn infer_thought(tool_name: &str, arguments: &str) -> Option<String> {
    // Find matching template
    let template = THOUGHT_TEMPLATES
        .iter()
        .find(|(name, _)| *name == tool_name)
        .map(|(_, t)| *t)?;

    // Parse arguments as JSON
    let args: serde_json::Value = serde_json::from_str(arguments).ok()?;

    // Replace placeholders
    let mut thought = template.to_string();

    // Handle {path} placeholder
    if thought.contains("{path}") {
        if let Some(path) = args.get("path").and_then(|v| v.as_str()) {
            let basename = std::path::Path::new(path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(path);
            thought = thought.replace("{path}", basename);
        } else {
            thought = thought.replace("{path}", "...");
        }
    }

    // Handle {directory} placeholder
    if thought.contains("{directory}") {
        if let Some(dir) = args.get("directory").and_then(|v| v.as_str()) {
            let short_dir = truncate_path(dir, 30);
            thought = thought.replace("{directory}", &short_dir);
        } else if let Some(dir) = args.get("path").and_then(|v| v.as_str()) {
            let short_dir = truncate_path(dir, 30);
            thought = thought.replace("{directory}", &short_dir);
        } else {
            thought = thought.replace("{directory}", "каталога");
        }
    }

    // Handle {query} placeholder
    if thought.contains("{query}") {
        if let Some(query) = args.get("query").and_then(|v| v.as_str()) {
            let short_query = crate::utils::truncate_str(query, 50);
            thought = thought.replace("{query}", &short_query);
        } else {
            thought = thought.replace("{query}", "...");
        }
    }

    // Handle {url} placeholder
    if thought.contains("{url}") {
        if let Some(url) = args.get("url").and_then(|v| v.as_str()) {
            let domain = extract_domain(url);
            thought = thought.replace("{url}", &domain);
        } else if let Some(urls) = args.get("urls").and_then(|v| v.as_array()) {
            if let Some(first_url) = urls.first().and_then(|v| v.as_str()) {
                let domain = extract_domain(first_url);
                thought = thought.replace("{url}", &domain);
            } else {
                thought = thought.replace("{url}", "...");
            }
        } else {
            thought = thought.replace("{url}", "...");
        }
    }

    Some(thought)
}

/// Generate a thought from command preview (for execute_command)
#[must_use]
pub fn infer_thought_from_command(command: &str) -> String {
    // Common command patterns and their descriptions
    let patterns: &[(&str, &str)] = &[
        ("cat ", "Viewing file content"),
        ("grep ", "Searching in files"),
        ("find ", "Searching for files"),
        ("ls ", "Viewing directory contents"),
        ("cd ", "Navigating to directory"),
        ("mkdir ", "Creating directory"),
        ("rm ", "Removing files"),
        ("cp ", "Copying files"),
        ("mv ", "Moving files"),
        ("curl ", "Downloading data from the network"),
        ("wget ", "Downloading file"),
        ("pip ", "Managing Python packages"),
        ("npm ", "Managing Node.js packages"),
        ("cargo ", "Working with Rust project"),
        ("git ", "Working with Git"),
        ("python ", "Running Python script"),
        ("node ", "Running Node.js script"),
        ("docker ", "Working with Docker"),
        ("ffmpeg ", "Processing media file"),
    ];

    let cmd_lower = command.to_lowercase();

    for (pattern, description) in patterns {
        if cmd_lower.starts_with(pattern) || cmd_lower.contains(&format!(" {pattern}")) {
            return description.to_string();
        }
    }

    // Default: just say "executing command"
    "Executing command".to_string()
}

/// Extract reasoning summary from full reasoning content
#[must_use]
pub fn extract_reasoning_summary(reasoning: &str, max_len: usize) -> String {
    // Clean up the reasoning text
    let cleaned = reasoning.trim();

    // Remove common prefixes like "I need to", "Let me", etc.
    let cleaned: String = regex_replace_all!(
        r"^(I need to|Let me|I will|I should|First,?|Now,?|Next,?)\s*",
        cleaned,
        ""
    )
    .into_owned();

    // Get first sentence or first N characters
    let first_sentence = cleaned
        .split(['.', '!', '?', '\n'])
        .next()
        .unwrap_or(&cleaned)
        .trim();

    if first_sentence.len() <= max_len {
        first_sentence.to_string()
    } else {
        format!(
            "{}...",
            &first_sentence.chars().take(max_len - 3).collect::<String>()
        )
    }
}

/// Extract domain from URL
fn extract_domain(url: &str) -> String {
    url.trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or(url)
        .to_string()
}

/// Truncate a path for display
fn truncate_path(path: &str, max_len: usize) -> String {
    if path.len() <= max_len {
        return path.to_string();
    }

    // Try to show the last part of the path
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return crate::utils::truncate_str(path, max_len).to_string();
    }

    // Show ".../" + last N components that fit
    let mut result = String::new();
    for part in parts.iter().rev() {
        let candidate = if result.is_empty() {
            part.to_string()
        } else {
            format!("{part}/{result}")
        };

        if candidate.len() + 4 > max_len {
            // ".../" prefix
            break;
        }
        result = candidate;
    }

    if result.len() < path.len() {
        format!(".../{result}")
    } else {
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_infer_thought_read_file() {
        let thought = infer_thought("read_file", r#"{"path": "/workspace/src/main.rs"}"#);
        assert_eq!(thought, Some("Reading file main.rs".to_string()));
    }

    #[test]
    fn test_infer_thought_tavily_search() {
        let thought = infer_thought("tavily_search", r#"{"query": "rust async programming"}"#);
        assert_eq!(
            thought,
            Some("Searching for information: rust async programming".to_string())
        );
    }

    #[test]
    fn test_infer_thought_from_command() {
        assert_eq!(
            infer_thought_from_command("cat /etc/hosts"),
            "Viewing file content"
        );
        assert_eq!(
            infer_thought_from_command("cargo build --release"),
            "Working with Rust project"
        );
    }

    #[test]
    fn test_extract_reasoning_summary() {
        let reasoning = "I need to analyze the file structure first. Then I will look for hooks.";
        let summary = extract_reasoning_summary(reasoning, 50);
        assert_eq!(summary, "analyze the file structure first");
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(extract_domain("https://docs.rs/tokio/latest"), "docs.rs");
        assert_eq!(
            extract_domain("http://example.com/path/to/file"),
            "example.com"
        );
    }

    #[test]
    fn test_truncate_path() {
        assert_eq!(truncate_path("/a/b/c", 30), "/a/b/c");
        // Path gets truncated to fit within 20 chars
        let result = truncate_path("/very/long/path/to/some/file.rs", 20);
        assert!(result.starts_with("..."));
        assert!(result.len() <= 24); // Allow some flexibility
    }
}
