#[cfg(feature = "crawl4ai")]
mod crawl4ai_tests {
    use oxide_agent::agent::provider::ToolProvider;
    use oxide_agent::agent::providers::Crawl4aiProvider;
    use std::collections::HashSet;

    #[test]
    fn crawl4ai_can_handle_tools() {
        let provider = Crawl4aiProvider::new("http://localhost:11235");
        assert!(provider.can_handle("deep_crawl"));
        assert!(provider.can_handle("web_markdown"));
        assert!(provider.can_handle("web_pdf"));
        assert!(!provider.can_handle("web_search"));
    }

    #[test]
    fn crawl4ai_tools_listed() {
        let provider = Crawl4aiProvider::new("http://localhost:11235");
        let tools = provider.tools();
        assert_eq!(tools.len(), 3);

        let names: HashSet<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
        assert!(names.contains("deep_crawl"));
        assert!(names.contains("web_markdown"));
        assert!(names.contains("web_pdf"));
    }
}
