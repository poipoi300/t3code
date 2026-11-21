//! Display the review guide already produced for this change (PullRequestGuideTab and GuideStop).
use super::{detail::button, markdown::PrMarkdown};
use crate::{
    i18n::{t, tr},
    runtime::tasks::watch,
    theme::tw,
};
use nsh_core::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use t3_client::workspace::pr_views::service::PullRequestReviewSession;
use t3_client::workspace::{
    diff_panel::{comments::file_diff_metadata, files::parse_patch_files},
    pr_views::guide::excerpt_text,
};
use t3_contracts::pull_request_review_guide::*;

const FAILURE_HINT: &str = "Check the review guide model in Settings → Source control.";

pub(super) fn guide(
    client: PullRequestReviewSession,
    markdown: PrMarkdown,
    open_code: impl Fn() + Copy + 'static,
) -> AnyElement {
    let reader = client.clone();
    let data = signal(None);
    let retry = signal(0u64);
    effect(move || {
        retry.get();
        let client = client.clone();
        watch(async move {
            loop {
                let response = client.guide().await;
                let running = response.as_ref().is_ok_and(|result| {
                    matches!(result.run, Some(PullRequestReviewGuideRun::Running(_)))
                });
                data.set(Some(response.map(Rc::new).map_err(|_| ())));
                if !running {
                    break;
                }
                timer(std::time::Duration::from_secs(5)).await;
            }
        });
    });
    let current_guide = computed(move || {
        data.with(|result| {
            result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .and_then(|result| result.guide.clone())
                .map(Rc::new)
        })
    });
    view()
        .app_class(tw("flex min-w-0 flex-col gap-3"))
        .child(dynamic(move || match data.get() {
            None => view().into_element(),
            Some(Err(())) => view()
                .child(text(t("Could not load the review guide")))
                .child(button(t("Retry"), move || retry.update(|n| *n += 1)))
                .into_element(),
            Some(Ok(result)) => status(&result),
        }))
        .child(dynamic(move || {
            current_guide
                .get()
                .map(|guide| tour(guide, markdown.clone(), reader.clone(), open_code))
        }))
        .into_element()
}
fn status(result: &PullRequestReviewGuideResult) -> AnyElement {
    let running = matches!(result.run, Some(PullRequestReviewGuideRun::Running(_)));
    let failure = match &result.run {
        Some(PullRequestReviewGuideRun::Failed(f))
            if Some(&f.head_sha) == result.current_head_sha.as_ref() =>
        {
            Some(f.detail.0.clone())
        }
        _ => None,
    };
    let mut root = view().app_class(tw("flex min-w-0 flex-col gap-3"));
    let description = match (&result.guide, running, failure.as_ref()) {
        (None, true, _) => {
            root = root.child(text(t("Writing a review guide")));
            Some("An agent is reading the change. A large one takes a few minutes.")
        }
        (None, false, None) => {
            root = root.child(text(t("No review guide yet")));
            Some(
                "A guide is a tour of the change: stops through the code in reading order, with the calls only a maintainer can make.",
            )
        }
        (None, false, Some(_)) => {
            root = root.child(text(t("Could not write a review guide")));
            Some(FAILURE_HINT)
        }
        (Some(_), true, _) => Some("Rewriting this guide."),
        (Some(_), false, Some(_)) => {
            root = root.child(text(t("Could not rewrite this guide.")));
            Some(FAILURE_HINT)
        }
        (Some(g), false, None)
            if result
                .current_head_sha
                .as_ref()
                .is_some_and(|head| head != &g.head_sha) =>
        {
            Some("Written before the latest push.")
        }
        _ => None,
    };
    if let Some(description) = description {
        root = root.child(text(t(description)));
    }
    if !running && let Some(reason) = failure {
        let expanded = signal(false);
        root = root
            .child(button(t("Technical details"), move || {
                expanded.update(|v| *v = !*v)
            }))
            .child(dynamic(move || {
                expanded
                    .get()
                    .then(|| text(SharedString::from(reason.clone())))
            }));
    }
    root.into_element()
}
fn tour(
    guide: Rc<PullRequestReviewGuide>,
    markdown: PrMarkdown,
    client: PullRequestReviewSession,
    open_code: impl Fn() + Copy + 'static,
) -> AnyElement {
    let excerpts = excerpt_pages(client, &guide.stops);
    let position = signal(0usize);
    dynamic(move || {
        let i = position.get();
        let Some(stop) = guide.stops.get(i) else {
            return text(t("This change has no files to tour.")).into_element();
        };
        let total = guide.stops.len();
        let mut page = view()
            .app_class(tw("flex min-w-0 flex-col gap-2"))
            .child(text(tr(
                "Stop {index} of {count}",
                &[
                    ("index", (i as i64 + 1).into()),
                    ("count", (total as i64).into()),
                ],
            )))
            .child(
                view()
                    .app_class(tw("flex flex-row gap-2"))
                    .child(
                        nsh_base::button()
                            .accessibility_label(t("Previous stop"))
                            .child(text(t("Previous stop")))
                            .disabled(i == 0)
                            .on_press(move || position.set(i.saturating_sub(1))),
                    )
                    .child(
                        nsh_base::button()
                            .accessibility_label(t("Next stop"))
                            .child(text(t("Next stop")))
                            .disabled(i + 1 >= total)
                            .on_press(move || position.set((i + 1).min(total - 1))),
                    ),
            )
            .child(markdown.render(&stop.title.0))
            .child(markdown.render(&stop.summary));
        if let Some(attention) = &stop.attention {
            use PullRequestReviewGuideAttentionKind::*;
            page = page
                .child(text(t(match attention.kind {
                    Tradeoff => "A tradeoff to weigh",
                    Contract => "A contract to check",
                    Product => "A product call",
                    Risk => "A risk to judge",
                    Accessibility => "An accessibility call",
                })))
                .child(markdown.render(&attention.note.0));
        }
        for (j, excerpt) in stop.excerpts.iter().enumerate() {
            let path = excerpt.path.0.clone();
            let (start, end) = (excerpt.start, excerpt.end);
            page = page
                .child(markdown.render(&excerpt.caption))
                .child(dynamic(move || match excerpts.get() {
                    None => text(tr("Loading {path}...", &[("path", path.clone().into())]))
                        .into_element(),
                    Some(snippets) => match snippets.get(&(i, j)) {
                        Some(snippet) => view()
                            .child(text(format!("{path} · L{start}–{end}")))
                            .child(text(snippet.clone()).app_class(tw("font-mono text-xs")))
                            .into_element(),
                        None => view()
                            .child(text(tr(
                                "{path} lines {start}-{end} are not in the loaded diff.",
                                &[
                                    ("path", path.clone().into()),
                                    ("start", start.to_string().into()),
                                    ("end", end.to_string().into()),
                                ],
                            )))
                            .child(button(t("Open in Code"), open_code))
                            .into_element(),
                    },
                }));
        }
        for note in &stop.notes {
            page = page
                .child(text(format!("{} · L{}", note.path.0, note.line)))
                .child(markdown.render(&note.title.0))
                .child(markdown.render(&note.body));
        }
        let other_files = stop
            .files
            .iter()
            .filter(|f| !stop.excerpts.iter().any(|e| e.path == f.path));
        if other_files.clone().next().is_some() {
            page = page.child(text(t(if stop.excerpts.is_empty() {
                "Files in this stop"
            } else {
                "Also in this stop"
            })));
        }
        for file in other_files {
            page = page.child(text(file.path.0.clone()));
            if let Some(note) = &file.note {
                page = page.child(markdown.render(note));
            }
        }
        if !stop.questions.is_empty() {
            page = page.child(text(t("Questions to consider")));
        }
        for question in &stop.questions {
            page = page.child(markdown.render(&question.0));
        }
        page.into_element()
    })
    .into_element()
}

