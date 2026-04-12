# Tool Call Loop Detection

## Algorithm

```rust
struct ToolCallTracker {
    last_key: Option<String>,      // SHA256 hash
    repetition_count: usize,
}

const TOOL_CALL_LOOP_THRESHOLD: usize = 5;
```

### Step-by-Step

1. **Generate key for tool call:**

   ```rust
   key = sha256(format!("{}:{}", tool_name, json_stringify(args)))
   ```

2. **Compare with last call:**

   ```rust
   if last_key == Some(key) {
       repetition_count += 1;
   } else {
       last_key = Some(key);
       repetition_count = 1;
   }
   ```

3. **Check threshold:**
   ```rust
   if repetition_count >= TOOL_CALL_LOOP_THRESHOLD {
       return LoopDetected::ConsecutiveIdenticalToolCalls;
   }
   ```

## Edge Cases

- Different args: Reset counter
- Different tool name: Reset counter
- Non-tool events: No effect on counter

## Rust Implementation Notes

Use `sha2` crate:

```toml
[dependencies]
sha2 = "0.10"
serde_json = "1.0"
```

```rust
use sha2::{Sha256, Digest};
use serde_json::Value;

fn get_tool_call_key(name: &str, args: &Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(name.as_bytes());
    hasher.update(b":");
    hasher.update(args.to_string().as_bytes());
    format!("{:x}", hasher.finalize())
}
```

## Testing Scenarios

1. `< threshold` calls: No detection
2. `== threshold` calls: Detection
3. `> threshold` calls: Detection (only once)
4. Alternating tools: No detection
5. Different args: No detection
