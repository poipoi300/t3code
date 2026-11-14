//! Port of apps/web/src/components/ProjectFavicon.tsx ProjectFavicon.

use std::rc::Rc;

use nsh_core::prelude::*;
use t3_client::services::assets::{ProjectFaviconKey, is_project_favicon_fallback_url};
use t3_client::types::EnvironmentId;
use t3_client::workspace::project_ops::icons::{
    derive_project_identity, project_icon_color_class_name,
};
use t3_contracts::orchestration_project::OrchestrationProjectShell;
use t3_contracts::project::{ProjectIconColor, ProjectIconOverride};

use super::icon::{Icon, icon_with_class};
use crate::theme::{cn, tw};

type FaviconSource = Rc<dyn Fn(&ProjectFaviconKey) -> Option<String>>;

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectFaviconProject {
    pub environment_id: EnvironmentId,
    pub workspace_root: String,
    pub title: String,
    pub favicon_path: Option<String>,
    pub project_icon: Option<ProjectIconOverride>,
}

impl ProjectFaviconProject {
    pub fn from_shell(environment_id: EnvironmentId, project: &OrchestrationProjectShell) -> Self {
        Self {
            environment_id,
            workspace_root: project.workspace_root.0.clone(),
            title: project.title.0.clone(),
            favicon_path: project
                .favicon_path
                .as_ref()
                .and_then(|path| path.as_ref().map(|path| path.0.clone())),
            project_icon: project
                .project_icon
                .as_ref()
                .and_then(|icon| icon.as_ref().and_then(|icon| icon.0.clone())),
        }
    }

    pub fn resource_key(&self) -> ProjectFaviconKey {
        ProjectFaviconKey {
            environment: self.environment_id.clone(),
            cwd: self.workspace_root.clone(),
            path: self.favicon_path.clone().filter(|path| !path.is_empty()),
        }
    }
}

/// `source` reads the client's shared favicon resource (including its persistent image cache).
/// It is read only in automatic mode; overrides never activate a network resource.
pub fn project_favicon<P, S>(
    project: P,
    source: S,
    class_name: &str,
    fallback_icon: Option<Icon>,
) -> impl IntoElement + use<P, S>
where
    P: Fn() -> ProjectFaviconProject + 'static,
    S: Fn(&ProjectFaviconKey) -> Option<String> + 'static,
{
    let project = computed(project);
    let source = Rc::new(source);
    let class_name = class_name.to_owned();
    let override_icon = computed(move || project.with(|project| project.project_icon.clone()));
    dynamic(move || {
        let override_icon = override_icon.get();
        untracked(|| match override_icon {
            Some(ProjectIconOverride::Monogram(monogram)) => {
                project_monogram(&monogram.text.0, monogram.color, &class_name).into_element()
            }
            Some(ProjectIconOverride::Emoji(emoji)) => {
                project_favicon_fallback(None, Some(&emoji.emoji.0), Icon::FolderCode, &class_name)
            }
            Some(ProjectIconOverride::Lucide(glyph)) => {
                lucide_project_icon(&glyph.name.0, &glyph.color, &class_name).into_element()
            }
            None => automatic_project_favicon(
                project,
                source.clone(),
                &class_name,
                fallback_icon.unwrap_or(Icon::FolderCode),
            )
            .into_element(),
        })
    })
}

fn automatic_project_favicon(
    project: Memo<ProjectFaviconProject>,
    source: FaviconSource,
    class_name: &str,
    fallback_icon: Icon,
) -> impl IntoElement + use<> {
    let key = computed(move || project.with(ProjectFaviconProject::resource_key));
    let class_name = class_name.to_owned();
    dynamic(move || {
        let key = key.get();
        let source = source.clone();
        let class_name = class_name.clone();
        untracked(|| {
            let src = computed(move || source(&key));
            let present = computed(move || {
                src.with(|src| {
                    src.as_ref()
                        .is_some_and(|url| !url.is_empty() && !is_project_favicon_fallback_url(url))
                })
            });
            dynamic(move || {
                let present = present.get();
                untracked(|| {
                    if present {
                        project_favicon_image(src, project, fallback_icon, &class_name)
                            .into_element()
                    } else {
                        fallback_for_project(project, fallback_icon, &class_name).into_element()
                    }
                })
            })
        })
    })
}

