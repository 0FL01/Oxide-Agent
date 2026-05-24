#[cfg(feature = "tool-browser-use")]
mod browser_use_tests {
    use oxide_agent_core::agent::providers::BrowserUseProvider;
    use std::collections::HashSet;
    use std::sync::Arc;

    #[test]
    fn browser_use_typed_runtime_registers_expected_tools() {
        let provider = Arc::new(BrowserUseProvider::new(
            "http://localhost:8002",
            Arc::new(oxide_agent_core::config::AgentSettings::default()),
        ));
        let names: HashSet<String> = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.name().into_inner())
            .collect();

        assert_eq!(names.len(), 5);
        assert!(names.contains("browser_use_run_task"));
        assert!(names.contains("browser_use_get_session"));
        assert!(names.contains("browser_use_close_session"));
        assert!(names.contains("browser_use_extract_content"));
        assert!(names.contains("browser_use_screenshot"));
        assert!(!names.contains("web_search"));
    }

    #[test]
    fn browser_use_typed_runtime_specs_list_expected_tools() {
        let provider = Arc::new(BrowserUseProvider::new(
            "http://localhost:8002",
            Arc::new(oxide_agent_core::config::AgentSettings::default()),
        ));
        let tools = provider
            .tool_runtime_executors()
            .into_iter()
            .map(|executor| executor.spec())
            .collect::<Vec<_>>();
        assert_eq!(tools.len(), 5);

        let names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains("browser_use_run_task"));
        assert!(names.contains("browser_use_get_session"));
        assert!(names.contains("browser_use_close_session"));
        assert!(names.contains("browser_use_extract_content"));
        assert!(names.contains("browser_use_screenshot"));
    }
}
