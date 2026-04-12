#[cfg(feature = "searxng")]
mod searxng_tests {
    use oxide_agent_core::agent::provider::ToolProvider;
    use oxide_agent_core::agent::providers::SearxngProvider;

    #[test]
    fn searxng_can_handle_tool() {
        let provider = SearxngProvider::new("http://localhost:8080")
            .expect("provider should construct with valid base URL");

        assert!(provider.can_handle("searxng_search"));
        assert!(!provider.can_handle("web_search"));
    }

    #[test]
    fn searxng_tools_listed() {
        let provider = SearxngProvider::new("http://localhost:8080")
            .expect("provider should construct with valid base URL");
        let tools = provider.tools();

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "searxng_search");
    }
}
