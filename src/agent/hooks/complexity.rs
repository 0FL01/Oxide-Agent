//! Complexity analyzer hook.
//!
//! Adds a system hint to consider delegating heavy work to a sub-agent.

use super::registry::Hook;
use super::types::{HookContext, HookEvent, HookResult};

/// Hook that suggests delegation for complex prompts.
pub struct ComplexityAnalyzerHook {
    min_word_count: usize,
}

impl ComplexityAnalyzerHook {
    /// Create a new complexity analyzer hook with default thresholds.
    #[must_use]
    pub const fn new() -> Self {
        Self { min_word_count: 70 }
    }

    fn is_complex_prompt(&self, prompt: &str) -> bool {
        let normalized = prompt.to_lowercase();
        let word_count = normalized.split_whitespace().count();
        if word_count >= self.min_word_count {
            return true;
        }

        let keywords = [
            "исслед",
            "сравн",
            "обзор",
            "анализ",
            "отчет",
            "подбор",
            "вариант",
            "тренд",
            "рынок",
            "свод",
            "таблиц",
            "compare",
            "research",
            "analysis",
            "overview",
            "report",
            "benchmark",
            "market",
            "git",
            "clone",
            "repo",
            "репозитор",
            "код",
            "файлы",
            "сканир",
            "codebase",
            "files",
            "изучи",
        ];

        if keywords.iter().any(|keyword| normalized.contains(keyword)) {
            return true;
        }

        let sentence_markers = ["?", "!", "."];
        let sentence_hits: usize = sentence_markers
            .iter()
            .map(|marker| normalized.matches(marker).count())
            .sum();

        sentence_hits >= 3
    }
}

impl Default for ComplexityAnalyzerHook {
    fn default() -> Self {
        Self::new()
    }
}

impl Hook for ComplexityAnalyzerHook {
    fn name(&self) -> &'static str {
        "complexity_analyzer"
    }

    fn handle(&self, event: &HookEvent, _context: &HookContext) -> HookResult {
        let HookEvent::BeforeAgent { prompt } = event else {
            return HookResult::Continue;
        };

        if !self.is_complex_prompt(prompt) {
            return HookResult::Continue;
        }

        HookResult::InjectContext(
            "[СИСТЕМА: Похоже, задача объемная или исследовательская. \n\
Если нужна черновая работа (поиск, сбор данных, чтение длинных материалов), \n\
рассмотри делегирование через tool `delegate_to_sub_agent` с четкой постановкой \n\
задачи и списком разрешенных инструментов.]"
                .to_string(),
        )
    }
}