type Snippets = BTreeMap<(usize, usize), String>;

fn excerpt_pages(
    client: PullRequestReviewSession,
    stops: &[PullRequestReviewGuideStop],
) -> Signal<Option<Rc<Snippets>>> {
    let snippets = signal(None);
    let mut requested = BTreeMap::<String, Vec<_>>::new();
    for (stop, data) in stops.iter().enumerate() {
        for (index, excerpt) in data.excerpts.iter().enumerate() {
            requested
                .entry(excerpt.path.0.clone())
                .or_default()
                .push(((stop, index), excerpt.clone()));
        }
    }
    if requested.is_empty() {
        return snippets;
    }
    watch(async move {
        let mut result = Snippets::new();
        let mut visited = BTreeSet::new();
        let mut cursor = None;
        while let Ok(page) = client.diff(cursor.take()).await {
            for file in parse_patch_files(&page.patch) {
                let Some(excerpts) = requested.remove(file.path()) else {
                    continue;
                };
                let metadata = file_diff_metadata(&page.patch[file.range]);
                for (key, excerpt) in excerpts {
                    if let Some(content) =
                        excerpt_text(&metadata, &excerpt.side, excerpt.start, excerpt.end)
                    {
                        result.insert(key, content);
                    }
                }
            }
            if requested.is_empty() {
                break;
            }
            let Some(next) = page.next_cursor else { break };
            if !visited.insert(next.0.clone()) {
                break;
            }
            cursor = Some(next);
        }
        snippets.set(Some(Rc::new(result)));
    });
    snippets
}
