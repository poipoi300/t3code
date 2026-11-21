use super::super::pr_list::ListSort;
use crate::{
    ClientError,
    platform::{Storage, StorageArea, StorageKey, StorageWrite},
};
use serde_json::{Value, json};
use t3_contracts::pull_request::{
    PullRequestInvolvement, PullRequestListFilters, PullRequestListState,
};

pub fn default_preferences() -> Value {
    json!({"involvement": "all", "state": "open"})
}
pub fn normalize_preferences(value: &Value) -> Option<Value> {
    serde_json::from_value::<PullRequestInvolvement>(value.get("involvement")?.clone()).ok()?;
    serde_json::from_value::<PullRequestListState>(value.get("state")?.clone()).ok()?;
    let mut result = json!({"involvement": value["involvement"], "state": value["state"]});
    for key in ["environmentId", "projectId", "host", "q", "author"] {
        if let Some(raw) = value.get(key) {
            let text = raw.as_str()?;
            if matches!(key, "environmentId" | "projectId") {
                if text.trim().is_empty() {
                    return None;
                }
            } else if text.encode_utf16().count() > 200 {
                return None;
            }
            if !text.is_empty() {
                result[key] = raw.clone();
            }
        }
    }
    for key in ["draft", "review", "checks"] {
        if let Some(raw) = value.get(key) {
            raw.as_str()?;
            let filter =
                serde_json::from_value::<PullRequestListFilters>(json!({ key: raw })).ok()?;
            serde_json::to_value(filter).ok()?.get(key)?;
            result[key] = raw.clone();
        }
    }
    if let Some(labels) = value.get("labels") {
        let labels = labels.as_array()?;
        if labels.len() > 10
            || labels.iter().any(|label| {
                label
                    .as_str()
                    .is_none_or(|s| s.encode_utf16().count() > 200)
            })
        {
            return None;
        }
        if !labels.is_empty() {
            result["labels"] = json!(labels);
        }
    }
    if let Some(sort) = value.get("sort") {
        let decoded = serde_json::from_value::<ListSort>(sort.clone()).ok()?;
        if decoded != ListSort::Ready {
            result["sort"] = sort.clone();
        }
    }
    Some(result)
}
fn key() -> StorageKey {
    StorageKey::new("local-preferences", None, "t3.pullRequests.preferences")
}
pub async fn read_preferences(storage: &impl Storage) -> Value {
    match storage.get(StorageArea::Ordinary, key()).await {
        Ok(Some(bytes)) => serde_json::from_slice(&bytes)
            .ok()
            .and_then(|value| normalize_preferences(&value))
            .unwrap_or_else(default_preferences),
        _ => default_preferences(),
    }
}
pub async fn write_preferences(
    storage: &impl Storage,
    preferences: &Value,
) -> Result<(), ClientError> {
    storage
        .write(
            StorageArea::Ordinary,
            vec![StorageWrite {
                key: key(),
                value: Some(serde_json::to_vec(preferences)?),
            }],
        )
        .await?;
    Ok(())
}

/// The search parameters of /pull-requests. Unknown parameters survive a filter change,
/// including the detail selection maintained by the detail panel.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListSearch(Vec<(String, String)>);

impl ListSearch {
    pub fn parse(search: &str) -> Self {
        Self(
            url::form_urlencoded::parse(search.trim_start_matches('?').as_bytes())
                .into_owned()
                .collect(),
        )
    }

    pub fn value(&self, key: &str) -> &str {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map_or("", |(_, v)| v)
    }

    /// Typed qualifiers take precedence over the Draft menu until the user changes it.
    pub fn effective_draft(&self) -> &str {
        use t3_contracts::pull_request::PullRequestListFiltersDraft;
        match super::queries::parse_list_search(self.value("q"))
            .1
            .draft
            .flatten()
        {
            Some(PullRequestListFiltersDraft::Only) => "only",
            Some(PullRequestListFiltersDraft::Hide) => "hide",
            None => self.value("draft"),
        }
    }

    pub fn set(&mut self, key: &str, value: &str) {
        if key == "draft" {
            if value == self.effective_draft() {
                return;
            }
            if super::queries::parse_list_search(self.value("q"))
                .1
                .draft
                .is_some()
            {
                let q = super::queries::without_draft_qualifiers(self.value("q"));
                self.replace("q", &q);
            }
        }
        self.replace(key, value);
    }

    fn replace(&mut self, key: &str, value: &str) {
        self.0.retain(|(k, _)| k != key);
        if !value.is_empty() {
            self.0.push((key.into(), value.into()));
        }
    }

