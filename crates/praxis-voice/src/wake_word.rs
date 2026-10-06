use serde::{Deserialize, Serialize};

/// Check if transcribed text matches any wake words
/// - ["*"] means always match (listen always)
/// - ["orange", "tomato"] means match if text contains "orange" or "tomato"
/// - [] means never match (listening disabled)
pub fn matches_wake_word(text: &str, wake_words: &[String]) -> WakeWordMatch {
    tracing::debug!("WAKE_WORD: Checking '{}' against {:?}", text, wake_words);

    // Empty = listening disabled
    if wake_words.is_empty() {
        tracing::debug!("WAKE_WORD: No wake words configured, listening disabled");
        return WakeWordMatch {
            matched: false,
            wake_word: None,
            remaining_text: text.to_string(),
        };
    }

    // ["*"] = always listen
    if wake_words.contains(&"*".to_string()) {
        tracing::debug!("WAKE_WORD: Wildcard '*' active, always matching");
        return WakeWordMatch {
            matched: true,
            wake_word: None,
            remaining_text: text.to_string(),
        };
    }

    let text_lower = text.to_lowercase();
    for word in wake_words {
        if text_lower.contains(&word.to_lowercase()) {
            let remaining = text_lower
                .replace(&word.to_lowercase(), "")
                .trim()
                .to_string();
            tracing::info!(
                "WAKE_WORD: Matched wake word '{}' in '{}', remaining: '{}'",
                word,
                text,
                remaining
            );
            return WakeWordMatch {
                matched: true,
                wake_word: Some(word.clone()),
                remaining_text: if remaining.is_empty() {
                    text.to_string()
                } else {
                    remaining
                },
            };
        }
    }

    tracing::debug!("WAKE_WORD: No wake word matched in '{}'", text);
    WakeWordMatch {
        matched: false,
        wake_word: None,
        remaining_text: text.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WakeWordMatch {
    pub matched: bool,
    pub wake_word: Option<String>,
    pub remaining_text: String,
}

#[cfg(test)]
mod wake_word_tests {
    use super::*;

    #[test]
    fn test_empty_wake_words_disabled() {
        let result = matches_wake_word("hello world", &[]);
        assert!(!result.matched);
    }

    #[test]
    fn test_star_wake_word_always_matches() {
        let words = vec!["*".to_string()];
        let result = matches_wake_word("anything goes here", &words);
        assert!(result.matched);
        assert!(result.wake_word.is_none());
    }

    #[test]
    fn test_wake_word_found() {
        let words = vec!["orange".to_string(), "tomato".to_string()];
        let result = matches_wake_word("orange please help me", &words);
        assert!(result.matched);
        assert_eq!(result.wake_word, Some("orange".to_string()));
    }

    #[test]
    fn test_wake_word_case_insensitive() {
        let words = vec!["Orange".to_string()];
        let result = matches_wake_word("I said ORANGE help", &words);
        assert!(result.matched);
    }

    #[test]
    fn test_wake_word_not_found() {
        let words = vec!["orange".to_string(), "tomato".to_string()];
        let result = matches_wake_word("hello world", &words);
        assert!(!result.matched);
    }

    #[test]
    fn test_wake_word_second_word() {
        let words = vec!["orange".to_string(), "tomato".to_string()];
        let result = matches_wake_word("tomato show me files", &words);
        assert!(result.matched);
        assert_eq!(result.wake_word, Some("tomato".to_string()));
    }
}