/// Port of ProjectFaviconImage: keep the displayed image while a replacement loads.
fn project_favicon_image(
    src: Memo<Option<String>>,
    project: Memo<ProjectFaviconProject>,
    fallback_icon: Icon,
    class_name: &str,
) -> impl IntoElement + use<> {
    let displayed = signal(
        src.get_untracked()
            .filter(|src| src.starts_with("data:image/")),
    );
    let visible_class = class_name.to_owned();
    let shown = dynamic(move || match displayed.get() {
        Some(shown) => {
            let failed = shown.clone();
            let height = signal(14.0_f32);
            // Native load/error events carry no source URL; a new source needs a new node.
            image(shown.clone())
                .key(shown)
                .test_id("project-favicon-image")
                .app_class(tw(&cn([
                    Some("size-3.5 shrink-0 object-contain"),
                    Some(&visible_class),
                ])))
                // NSH rejects percentage radii in classes and typed styles; measure the tile.
                .on_layout(move |bounds: &Bounds<Points>| height.set(bounds.size.height.0))
                .rounded(move || px(height.get() * 0.25))
                .accessible(false)
                .on_error(move |_: &str| {
                    clear_failed_source(displayed, &failed);
                })
                .into_element()
        }
        None => fallback_for_project(project, fallback_icon, &visible_class).into_element(),
    });
    let loading = dynamic(move || {
        let next = src.get()?;
        if displayed.get().as_deref() == Some(next.as_str()) {
            return None;
        }
        let loaded = next.clone();
        // NSH's display:none image does not load on every backend; an invisible, non-layout
        // image is the web's hidden preloader, disposed with this source's region.
        Some(
            image(next.clone())
                .key(next.clone())
                .test_id("project-favicon-preloader")
                .app_class(tw("absolute size-3.5 opacity-0 pointer-events-none"))
                .accessible(false)
                .on_load(
                    move |_: &nsh_core::geometry::Size<nsh_core::geometry::DevicePixels>| {
                        if src.get_untracked().as_deref() == Some(loaded.as_str()) {
                            displayed.set(Some(loaded.clone()));
                        }
                    },
                )
                .on_error(move |_: &str| {
                    clear_failed_source(displayed, &next);
                }),
        )
    });
    // NSH has no fragment element; this wrapper keeps the visible tile and preloader together.
    view()
        .app_class(tw("relative inline-flex shrink-0"))
        .child(shown)
        .child(loading)
}

fn clear_failed_source(displayed: Signal<Option<String>>, failed: &str) {
    if displayed.with_untracked(|current| current.as_deref() == Some(failed)) {
        displayed.set(None);
    }
}

fn fallback_for_project(
    project: Memo<ProjectFaviconProject>,
    fallback_icon: Icon,
    class_name: &str,
) -> impl IntoElement + use<> {
    let class_name = class_name.to_owned();
    dynamic(move || {
        project.with(|project| {
            project_favicon_fallback(Some(&project.title), None, fallback_icon, &class_name)
        })
    })
}

