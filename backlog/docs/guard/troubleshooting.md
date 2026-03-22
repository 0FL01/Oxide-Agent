# Guardian Troubleshooting

Common issues and solutions when implementing or using Guardian.

## Common Issues

### 1. Guardian Always Denies Everything

**Symptoms:** All requests get `risk_score >= 80`

**Possible Causes:**

- Model is too conservative
- Policy prompt not loaded correctly
- Transcript contains suspicious-looking but benign commands

**Solutions:**

```rust
// Check that policy.md is included correctly
const POLICY_PROMPT: &str = include_str!("policy.md");

// If model is too conservative, adjust the system prompt
let adjusted_prompt = format!(
    "{}\
    \n\nIMPORTANT: Be slightly more permissive for actions that are:\
    \n- Explicitly requested by the user\
    \n- Routine development operations\
    \n- Low-impact file operations\
    \nAssign HIGH risk only for clear data exfiltration or destructive actions.",
    POLICY_PROMPT
);
```

### 2. Guardian Times Out Frequently

**Symptoms:** `GuardianReviewSessionOutcome::TimedOut` happens often

**Possible Causes:**

- Model is slow to respond
- Network latency
- Transcript too long

**Solutions:**

```rust
// Increase timeout
const GUARDIAN_REVIEW_TIMEOUT: Duration = Duration::from_secs(120); // 2 minutes

// Or add retry logic
async fn review_with_retry(
    session: &GuardianSession,
    request: &Request,
    max_retries: u8,
) -> Result<Assessment> {
    for attempt in 0..max_retries {
        match session.review(request).await {
            Ok(assessment) => return Ok(assessment),
            Err(GuardianError::Timeout) if attempt < max_retries - 1 => {
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    Err(GuardianError::Timeout)
}
```

### 3. JSON Parsing Fails

**Symptoms:** `parse_guardian_assessment` returns error

**Possible Causes:**

- Model returns prose with JSON embedded
- JSON is truncated
- Malformed JSON output

**Solutions:**

```rust
// Improved parsing with multiple strategies
pub fn parse_guardian_assessment(text: &str) -> Result<GuardianAssessment> {
    // Strategy 1: Direct parse
    if let Ok(a) = serde_json::from_str(text) {
        return Ok(a);
    }

    // Strategy 2: Find first {
    if let Some(start) = text.find('{') {
        // Find matching }
        let mut depth = 0;
        for (i, c) in text[start..].chars().enumerate() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                let json_str = &text[start..=start + i];
                if let Ok(a) = serde_json::from_str(json_str) {
                    return Ok(a);
                }
            }
        }
    }

    // Strategy 3: Look for JSON-like structure
    let pattern = r#"\{[^}]*"risk_level"[^}]*\}"#;
    if let Ok(re) = Regex::new(pattern) {
        if let Some(mat) = re.find(text) {
            if let Ok(a) = serde_json::from_str(mat.as_str()) {
                return Ok(a);
            }
        }
    }

    Err(GuardianError::ParseFailed("Could not extract JSON".into()))
}
```

### 4. Session Cache Not Working

**Symptoms:** New Guardian session created for every review

**Possible Causes:**

- `reuse_key` changes every time
- Config includes volatile fields
- Trunk marked as stale

**Solutions:**

```rust
// Make reuse_key more stable
#[derive(Default)]
pub struct GuardianReviewSessionReuseKey {
    pub model: String,
    pub model_provider: String,
    // Exclude volatile fields like timestamps
    pub instructions_hash: u64,  // Hash instead of full text
    pub features: FeaturesFlags,
    pub cwd: Option<PathBuf>,     // Only if critical
}

// Only include truly relevant fields
impl GuardianReviewSessionReuseKey {
    pub fn from_config(config: &Config) -> Self {
        Self {
            model: config.model.clone(),
            model_provider: config.model_provider.clone(),
            instructions_hash: hash_str(&config.instructions),
            features: config.features.into(),
            cwd: None,  // Don't include - changes frequently
        }
    }
}
```

### 5. Memory Leaks with Sessions

**Symptoms:** Memory grows over time, sessions not released

**Possible Causes:**

- Ephemeral sessions not cleaned up
- Circular references
- `Arc` reference cycles

