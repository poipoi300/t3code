use crate::ClientError;
use t3_contracts::{
    base_schemas::{EnvironmentId, ProjectId, ThreadId},
    environment::{ScopedProjectRef, ScopedThreadRef},
};
pub fn thread_key(reference: &ScopedThreadRef) -> String {
    format!("{}\0{}", reference.environment_id.0, reference.thread_id.0)
}
pub fn project_key(reference: &ScopedProjectRef) -> String {
    format!("{}\0{}", reference.environment_id.0, reference.project_id.0)
}
pub fn parse_thread_key(key: &str) -> Result<ScopedThreadRef, ClientError> {
    let (environment, thread) = key
        .split_once('\0')
        .ok_or_else(|| ClientError::InvalidPayload("invalid scoped thread key".into()))?;
    Ok(ScopedThreadRef {
        environment_id: EnvironmentId(environment.into()),
        thread_id: ThreadId(thread.into()),
    })
}
pub fn parse_project_key(key: &str) -> Result<ScopedProjectRef, ClientError> {
    let (environment, project) = key
        .split_once('\0')
        .ok_or_else(|| ClientError::InvalidPayload("invalid scoped project key".into()))?;
    Ok(ScopedProjectRef {
        environment_id: EnvironmentId(environment.into()),
        project_id: ProjectId(project.into()),
    })
}
pub fn project_collection_key(
    references: &[ScopedProjectRef],
) -> Result<String, serde_json::Error> {
    serde_json::to_string(
        &references
            .iter()
            .map(|r| (&r.environment_id, &r.project_id))
            .collect::<Vec<_>>(),
    )
}
pub fn parse_project_collection_key(key: &str) -> Result<Vec<ScopedProjectRef>, serde_json::Error> {
    let values: Vec<(EnvironmentId, ProjectId)> = serde_json::from_str(key)?;
    Ok(values
        .into_iter()
        .map(|(environment_id, project_id)| ScopedProjectRef {
            environment_id,
            project_id,
        })
        .collect())
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn roundtrips_and_first_separator() {
        for (environment, id) in [("e", "t"), ("e:1", "t:2"), ("e", "t\0suffix"), ("", "")] {
            let r = ScopedThreadRef {
                environment_id: EnvironmentId(environment.into()),
                thread_id: ThreadId(id.into()),
            };
            assert_eq!(parse_thread_key(&thread_key(&r)).unwrap(), r);
            let r = ScopedProjectRef {
                environment_id: EnvironmentId(environment.into()),
                project_id: ProjectId(id.into()),
            };
            assert_eq!(parse_project_key(&project_key(&r)).unwrap(), r);
        }
        assert!(parse_thread_key("bare").is_err());
        assert!(parse_project_key("bare").is_err());
    }
    #[test]
    fn collection_preserves_order_and_validates_tuples() {
        let refs = vec![
            ScopedProjectRef {
                environment_id: EnvironmentId("e2".into()),
                project_id: ProjectId("p".into()),
            },
            ScopedProjectRef {
                environment_id: EnvironmentId("e1".into()),
                project_id: ProjectId("p".into()),
            },
        ];
        assert_eq!(
            parse_project_collection_key(&project_collection_key(&refs).unwrap()).unwrap(),
            refs
        );
        for bad in ["null", "[[1,2]]", "[[\"a\"]]", "[[\"a\",\"b\",\"c\"]]"] {
            assert!(parse_project_collection_key(bad).is_err());
        }
    }
}
