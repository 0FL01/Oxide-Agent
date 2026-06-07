use leptos::prelude::{Get, ReadSignal, Set, WriteSignal};
use oxide_agent_web_contracts::{
    AgentEffort, AgentProfileSelection, AgentProfileView, UserSettingsResponse,
};

pub(super) const PROFILE_VALUE_DEFAULT: &str = "__default__";
pub(super) const PROFILE_VALUE_NONE: &str = "__none__";

pub(super) fn agent_effort_value(effort: AgentEffort) -> &'static str {
    match effort {
        AgentEffort::Standard => "standard",
        AgentEffort::Extended => "extended",
        AgentEffort::Heavy => "heavy",
    }
}

pub(super) fn agent_effort_from_value(value: &str) -> AgentEffort {
    match value {
        "extended" => AgentEffort::Extended,
        "heavy" => AgentEffort::Heavy,
        _ => AgentEffort::Standard,
    }
}

pub(super) fn apply_loaded_default_effort(
    settings: UserSettingsResponse,
    effort_touched: ReadSignal<bool>,
    set_selected_effort: WriteSignal<AgentEffort>,
) {
    if !effort_touched.get() {
        set_selected_effort.set(settings.default_effort.unwrap_or(AgentEffort::Standard));
    }
}

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