fn project_favicon_fallback(
    project_name: Option<&str>,
    emoji: Option<&str>,
    fallback_icon: Icon,
    class_name: &str,
) -> AnyElement {
    if let Some(name) = project_name.filter(|name| !name.trim().is_empty()) {
        let identity = derive_project_identity(name);
        return project_monogram(&identity.monogram, identity.color, class_name).into_element();
    }
    if let Some(emoji) = emoji.filter(|emoji| !emoji.is_empty()) {
        let emoji = SharedString::from(emoji.to_owned());
        let height = signal(14.0_f32);
        // NSH has no container query units: measured tile height is the web's 80cqh.
        return view()
            .accessible(false)
            .app_class(tw(&cn([
                Some("inline-flex size-3.5 shrink-0 items-center justify-center leading-none"),
                Some(class_name),
            ])))
            .on_layout(move |bounds: &Bounds<Points>| height.set(bounds.size.height.0))
            .child(
                text(emoji)
                    .text_size(move || px(height.get() * 0.8))
                    .app_class(tw("leading-none"))
                    .accessible(false),
            )
            .into_element();
    }
    icon_with_class(
        fallback_icon,
        &cn([Some("size-3.5 shrink-0 text-icon-muted"), Some(class_name)]),
    )
    .into_element()
}

/// Port of ProjectFavicon.tsx LucideProjectIcon; NSH bundles glyphs instead of a lazy JS chunk.
pub fn lucide_project_icon(
    name: &str,
    color: &ProjectIconColor,
    class_name: &str,
) -> impl IntoElement + use<> {
    let color_class = project_icon_color_class_name(color);
    view()
        .accessible(false)
        .app_class(tw(&cn([
            Some("inline-flex size-3.5 shrink-0 items-center justify-center"),
            Some(color_class),
            Some(class_name),
        ])))
        .child(icon_with_class(
            Icon::from_name(name).unwrap_or(Icon::FolderCode),
            &cn([Some("size-full"), Some(color_class)]),
        ))
}

/// Port of apps/web/src/components/ProjectMonogram.tsx ProjectMonogram.
pub fn project_monogram(
    text_value: &str,
    color: ProjectIconColor,
    class_name: &str,
) -> impl IntoElement + use<> {
    let glyph = SharedString::from(text_value.to_owned());
    let height = signal(16.0_f32);
    // NSH's native SVG canvas drops text nodes; native text preserves the glyphs, but cannot
    // express SVG textLength/lengthAdjust or its y=10.8 baseline. The tile keeps the web's ratios.
    view()
        .accessible(false)
        .test_id("project-monogram")
        .app_class(tw(&cn([
            Some("inline-flex size-4 shrink-0 items-center justify-center"),
            Some(class_name),
        ])))
        .on_layout(move |bounds: &Bounds<Points>| height.set(bounds.size.height.0))
        .child(
            view()
                .app_class(tw(&cn([
                    Some("size-full overflow-hidden font-mono select-none"),
                    Some(project_icon_color_class_name(&color)),
                ])))
                .rounded(move || px(height.get() * 0.25))
                .child(
                    view()
                        .app_class(tw("size-full items-center justify-center bg-current/14"))
                        .child(
                            text(glyph)
                                .accessible(false)
                                .number_of_lines(1u32)
                                .text_size(move || px(height.get() * 8.25 / 16.0))
                                .app_class(tw("font-mono font-bold leading-none")),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_identity_normalizes_only_an_empty_path_and_keeps_environment_and_workspace() {
        let mut project = ProjectFaviconProject {
            environment_id: EnvironmentId("one".into()),
            workspace_root: "/repo".into(),
            title: "Project".into(),
            favicon_path: None,
            project_icon: None,
        };
        let original = project.resource_key();
        project.favicon_path = Some(String::new());
        assert_eq!(project.resource_key(), original);
        project.title = "Renamed".into();
        assert_eq!(project.resource_key(), original);
        project.favicon_path = Some(" icon.svg ".into());
        assert_eq!(project.resource_key().path.as_deref(), Some(" icon.svg "));
        project.favicon_path = None;
        project.environment_id = EnvironmentId("two".into());
        assert_ne!(project.resource_key(), original);
        project.environment_id = original.environment.clone();
        project.workspace_root = "/other".into();
        assert_ne!(project.resource_key(), original);
    }
}
