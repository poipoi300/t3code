//! PullRequestMarkdown's provider line breaks and environment-bound image URLs.
use crate::{
    runtime::tasks::watch,
    screens::thread::markdown::{
        base::{MarkdownImageRenderer, chat_markdown},
        rich::{MarkdownImageState, MarkdownMediaProps, chat_markdown_image},
    },
};
use futures::{FutureExt, future::LocalBoxFuture};
use nsh_base::text_view::{Image, Inline, MarkdownPlugin, MarkdownPlugins, ParseContext, mdast};
use nsh_core::prelude::*;
use std::rc::Rc;
use t3_client::{composition::ClientRuntime, platform::Platform, types::EnvironmentId};
use t3_contracts::{
    assets::AssetResource, base_schemas::TrimmedNonEmptyString, pull_request::PullRequestDetail,
};

type AssetUrl =
    Rc<dyn Fn(&EnvironmentId, AssetResource) -> LocalBoxFuture<'static, Option<String>>>;
#[derive(Clone)]
pub(super) struct Media(AssetUrl);
impl Media {
    pub fn new<P: Platform + Clone + 'static>(client: &ClientRuntime<P>) -> Self {
        let files = client.project_files();
        Self(Rc::new(move |env, resource| {
            files.asset_url(env, resource).map(Result::ok).boxed_local()
        }))
    }
    pub fn scope(&self, env: EnvironmentId, pr: &PullRequestDetail) -> PrMarkdown {
        PrMarkdown {
            env,
            media: self.clone(),
            cwd: pr.workspace_root.clone(),
            repository: pr.repository.clone(),
            hard_breaks: matches!(
                pr.provider.0.as_str(),
                "github" | "forgejo" | "azure-devops"
            ),
        }
    }
}
#[derive(Clone)]
pub(super) struct PrMarkdown {
    env: EnvironmentId,
    media: Media,
    cwd: TrimmedNonEmptyString,
    repository: TrimmedNonEmptyString,
    hard_breaks: bool,
}

struct HardBreaks;
impl MarkdownPlugin for HardBreaks {
    fn name(&self) -> &str {
        "pull-request-hard-breaks"
    }
    fn inline(&self, node: &mdast::Node, _: &mut ParseContext<'_>) -> Option<Inline> {
        match node {
            mdast::Node::Text(t) if t.value.contains('\n') => Some(Inline::text(t.value.clone())),
            _ => None,
        }
    }
}
impl PrMarkdown {
    pub fn render(&self, body: &str) -> AnyElement {
        let plugins = use_context::<MarkdownPlugins>().unwrap_or_default();
        let body = body.to_owned();
        let media = self.clone();
        dynamic(move || {
            untracked(|| {
                if media.hard_breaks {
                    provide_context(plugins.clone().with(HardBreaks));
                }
                let media = media.clone();
                provide_context(MarkdownImageRenderer(Rc::new(move |image| {
                    media.image(image)
                })));
                let body = body.clone();
                chat_markdown(move || body.clone(), || false)
            })
        })
        .into_element()
    }
    fn image(&self, image: &Image) -> AnyElement {
        let src = signal(None::<String>);
        let failed = signal(false);
        let resource = AssetResource::SourceControlMedia {
            cwd: self.cwd.clone(),
            repository: Some(self.repository.clone()),
            url: TrimmedNonEmptyString(image.src.clone()),
        };
        let request = (self.media.0)(&self.env, resource);
        watch(async move {
            match request.await {
                Some(url) => src.set(Some(url)),
                None => failed.set(true),
            }
        });
        let alt = image.alt.clone();
        let fallback = alt.clone();
        let copy = super::super::thread::markdown::rich::markdown_image_copy(
            &image.alt,
            &image.src,
            image.title.as_deref(),
        );
        let props = computed(move || MarkdownMediaProps {
            src: src.get(),
            alt: alt.clone(),
            copy_markdown: Some(copy.clone()),
            ..Default::default()
        });
        dynamic(move || {
            if failed.get() {
                text(SharedString::from(fallback.clone())).into_element()
            } else {
                untracked(|| {
                    chat_markdown_image(props, MarkdownImageState::default(), false).into_element()
                })
            }
        })
        .into_element()
    }
}
