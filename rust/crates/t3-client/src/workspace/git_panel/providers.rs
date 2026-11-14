//! Presentation fields from the shared source-control definition registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishProvider {
    pub kind: String, pub label: String, pub host: String, pub path_placeholder: String, pub icon_svg: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceControlPresentation { pub singular: String, pub short_label: String, pub icon_svg: Option<String> }

pub fn publish_providers() -> Vec<PublishProvider> {
    DEFINITIONS.iter().map(|&(kind, label, host, path, icon)| PublishProvider { kind: kind.into(), label: label.into(), host: host.into(), path_placeholder: path.into(), icon_svg: icon.into() }).collect()
}
pub fn presentation(kind: Option<&str>) -> SourceControlPresentation {
    let definition = match kind { None => DEFINITIONS.first(), Some(kind) => DEFINITIONS.iter().find(|d| d.0 == kind) };
    let (singular, short_label) = match kind {
        Some("gitlab") => ("merge request", "MR"),
        Some(_) if definition.is_none() => ("change request", "change request"),
        _ => ("pull request", "PR"),
    };
    SourceControlPresentation { singular: singular.into(), short_label: short_label.into(), icon_svg: definition.map(|d| d.4.into()) }
}

const DEFINITIONS: &[(&str, &str, &str, &str, &str)] = &[
    ("github", "GitHub", "github.com", "owner/repo", ""),
    ("cursor-origin", "Cursor Origin", "git.cursor.sh", "owner/repo", ""),
    ("gitlab", "GitLab", "gitlab.com", "group/project", ""),
    ("gitcafe", "GitCafe", "git.cafe", "owner/repo", r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><path d="M4.8 4.8h54.4v54.4H4.8z" fill="#a78bfa"/></svg>"##),
    ("forgejo", "Forgejo", "Your signed-in server", "owner/repo", ""),
    ("bitbucket", "Bitbucket", "bitbucket.org", "workspace/repository", ""),
    ("azure-devops", "Azure DevOps", "dev.azure.com", "project/repository", ""),
];