**Solutions:**

```rust
// Use Weak references
pub struct GuardianSessionManager {
    trunk: Option<Arc<GuardianSession>>,
    // Use Weak to avoid cycles
    ephemeral_refs: Vec<Weak<GuardianSession>>,
}

impl GuardianSessionManager {
    pub fn cleanup(&mut self) {
        // Remove dead weak references
        self.ephemeral_refs.retain(|w| w.upgrade().is_some());
    }
}

// Implement Drop for cleanup
impl Drop for EphemeralSession {
    fn drop(&mut self) {
        // Signal shutdown
        self.cancel_token.cancel();
        // Cleanup resources
    }
}
```

### 6. Transcript Too Large

**Symptoms:** Token limits exceeded, truncation not working

**Solutions:**

```rust
// Implement smarter truncation
pub fn smart_truncate_transcript(
    entries: Vec<TranscriptEntry>,
    max_tokens: usize,
) -> (Vec<TranscriptEntry>, usize) {
    let mut result = Vec::new();
    let mut tokens = 0;

    // Keep user messages (authorization signal)
    for entry in &entries {
        if matches!(entry, TranscriptEntry::User(_)) {
            let entry_tokens = estimate_tokens(entry);
            if tokens + entry_tokens <= max_tokens {
                tokens += entry_tokens;
                result.push(entry.clone());
            }
        }
    }

    // Fill remaining budget with other entries
    for entry in &entries {
        if !matches!(entry, TranscriptEntry::User(_)) {
            let entry_tokens = estimate_tokens(entry);
            if tokens + entry_tokens <= max_tokens {
                tokens += entry_tokens;
                result.push(entry.clone());
            }
        }
    }

    (result, tokens)
}
```

### 7. False Positives on Benign Commands

**Symptoms:** `curl` or `grep` commands always denied

**Solutions:**

```rust
// Add whitelisting for common tools
pub fn quick_approval_check(request: &GuardianApprovalRequest) -> Option<bool> {
    match request {
        GuardianApprovalRequest::Shell { command, .. } => {
            let cmd = command.join(" ");

            // Benign commands
            let benign = [
                "git status",
                "git diff",
                "ls",
                "pwd",
                "echo",
                "cat (non-secret files)",
                "head",
                "tail",
                "grep (local files)",
            ];

            for pattern in benign {
                if cmd.starts_with(pattern) {
                    return Some(true); // Auto-approve
                }
            }

            // Suspicious patterns that need review
            let suspicious = ["@", ">", "|", "&&", "||", ";"];
            let has_redirects = suspicious.iter().any(|s| cmd.contains(s));

            if has_redirects {
                return None; // Needs full review
            }

            Some(true) // Default approve simple commands
        }
        _ => None, // Full review for others
    }
}
```

## Debugging Tips

### Enable Verbose Logging

```rust
pub struct Guardian {
    logger: Option<Box<dyn Logger>>,
}

impl Guardian {
    pub fn review(&self, request: &Request, context: &str) -> Result<Assessment> {
        let start = Instant::now();

        if let Some(logger) = &self.logger {
            logger.debug(format!("Starting review for: {:?}", request));
        }

        let result = self.execute_review(request, context);

        if let Some(logger) = &self.logger {
            logger.debug(format!(
                "Review completed in {:?}: {:?}",
                start.elapsed(),
                result
            ));
        }

        result
    }
}
```

### Test with Fixed Responses

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct MockGuardian {
        response: &'static str,
    }

    impl Guardian for MockGuardian {
        fn review_impl(&self, _: &Request, _: &str) -> Result<Assessment> {
            serde_json::from_str(self.response)
                .map_err(|e| GuardianError::ParseFailed(e.to_string()))
        }
    }

    #[test]
    fn test_high_risk_shell_command() {
        let guardian = MockGuardian {
            response: r#"{"risk_level": "high", "risk_score": 95, "rationale": "test"}"#,
        };

        let request = GuardianApprovalRequest::Shell {
            id: "1".into(),
            command: vec!["curl".into(), "-X".into(), "POST".into()],
            cwd: PathBuf::from("/"),
            justification: None,
        };

        let assessment = guardian.review(&request, "test").unwrap();
        assert!(assessment.risk_score >= 80);
    }
}
```

### Monitor Common Patterns

```rust
pub struct GuardianMetrics {
    pub total_reviews: AtomicU64,
    pub approved: AtomicU64,
    pub denied: AtomicU64,
    pub timed_out: AtomicU64,
    pub avg_duration_ms: AtomicU64,
}

