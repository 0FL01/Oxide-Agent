use oxide_agent_web_contracts::{AgentProfileSelection, AgentProfileView};

pub(super) const PROFILE_VALUE_DEFAULT: &str = "__default__";
pub(super) const PROFILE_VALUE_NONE: &str = "__none__";

pub(super) fn missing_profile_option_label(
    profiles: &[AgentProfileView],
    selected: &str,
) -> Option<String> {
    if selected.is_empty()
        || selected == PROFILE_VALUE_DEFAULT
        || selected == PROFILE_VALUE_NONE
        || profiles.iter().any(|profile| profile.agent_id == selected)
    {
        return None;
    }
    Some(format!("Current profile · {selected}"))
}

pub(super) fn agent_profile_selection_from_value(value: &str) -> AgentProfileSelection {
    match value {
        PROFILE_VALUE_DEFAULT => AgentProfileSelection::Default,
        PROFILE_VALUE_NONE => AgentProfileSelection::None,
        value => AgentProfileSelection::Profile {
            agent_profile_id: value.to_string(),
        },
    }
}

pub(super) fn profile_value_to_id(value: &str) -> Option<String> {
    (value != PROFILE_VALUE_NONE && value != PROFILE_VALUE_DEFAULT && !value.trim().is_empty())
        .then(|| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::{PROFILE_VALUE_DEFAULT, PROFILE_VALUE_NONE, missing_profile_option_label};

    #[test]
    fn missing_profile_option_keeps_persisted_selection_visible_before_profiles_load() {
        assert_eq!(
            missing_profile_option_label(&[], "sre-agent"),
            Some("Current profile · sre-agent".to_string())
        );
        assert_eq!(missing_profile_option_label(&[], PROFILE_VALUE_NONE), None);
        assert_eq!(
            missing_profile_option_label(&[], PROFILE_VALUE_DEFAULT),
            None
        );
    }
}
