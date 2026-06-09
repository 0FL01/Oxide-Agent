use oxide_agent_web_contracts::TaskSummary;
use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct TaskVersionGroup {
    pub(super) version_group_id: String,
    pub(super) versions: Vec<TaskSummary>,
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
            versions.sort_by(|a, b| {
                a.effective_version_index()
                    .cmp(&b.effective_version_index())
                    .then_with(|| a.created_at.cmp(&b.created_at))
                    .then_with(|| a.task_id.cmp(&b.task_id))
            });
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
