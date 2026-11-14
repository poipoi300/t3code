use super::*;

pub fn default_project(
    projects: &[EnvironmentProject],
    order: &[String],
    last_used: Option<&str>,
    fallback_ready: bool,
) -> Option<ProjectKey> {
    let projects = projects
        .iter()
        .filter(|p| p.project.archived_at.as_ref().is_none_or(Option::is_none))
        .cloned()
        .collect::<Vec<_>>();
    if let Some(project) = projects.iter().find(|p| {
        last_used.is_some_and(|last| last == format!("{}:{}", p.environment.0, p.project.id.0))
    }) {
        return Some(project.key());
    }
    if !fallback_ready {
        return None;
    }
    crate::view_logic::shell::sidebar::order_by_preferences(&projects, order, |p| {
        vec![
            p.physical_key().legacy_preference_key(),
            format!(
                "legacy-project-cwd:{}",
                projects::normalize_project_path_for_comparison(&p.project.workspace_root.0)
            ),
        ]
    })
    .first()
    .map(|p| p.key())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn last_used_is_scoped_and_missing_projects_wait_for_fallback_readiness() {
        let source = serde_json::from_str(
            include_str!("../../../../../fixtures/samples/OrchestrationProjectShell.jsonl")
                .lines()
                .next()
                .expect("fixture"),
        )
        .expect("project");
        let first = EnvironmentProject {
            environment: EnvironmentId("first".into()),
            project: source,
            is_scratch: false,
        };
        let mut last = first.clone();
        last.environment = EnvironmentId("last-used".into());
        let key = format!("last-used:{}", last.project.id.0);
        assert_eq!(
            default_project(&[first.clone(), last.clone()], &[], Some(&key), true),
            Some(last.key())
        );
        assert_eq!(
            default_project(std::slice::from_ref(&first), &[], Some(&key), false),
            None
        );
        assert_eq!(
            default_project(std::slice::from_ref(&first), &[], Some(&key), true),
            Some(first.key())
        );
        last.project.archived_at = Some(Some(t3_contracts::wire::WireDateTime(
            "2026-11-09T00:00:00.000Z".into(),
        )));
        assert_eq!(
            default_project(&[first.clone(), last], &[], Some(&key), true),
            Some(first.key())
        );
    }
}
