use oxide_agent_core::agent::recovery::sanitize_xml_tags;
use proptest::prelude::*;

proptest! {
    /// Test that sanitize_xml_tags does not crash on any valid UTF-8 input.
    #[test]
    fn does_not_crash(s in "\\PC*") {
        let _ = sanitize_xml_tags(&s);
    }

    /// Test that forbidden tags are actually removed from the output.
    #[test]
    fn removes_forbidden_tags(
        // List of tags matched by the regex in recovery.rs
        tag in "(tool_call|tool_name|filepath|arg_key|arg_value|command|query|url|content|directory|path|arg_key_[0-9]+|arg_value_[0-9]+|arg[0-9]+)",
        is_closing in proptest::bool::ANY,
        // Content around the tag
        prefix in "[a-zA-Z0-9 ]*",
        suffix in "[a-zA-Z0-9 ]*"
    ) {
        let tag_str = if is_closing {
            format!("</{}>", tag)
        } else {
            format!("<{}>", tag)
        };

        let input = format!("{} {} {}", prefix, tag_str, suffix);
        let result = sanitize_xml_tags(&input);

        // The tag should no longer be present in the output
        prop_assert!(!result.contains(&tag_str), "Tag {} was not removed from {}", tag_str, input);

        // The prefix and suffix should still be there (though whitespace might be normalized)
        if !prefix.is_empty() {
            prop_assert!(result.contains(prefix.trim()), "Prefix '{}' lost in '{}'", prefix, result);
        }
        if !suffix.is_empty() {
            prop_assert!(result.contains(suffix.trim()), "Suffix '{}' lost in '{}'", suffix, result);
        }
    }

    /// Test that multiple forbidden tags are all removed.
    #[test]
    fn removes_multiple_tags(
        t1 in "tool_call|filepath|command",
        t2 in "arg_key|arg_value|content",
        content in "[a-zA-Z0-9 ]*"
    ) {
        let input = format!("<{}><{}>{}</{}></{}>", t1, t2, content, t2, t1);
        let result = sanitize_xml_tags(&input);

        let t1_open = format!("<{}>", t1);
        let t2_open = format!("<{}>", t2);
        let t1_close = format!("</{}>", t1);
        let t2_close = format!("</{}>", t2);

        prop_assert!(!result.contains(&t1_open));
        prop_assert!(!result.contains(&t2_open));
        prop_assert!(!result.contains(&t1_close));
        prop_assert!(!result.contains(&t2_close));

        if !content.is_empty() {
             prop_assert!(result.contains(content.trim()));
        }
    }
}
