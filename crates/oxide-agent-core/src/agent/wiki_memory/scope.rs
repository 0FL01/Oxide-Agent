use sha2::{Digest, Sha256};

const CONTEXT_SLUG_MAX_CHARS: usize = 48;

/// Create a deterministic, storage-safe wiki context id from user and transport context.
#[must_use]
pub fn wiki_context_id(user_id: i64, context_key: &str) -> String {
    let slug = wiki_slug(context_key, CONTEXT_SLUG_MAX_CHARS);
    let mut hasher = Sha256::new();
    hasher.update(user_id.to_string().as_bytes());
    hasher.update(b":");
    hasher.update(context_key.as_bytes());
    let digest = format!("{:x}", hasher.finalize());
    format!("{slug}-{}", &digest[..8])
}

/// Slugify arbitrary user/context text for deterministic wiki paths.
#[must_use]
pub fn wiki_slug(value: &str, max_chars: usize) -> String {
    let mut slug = String::new();
    let mut previous_dash = false;

    for ch in value.chars() {
        let normalized = if ch.is_ascii_alphanumeric() {
            Some(ch.to_ascii_lowercase())
        } else if ch == '-' || ch == '_' || ch.is_whitespace() || ch == ':' || ch == '/' {
            Some('-')
        } else {
            None
        };

        match normalized {
            Some('-') if !previous_dash && !slug.is_empty() => {
                slug.push('-');
                previous_dash = true;
            }
            Some(ch) if ch != '-' => {
                slug.push(ch);
                previous_dash = false;
            }
            _ => {}
        }

        if slug.len() >= max_chars {
            break;
        }
    }

    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "context".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wiki_slug_normalizes_context_key() {
        assert_eq!(
            wiki_slug("Telegram Topic: 1234/Deploy", 64),
            "telegram-topic-1234-deploy"
        );
    }

    #[test]
    fn wiki_slug_falls_back_for_empty_values() {
        assert_eq!(wiki_slug("///", 64), "context");
    }

    #[test]
    fn wiki_context_id_includes_slug_and_stable_hash() {
        let first = wiki_context_id(42, "Telegram Topic: 1234");
        let second = wiki_context_id(42, "Telegram Topic: 1234");
        assert_eq!(first, second);
        assert!(first.starts_with("telegram-topic-1234-"));
        assert_eq!(first.rsplit_once('-').map(|(_, hash)| hash.len()), Some(8));
    }

    #[test]
    fn wiki_context_id_is_user_scoped() {
        let first = wiki_context_id(1, "topic");
        let second = wiki_context_id(2, "topic");
        assert_ne!(first, second);
    }
}
