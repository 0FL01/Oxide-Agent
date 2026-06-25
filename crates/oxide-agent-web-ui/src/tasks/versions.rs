use oxide_agent_web_contracts::TaskSummary;
use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct TaskVersionGroup {
    pub(super) version_group_id: String,
    pub(super) versions: Vec<TaskSummary>,
}

/// Sort a set of task versions by version index, then creation time, then id.
/// Shared by `group_task_versions` (all groups) and `versions_for_group`
/// (single group) so the ordering is identical everywhere a card reads it.
fn sort_versions(versions: &mut [TaskSummary]) {
    versions.sort_by(|a, b| {
        a.effective_version_index()
            .cmp(&b.effective_version_index())
            .then_with(|| a.created_at.cmp(&b.created_at))
            .then_with(|| a.task_id.cmp(&b.task_id))
    });
}

/// Select and sort the versions that belong to a single version group from the
/// live `tasks` signal. This is the reactive source `TaskCard` reads instead
/// of a stale by-value snapshot, so status/timestamp updates flowing through
/// `tasks` reach the card without recreating it.
pub(super) fn versions_for_group(
    tasks: &[TaskSummary],
    version_group_id: &str,
) -> Vec<TaskSummary> {
    let mut versions: Vec<TaskSummary> = tasks
        .iter()
        .filter(|task| task.effective_version_group_id() == version_group_id)
        .cloned()
        .collect();
    sort_versions(&mut versions);
    versions
}

pub(super) fn group_task_versions(tasks: &[TaskSummary]) -> Vec<TaskVersionGroup> {
    let mut grouped = HashMap::<String, Vec<TaskSummary>>::new();
    for task in tasks {
        grouped
            .entry(task.effective_version_group_id().to_string())
            .or_default()
            .push(task.clone());
    }

    let mut groups = grouped
        .into_iter()
        .map(|(version_group_id, mut versions)| {
            sort_versions(&mut versions);
            TaskVersionGroup {
                version_group_id,
                versions,
            }
        })
        .collect::<Vec<_>>();

    groups.sort_by(|a, b| {
        first_group_task(&a.versions)
            .created_at
            .cmp(&first_group_task(&b.versions).created_at)
            .then_with(|| a.version_group_id.cmp(&b.version_group_id))
    });
    groups
}

fn first_group_task(versions: &[TaskSummary]) -> &TaskSummary {
    versions
        .first()
        .expect("task version groups must contain at least one task")
}

pub(super) fn selected_version_index(
    versions: &[TaskSummary],
    selected_task_id: Option<&str>,
) -> usize {
    selected_task_id
        .and_then(|task_id| versions.iter().position(|task| task.task_id == task_id))
        .unwrap_or_else(|| versions.len().saturating_sub(1))
}

pub(super) fn selected_visible_activity_task_ids(
    tasks: &[TaskSummary],
    selected_versions: &HashMap<String, String>,
) -> Vec<String> {
    group_task_versions(tasks)
        .into_iter()
        .filter_map(|group| {
            let selected_task_id = selected_versions
                .get(&group.version_group_id)
                .map(String::as_str);
            let selected_index = selected_version_index(&group.versions, selected_task_id);
            group
                .versions
                .get(selected_index)
                .map(|task| task.task_id.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::selected_visible_activity_task_ids;
    use oxide_agent_web_contracts::{TaskStatus, TaskSummary};
    use std::collections::HashMap;

    fn task(task_id: &str, group_id: &str, version_index: u32) -> TaskSummary {
        let created_at = format!("2026-06-11T00:00:{version_index:02}Z");
        let updated_at = format!("2026-06-11T00:00:{:02}Z", version_index + 1);
        serde_json::from_value(serde_json::json!({
            "task_id": task_id,
            "version_group_id": group_id,
            "version_index": version_index,
            "parent_task_id": null,
            "status": TaskStatus::Completed,
            "input_markdown": "input",
            "attachments": [],
            "input_edited_at": null,
            "final_response_markdown": null,
            "error_message": null,
            "pending_user_input": null,
            "last_event_seq": 0,
            "created_at": created_at,
            "started_at": created_at,
            "updated_at": updated_at,
            "finished_at": updated_at,
        }))
        .expect("task summary is valid")
    }

    #[test]
    fn selected_visible_activity_task_ids_match_rendered_versions() {
        let tasks = vec![
            task("task-a-v0", "group-a", 0),
            task("task-a-v1", "group-a", 1),
            task("task-b-v0", "group-b", 0),
            task("task-b-v1", "group-b", 1),
        ];
        let mut selected_versions = HashMap::new();
        selected_versions.insert("group-a".to_string(), "task-a-v0".to_string());

        assert_eq!(
            selected_visible_activity_task_ids(&tasks, &selected_versions),
            vec!["task-a-v0".to_string(), "task-b-v1".to_string()]
        );
    }
}
