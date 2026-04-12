# Content Loop Detection

## Parameters

```rust
const CONTENT_CHUNK_SIZE: usize = 50;
const CONTENT_LOOP_THRESHOLD: usize = 10;
const MAX_HISTORY_LENGTH: usize = 5000;
const MAX_ALLOWED_DISTANCE_MULTIPLIER: usize = 5;
```

## Algorithm Overview

1. Sliding window over streaming content
2. Extract fixed-size chunks (50 chars)
3. Hash each chunk (SHA256)
4. Track positions where chunks appear
5. Detect loops when chunks repeat frequently

## State Structures

```rust
struct ContentTracker {
    history: String,                          // max 5000 chars
    content_stats: HashMap<String, Vec<usize>>, // hash → positions
    last_index: usize,                        // current position
    in_code_block: bool,
}
```

## Step-by-Step Processing

### 1. Content Preprocessing

Skip tracking when encountering:

- Code fences (```)
- Tables (`|...|`, `|+--+`)
- List items (`*`, `-`, `1.`)
- Headings (`#...`)
- Blockquotes (`>...`)
- Dividers (`===`, `---`, `===`)

```rust
if has_code_fence || has_table || has_list ||
   has_heading || has_blockquote || is_divider {
    reset_content_tracking();
    return false;
}
```

### 2. Code Block Detection

````rust
num_fences = content.matches("```").count();
in_code_block = if num_fences % 2 == 0 {
    in_code_block
} else {
    !in_code_block
};

if was_in_code_block || in_code_block {
    return false;  // Skip detection inside code blocks
}
````

### 3. History Management

Append new content:

```rust
history.push_str(content);
```

Truncate if needed (adjust all indices):

```rust
if history.len() > MAX_HISTORY_LENGTH {
    truncate_amount = history.len() - MAX_HISTORY_LENGTH;
    history = history[truncate_amount..].to_string();

    // Adjust all stored positions
    for positions in content_stats.values_mut() {
        positions.retain(|&mut pos| {
            pos = pos.saturating_sub(truncate_amount);
            pos > 0
        });
    }
}
```

### 4. Chunk Analysis Loop

```rust
while last_index + CONTENT_CHUNK_SIZE <= history.len() {
    let chunk = &history[last_index..last_index + CONTENT_CHUNK_SIZE];
    let hash = sha256(chunk);

    if is_loop_detected_for_chunk(chunk, hash) {
        return true;
    }

    last_index += 1;
}
```

### 5. Loop Detection Logic

```rust
fn is_loop_detected_for_chunk(
    chunk: &str,
    hash: &str,
    stats: &HashMap<String, Vec<usize>>,
) -> bool {
    let positions = stats.get(hash)?;

    // Verify actual content (prevent hash collision)
    let original_pos = positions[0];
    let original_chunk = &history[original_pos..original_pos + CONTENT_CHUNK_SIZE];
    if chunk != original_chunk {
        return false;
    }

    positions.push(last_index);

    if positions.len() < CONTENT_LOOP_THRESHOLD {
        return false;
    }

    // Check most recent occurrences are clustered
    let recent = &positions[positions.len() - CONTENT_LOOP_THRESHOLD..];
    let total_distance = recent.last()? - recent.first()?;
    let avg_distance = total_distance / (CONTENT_LOOP_THRESHOLD - 1);
    let max_distance = CONTENT_CHUNK_SIZE * MAX_ALLOWED_DISTANCE_MULTIPLIER;

    avg_distance <= max_distance
}
```

## Rust Implementation

````rust
use sha2::{Sha256, Digest};
use std::collections::HashMap;

pub struct ContentLoopDetector {
    history: String,
    stats: HashMap<String, Vec<usize>>,
    last_index: usize,
    in_code_block: bool,
}

impl ContentLoopDetector {
    pub fn new() -> Self {
        Self {
            history: String::new(),
            stats: HashMap::new(),
            last_index: 0,
            in_code_block: false,
        }
    }

    pub fn check(&mut self, content: &str) -> bool {
        if self.should_skip_tracking(content) {
            self.reset();
            return false;
        }

        self.update_code_block_state(content);

        if self.in_code_block {
            return false;
        }

        self.history.push_str(content);
        self.truncate_if_needed();

        while self.has_more_chunks() {
            let chunk = self.get_current_chunk();
            let hash = self.hash_chunk(&chunk);

            if self.check_chunk_loop(&chunk, &hash) {
                return true;
            }

            self.last_index += 1;
        }

        false
    }

    fn should_skip_tracking(&self, content: &str) -> bool {
        content.contains("```") ||
        content.contains('|') ||
        Regex::new(r"(^|\n)\s*[*\-+]\s").unwrap().is_match(content) ||
        content.contains('#')
    }

    fn update_code_block_state(&mut self, content: &str) {
        let fences = content.matches("```").count();
        self.in_code_block = fences % 2 != 0;
    }

    fn hash_chunk(&self, chunk: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(chunk.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    // ... helper methods
}
````

## Testing Scenarios

1. Random content: No detection
2. Repeating chunk consecutively: Detection at threshold
3. Repeating chunk far apart: No detection
4. Code block with repetition: No detection
5. Transition into code block: No detection