    /// Resolve a URL selection against known, non-scratch checkouts. A named server can
    /// still be pairing, so its project must never be resolved on a different server.
    pub fn detail_surface(
        &self,
        projects: &[crate::workspace::projects::EnvironmentProject],
        all_shells_known: bool,
    ) -> Option<crate::view_logic::shell::panels::Surface> {
        let prefer = |selection, filter| {
            let a = self.value(selection);
            if a.is_empty() { self.value(filter) } else { a }
        };
        let environment = prefer("selectedEnvironmentId", "environmentId");
        let host = prefer("selectedHost", "host");
        let project = prefer("selectedProjectId", "projectId");
        let repository = self.value("repository").trim();
        let number = self
            .value("number")
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)?;
        if repository.is_empty() || (environment.is_empty() && !all_shells_known) {
            return None;
        }
        let mut matches = projects.iter().filter(|candidate| {
            if candidate.is_scratch
                || (!environment.is_empty() && candidate.environment.0 != environment)
            {
                return false;
            }
            if !project.is_empty() {
                return candidate.project.id.0 == project;
            }
            candidate
                .project
                .repository_identity
                .as_ref()
                .and_then(Option::as_ref)
                .is_some_and(|identity| {
                    let identity_host = identity
                        .canonical_key
                        .0
                        .split('/')
                        .next()
                        .unwrap_or_default();
                    if !host.is_empty() && !identity_host.eq_ignore_ascii_case(host) {
                        return false;
                    }
                    let selector = if identity
                        .provider
                        .as_ref()
                        .is_some_and(|p| p.0 == "azure-devops")
                    {
                        identity
                            .name
                            .as_ref()
                            .map(|n| n.0.as_str())
                            .or_else(|| identity.display_name.as_ref()?.0.rsplit('/').next())
                            .map(|n| {
                                percent_encoding::percent_decode_str(n)
                                    .decode_utf8_lossy()
                                    .into_owned()
                            })
                    } else {
                        identity
                            .display_name
                            .as_ref()
                            .map(|n| n.0.clone())
                            .or_else(|| {
                                Some(format!(
                                    "{}/{}",
                                    identity.owner.as_ref()?.0,
                                    identity.name.as_ref()?.0
                                ))
                            })
                    };
                    selector.is_some_and(|r| r.eq_ignore_ascii_case(repository))
                })
        });
        let owner = matches.next()?;
        if matches.next().is_some() {
            return None;
        }
        Some(crate::view_logic::shell::panels::Surface::pull_request(
            Some(&owner.environment.0),
            &owner.project.id.0,
            (!host.is_empty()).then_some(host),
            repository,
            number,
        ))
    }

    pub fn href(&self) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&self.0)
            .finish();
        if query.is_empty() {
            "/pull-requests".into()
        } else {
            format!("/pull-requests?{query}")
        }
    }

    pub fn input(&self, query: &str) -> t3_contracts::pull_request::PullRequestListInput {
        use super::{markdown::bound_utf16, queries::parse_list_search};
        use t3_contracts::base_schemas::{ProjectId, TrimmedNonEmptyString};
        use t3_contracts::pull_request::PullRequestListInput;
        let bounded = |s: &str| TrimmedNonEmptyString(bound_utf16(s, 200, ""));
        let opt = |s: &str| (!s.is_empty()).then(|| Some(bounded(s)));
        let (text, mut filters) = parse_list_search(&bound_utf16(query.trim(), 200, ""));
        macro_rules! menu {
            ($field:ident) => {
                if filters.$field.is_none() {
                    filters.$field = serde_json::from_value(Value::String(
                        self.value(stringify!($field)).into(),
                    ))
                    .ok()
                    .map(Some);
                }
            };
        }
        menu!(draft);
        menu!(review);
        menu!(checks);
        if filters.author.is_none() {
            filters.author = opt(self.value("author").trim());
        }
        if filters.labels.is_none() {
            let mut labels = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            for (_, raw) in self.0.iter().filter(|(k, _)| k == "labels").take(100) {
                let candidates =
                    serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| vec![raw.clone()]);
                for value in candidates.into_iter().take(100) {
                    let name = bounded(value.trim());
                    if !name.0.is_empty() && labels.len() < 10 && seen.insert(name.0.to_lowercase())
                    {
                        labels.push(vec![name]);
                    }
                }
            }
            if !labels.is_empty() {
                filters.labels = Some(Some(labels));
            }
        }
        let filtered = filters.draft.is_some()
            || filters.review.is_some()
            || filters.checks.is_some()
            || filters.labels.is_some()
            || filters.excluded_labels.is_some()
            || filters.author.is_some();
        let involvement = serde_json::from_value(Value::String(self.value("involvement").into()))
            .unwrap_or(PullRequestInvolvement::All);
        let state = serde_json::from_value(Value::String(self.value("state").into()))
            .unwrap_or(PullRequestListState::Open);
        PullRequestListInput {
            state,
            involvement: Some(Some(involvement)),
            query: opt(&text),
            filters: filtered.then_some(Some(filters)),
            limit: Some(Some(99)),
            host: opt(self.value("host")),
            project_id: (!self.value("projectId").is_empty())
                .then(|| Some(ProjectId(self.value("projectId").into()))),
            project_ids: None,
            cursors: None,
        }
    }

    pub fn sort(&self) -> ListSort {
        serde_json::from_value(Value::String(self.value("sort").into())).unwrap_or(ListSort::Ready)
    }
}
