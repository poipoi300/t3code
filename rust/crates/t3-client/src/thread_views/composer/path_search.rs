use crate::{services::views::ComposerPathSearchTarget, types::EnvironmentId};
use t3_contracts::project::ProjectSearchEntriesInput;

pub struct PathSearchRequest {
    pub environment: EnvironmentId,
    pub input: ProjectSearchEntriesInput,
}

pub fn normalize_query(query: Option<&str>) -> Option<String> {
    query.map(|query| crate::view_logic::JsTrim::js_trim(query).to_owned())
}

pub fn request(
    target: &ComposerPathSearchTarget,
    can_read_files: bool,
) -> Option<PathSearchRequest> {
    if !can_read_files {
        return None;
    }
    let query = normalize_query(target.query.as_deref())?;
    let mut length = 0;
    let bounded = query
        .chars()
        .take_while(|character| {
            length += character.len_utf16();
            length <= 256
        })
        .collect::<String>();
    Some(PathSearchRequest {
        environment: target.environment.clone()?,
        input: ProjectSearchEntriesInput {
            cwd: t3_contracts::base_schemas::TrimmedNonEmptyString(target.cwd.clone()?),
            query: t3_contracts::base_schemas::TrimmedString(bounded),
            limit: 20,
            kind: None,
            image_only: None,
        },
    })
}

pub fn pending(target: &ComposerPathSearchTarget, search_pending: bool) -> bool {
    target.query.is_some() && search_pending
}
