#[cfg(feature = "browser_use")]
mod browser_use_tests {
    use oxide_agent_core::agent::provider::ToolProvider;
    use oxide_agent_core::agent::providers::BrowserUseProvider;
    use std::collections::HashSet;

    #[test]
    fn browser_use_can_handle_tools() {
        let provider = BrowserUseProvider::new(
            "http://localhost:8002",
            std::sync::Arc::new(oxide_agent_core::config::AgentSettings::default()),
        );
        assert!(provider.can_handle("browser_use_run_task"));
        assert!(provider.can_handle("browser_use_get_session"));
        assert!(provider.can_handle("browser_use_close_session"));
        assert!(!provider.can_handle("web_search"));
    }

    #[test]
    fn browser_use_tools_listed() {
        let provider = BrowserUseProvider::new(
            "http://localhost:8002",
            std::sync::Arc::new(oxide_agent_core::config::AgentSettings::default()),
        );
        let tools = provider.tools();
        assert_eq!(tools.len(), 3);

        let names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains("browser_use_run_task"));
        assert!(names.contains("browser_use_get_session"));
        assert!(names.contains("browser_use_close_session"));
    }
}
