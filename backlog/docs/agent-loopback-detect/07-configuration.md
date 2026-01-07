# Configuration Guide

## Model Configuration

### Required Models

For full functionality, you need two AI models:

```rust
pub struct LoopDetectionModels {
    /// Fast model for initial scan (low cost, fast)
    pub flash_model: String,

    /// Pro model for confirmation (high quality)
    pub pro_model: String,
}

impl Default for LoopDetectionModels {
    fn default() -> Self {
        Self {
            flash_model: "gemini-2.5-flash".to_string(),
            pro_model: "gemini-2.5-pro".to_string(),
        }
    }
}
```

### Alternative Models

If using OpenAI:

```rust
impl LoopDetectionModels {
    pub fn openai() -> Self {
        Self {
            flash_model: "gpt-4o-mini".to_string(),
            pro_model: "gpt-4o".to_string(),
        }
    }

    pub fn anthropic() -> Self {
        Self {
            flash_model: "claude-3-5-haiku".to_string(),
            pro_model: "claude-3-5-sonnet".to_string(),
        }
    }
}
```

## Threshold Configuration

```rust
#[derive(Clone)]
pub struct LoopDetectionThresholds {
    /// Number of identical tool calls to trigger detection
    pub tool_call_threshold: usize,

    /// Number of chunk repetitions to trigger detection
    pub content_loop_threshold: usize,

    /// Confidence threshold for LLM (0.0 - 1.0)
    pub llm_confidence_threshold: f64,
}

impl Default for LoopDetectionThresholds {
    fn default() -> Self {
        Self {
            tool_call_threshold: 5,
            content_loop_threshold: 10,
            llm_confidence_threshold: 0.9,
        }
    }
}
```

## Timing Configuration

```rust
pub struct LoopDetectionTiming {
    /// Minimum turns before LLM checks start
    pub min_turns_before_llm_check: usize,

    /// History size for LLM context
    pub llm_history_count: usize,

    /// Minimum check interval (turns)
    pub min_llm_check_interval: usize,

    /// Maximum check interval (turns)
    pub max_llm_check_interval: usize,
}

impl Default for LoopDetectionTiming {
    fn default() -> Self {
        Self {
            min_turns_before_llm_check: 30,
            llm_history_count: 20,
            min_llm_check_interval: 5,
            max_llm_check_interval: 15,
        }
    }
}
```

## Content Analysis Configuration

```rust
pub struct ContentAnalysisConfig {
    /// Size of chunks for analysis (characters)
    pub chunk_size: usize,

    /// Maximum content history length
    pub max_history_length: usize,

    /// Multiplier for max distance between repetitions
    /// chunks must appear within chunk_size * this value
    pub max_distance_multiplier: usize,
}

impl Default for ContentAnalysisConfig {
    fn default() -> Self {
        Self {
            chunk_size: 50,
            max_history_length: 5000,
            max_distance_multiplier: 5,
        }
    }
}
```

## Complete Configuration

```rust
#[derive(Clone)]
pub struct LoopDetectionConfig {
    pub models: LoopDetectionModels,
    pub thresholds: LoopDetectionThresholds,
    pub timing: LoopDetectionTiming,
    pub content: ContentAnalysisConfig,
    pub enabled: bool,
}

impl Default for LoopDetectionConfig {
    fn default() -> Self {
        Self {
            models: LoopDetectionModels::default(),
            thresholds: LoopDetectionThresholds::default(),
            timing: LoopDetectionTiming::default(),
            content: ContentAnalysisConfig::default(),
            enabled: true,
        }
    }
}
```

## Environment Variables

```rust
impl LoopDetectionConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("LOOP_DETECTION_ENABLED") {
            config.enabled = val.parse().unwrap_or(true);
        }

        if let Ok(val) = std::env::var("LOOP_DETECTION_TOOL_THRESHOLD") {
            config.thresholds.tool_call_threshold = val.parse().unwrap_or(5);
        }

        if let Ok(val) = std::env::var("LOOP_DETECTION_CONTENT_THRESHOLD") {
            config.thresholds.content_loop_threshold = val.parse().unwrap_or(10);
        }

        if let Ok(val) = std::env::var("LOOP_DETECTION_LLM_CONFIDENCE") {
            config.thresholds.llm_confidence_threshold = val.parse().unwrap_or(0.9);
        }

        if let Ok(val) = std::env::var("LOOP_DETECTION_FLASH_MODEL") {
            config.models.flash_model = val;
        }

        if let Ok(val) = std::env::var("LOOP_DETECTION_PRO_MODEL") {
            config.models.pro_model = val;
        }

        config
    }
}
```

## Configuration File (TOML)

```toml
[loop_detection]
enabled = true

[loop_detection.models]
flash_model = "gemini-2.5-flash"
pro_model = "gemini-2.5-pro"

[loop_detection.thresholds]
tool_call_threshold = 5
content_loop_threshold = 10
llm_confidence_threshold = 0.9

[loop_detection.timing]
min_turns_before_llm_check = 30
llm_history_count = 20
min_llm_check_interval = 5
max_llm_check_interval = 15

[loop_detection.content]
chunk_size = 50
max_history_length = 5000
max_distance_multiplier = 5
```

## Using Configuration

```rust
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
struct ConfigFile {
    loop_detection: LoopDetectionConfig,
}

fn load_config(path: &str) -> Result<LoopDetectionConfig, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(path)?;
    let config_file: ConfigFile = toml::from_str(&content)?;
    Ok(config_file.loop_detection)
}

// Usage
let config = load_config("config.toml")
    .unwrap_or_else(|_| LoopDetectionConfig::from_env());

let service = LoopDetectionService::new(Arc::new(config));
```

## Telegram-Specific Configuration

```rust
#[derive(Clone)]
pub struct TelegramLoopDetectionConfig {
    pub base: LoopDetectionConfig,
    pub reply_message_template: String,
    pub max_retries: usize,
}

impl Default for TelegramLoopDetectionConfig {
    fn default() -> Self {
        Self {
            base: LoopDetectionConfig::default(),
            reply_message_template: "I detected I'm stuck in a loop. Let me try a different approach.".to_string(),
            max_retries: 2,
        }
    }
}
```

## Disabling via Commands

```rust
impl LoopDetectionService {
    pub fn handle_disable_command(&mut self, user_id: u64) -> bool {
        if self.is_authorized(user_id) {
            self.disable_for_session();
            true
        } else {
            false
        }
    }

    fn is_authorized(&self, user_id: u64) -> bool {
        // Check if user is admin
        user_id == self.config.admin_user_id
    }
}
```