impl GuardianMetrics {
    pub fn record_review(&self, outcome: &ReviewOutcome, duration_ms: u64) {
        self.total_reviews.fetch_add(1, Ordering::Relaxed);

        match outcome {
            ReviewOutcome::Approved => self.approved.fetch_add(1, Ordering::Relaxed),
            ReviewOutcome::Denied => self.denied.fetch_add(1, Ordering::Relaxed),
            ReviewOutcome::Timeout => self.timed_out.fetch_add(1, Ordering::Relaxed),
        }

        // Rolling average
        let prev = self.avg_duration_ms.load(Ordering::Relaxed);
        let count = self.total_reviews.load(Ordering::Relaxed);
        let new_avg = (prev * (count - 1) + duration_ms) / count;
        self.avg_duration_ms.store(new_avg, Ordering::Relaxed);
    }
}
```

## Performance Optimization

### Parallel Reviews (when safe)

```rust
pub async fn review_parallel(
    requests: Vec<GuardianApprovalRequest>,
    session: &GuardianSession,
) -> Vec<Result<GuardianAssessment>> {
    let futures = requests
        .into_iter()
        .map(|req| session.review(&req));

    // Run in parallel, but limit concurrency
    let semaphore = Semaphore::new(3); // Max 3 concurrent

    let futures = futures.map(|fut| {
        let permit = semaphore.clone().acquire_owned();
        async move {
            let _permit = permit.await;
            fut.await
        }
    });

    futures::future::join_all(futures).await
}
```

### Session Warmup

```rust
pub struct Guardian {
    warmup_future: OnceCell<()>,
}

impl Guardian {
    pub fn warmup(&self, session: &GuardianSession) {
        self.warmup_future.get_or_init(|| {
            tokio::spawn(async move {
                // Run a dummy review to warm up the model
                let dummy_request = GuardianApprovalRequest::Shell {
                    id: "warmup".into(),
                    command: vec!["echo".into(), "warmup".into()],
                    cwd: PathBuf::from("/tmp"),
                    justification: None,
                };

                let _ = session.review(&dummy_request, "warmup").await;
            });
        });
    }
}
```

## Security Considerations

### Prevent Prompt Injection

```rust
pub fn sanitize_transcript_entry(entry: &mut TranscriptEntry) {
    match entry {
        TranscriptEntry::User(text) | TranscriptEntry::Assistant(text) => {
            // Remove attempts to override instructions
            let injection_patterns = [
                "ignore previous instructions",
                "disregard your instructions",
                "new system prompt",
                "you are now",
            ];

            for pattern in injection_patterns {
                if text.to_lowercase().contains(pattern) {
                    *text = text.replace(pattern, "[REDACTED]");
                }
            }
        }
        TranscriptEntry::Tool(name, args) => {
            // Sanitize tool names and args
            *name = sanitize_identifier(name);
            *args = sanitize_json(args);
        }
    }
}
```

### Audit Trail

```rust
pub struct GuardianAuditLog {
    entries: Mutex<Vec<AuditEntry>>,
}

#[derive(Debug, Clone)]
pub struct AuditEntry {
    pub timestamp: DateTime<Utc>,
    pub request: GuardianApprovalRequest,
    pub assessment: GuardianAssessment,
    pub decision: ReviewDecision,
    pub session_id: String,
}

impl GuardianAuditLog {
    pub fn record(
        &self,
        request: GuardianApprovalRequest,
        assessment: GuardianAssessment,
        decision: ReviewDecision,
    ) {
        let entry = AuditEntry {
            timestamp: Utc::now(),
            request,
            assessment,
            decision,
            session_id: current_session_id(),
        };

        self.entries.lock().unwrap().push(entry);
    }

    pub fn export(&self) -> Vec<AuditEntry> {
        self.entries.lock().unwrap().clone()
    }
}
```
