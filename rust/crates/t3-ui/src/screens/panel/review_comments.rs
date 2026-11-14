//! Port of apps/web/src/components/diffs/DiffCommentAnnotation.tsx DiffCommentAnnotation and the
//! annotation half of AnnotatableCodeView.tsx (beginComment, submitEntry, removeEntry).
//!
//! The view's lines are NSH `DiffView` annotations: `render_add_annotation` is the gutter's add
//! control, `render_annotation` draws the draft and the saved comments under their lines. The
//! comment itself (its range, label and quoted diff) is built by the client
//! (`workspace::diff_panel::comments`) and saved in the composer draft, as the web saves it.

use std::rc::Rc;

use nsh_base::{DiffAnnotation, DiffLineRange, DiffSide};
use nsh_core::prelude::*;
use t3_client::{
    types::ThreadKey,
    workspace::diff_panel::{
        DiffPanelSnapshot,
        comments::{CommentAnnotation, Side},
    },
};

use super::diff::DiffPanelRuntime;
use crate::i18n::{Value, t, tr};
use crate::runtime::Shared;
use crate::theme::tw;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::icon::{Icon, icon_with_class};

/// The comment being written: its lines and the annotation that holds its form.
#[derive(Clone, Debug, PartialEq)]
pub struct CommentDraft {
    pub side: Side,
    pub lines: (usize, usize),
    pub annotation: CommentAnnotation,
}

/// The draft and its text, kept by the panel while the view's rows recycle.
#[derive(Clone, Copy)]
pub struct ReviewComments {
    pub draft: Signal<Option<CommentDraft>>,
    pub text: Signal<String>,
    pub error: Signal<Option<String>>,
}

impl ReviewComments {
    pub fn new() -> Self {
        Self {
            draft: signal(None),
            text: signal(String::new()),
            error: signal(None),
        }
    }

    fn clear(self) {
        self.draft.set(None);
        self.text.set(String::new());
        self.error.set(None);
    }
}

impl Default for ReviewComments {
    fn default() -> Self {
        Self::new()
    }
}

fn side_of(side: DiffSide) -> Side {
    match side {
        DiffSide::Old => Side::Old,
        DiffSide::New => Side::New,
    }
}

fn diff_side(side: Side) -> DiffSide {
    match side {
        Side::Old => DiffSide::Old,
        Side::New => DiffSide::New,
    }
}

/// The view's annotations: the saved comments, then the draft (`items[].annotations`).
pub fn view_annotations(
    snapshot: Option<&DiffPanelSnapshot>,
    draft: Option<&CommentDraft>,
) -> Vec<DiffAnnotation> {
    let mut annotations: Vec<DiffAnnotation> = snapshot
        .map(|s| s.annotations.iter().collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(draft.map(|d| &d.annotation))
        .map(|a| {
            let mut annotation = DiffAnnotation::default();
            annotation.id = SharedString::from(a.id.clone());
            annotation.side = diff_side(a.side);
            annotation.line = a.line as u32;
            annotation
        })
        .collect();
    annotations.dedup_by(|a, b| a.id == b.id);
    annotations
}

/// The gutter's add control (`enableGutterUtility`), absent while a draft is open.
pub fn add_comment_control(
    runtime: DiffPanelRuntime,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    comments: ReviewComments,
    range: DiffLineRange,
) -> impl IntoElement {
    dynamic(move || {
        if comments.draft.get().is_some() {
            return None;
        }
        let runtime = runtime.clone();
        let side = side_of(range.side);
        let lines = (range.start as usize, range.end as usize);
        Some(button(
            ButtonContent::Icon {
                icon: Icon::Plus,
                label: t("Add comment"),
            },
            ButtonVariant::Default,
            ButtonSize::IconXs,
            || false,
            move || {
                let Some(s) = snapshot.get_untracked() else {
                    return;
                };
                if let Some(annotation) = runtime.0.draft_comment(&s.0, side, lines) {
                    comments.text.set(String::new());
                    comments.error.set(None);
                    comments.draft.set(Some(CommentDraft {
                        side,
                        lines,
                        annotation,
                    }));
                }
            },
        ))
    })
}

/// `renderAnnotation`: the draft's form, or a saved comment.
pub fn comment_annotation(
    runtime: DiffPanelRuntime,
    key: ThreadKey,
    snapshot: Signal<Option<Shared<DiffPanelSnapshot>>>,
    comments: ReviewComments,
    annotation: DiffAnnotation,
) -> AnyElement {
    let id = annotation.id.to_string();
    if let Some(draft) = comments
        .draft
        .get_untracked()
        .filter(|d| d.annotation.id == id)
    {
        let submit_runtime = runtime.clone();
        let submit = Rc::new(move || {
            let text = comments.text.get_untracked();
            if text.trim().is_empty() {
                return;
            }
            let Some(s) = snapshot.get_untracked() else {
                return;
            };
            match submit_runtime
                .0
                .add_review_comment(&key, &s.0, draft.side, draft.lines, &text)
            {
                Ok(()) => comments.clear(),
                Err(error) => comments.error.set(Some(error.to_string())),
            }
        });
        return view()
            .app_class(tw("py-1"))
            .child(diff_comment_draft(
                draft.annotation.range_label.clone(),
                comments,
                move || comments.clear(),
                move || submit(),
            ))
            .into_element();
    }
    let saved = snapshot.with_untracked(|s| {
        s.as_ref()
            .and_then(|s| s.annotations.iter().find(|a| a.id == id).cloned())
    });
    let Some(saved) = saved else {
        return view().into_element();
    };
    view()
        .app_class(tw("border-y border-border/30"))
        .child(diff_comment(saved.text.clone(), move || {
            if let Err(error) = runtime.0.remove_review_comment(&key, &id) {
                comments.error.set(Some(error.to_string()));
            }
        }))
        .into_element()
}

/// DiffCommentAnnotation in its `comment` kind. The delete button shows at rest: NSH has no
/// group hover reveal for a nested button, and the web shows it at rest below `sm` too.
fn diff_comment(text_value: String, on_delete: impl Fn() + 'static) -> impl IntoElement {
    view()
        .app_class(tw(
            "flex flex-row min-w-0 items-start gap-2.5 border-s-2 border-primary/55 bg-primary/[0.045] px-3 py-2.5 font-sans text-foreground",
        ))
        .child(icon_with_class(
            Icon::MessageCircle,
            "size-3.5 h-5 shrink-0 text-primary/70",
        ))
        .child(
            text(text_value)
                .select_text()
                .app_class(tw("min-w-0 flex-1 text-sm leading-5")),
        )
        .child(
            view().app_class(tw("-my-1 -mr-1 flex shrink-0")).child(button(
                ButtonContent::Custom {
                    label: t("Delete comment"),
                    child: icon_with_class(Icon::Trash2, "size-3").into_element(),
                },
                ButtonVariant::GhostMuted,
                ButtonSize::IconXs,
                || false,
                on_delete,
            )),
        )
}

/// DiffCommentAnnotation in its `draft` kind: the comment field, Cancel and Comment. Dictation
/// (SpeechTextField) and `@` completion belong to the voice and pull-request lanes.
fn diff_comment_draft(
    range_label: String,
    comments: ReviewComments,
    on_cancel: impl Fn() + 'static,
    on_comment: impl Fn() + 'static,
) -> impl IntoElement {
    let on_comment = Rc::new(on_comment);
    let comment = on_comment.clone();
    view()
        .app_class(tw("px-3 py-2 font-sans text-foreground"))
        .child(
            nsh_base::input()
                .label(tr(
                    "Comment on lines {range}",
                    &[("range", Value::Text(range_label))],
                ))
                .multiline(true)
                .auto_focus(true)
                .value(move || comments.text.get())
                .on_change(move |value: &str| comments.text.set(value.to_owned()))
                .placeholder(t("Add a comment…"))
                .app_class(tw(
                    "min-h-16 w-full rounded-md border border-input bg-background px-3 py-2 text-sm text-foreground",
                )),
        )
        .child(dynamic(move || {
            comments
                .error
                .get()
                .map(|error| text(error).app_class(tw("mt-1 text-2xs text-error/80")))
        }))
        .child(
            view()
                .app_class(tw("mt-1.5 flex flex-row flex-wrap items-center gap-1"))
                // The ⌘/Ctrl Enter hint and shortcut wait on a key seam for multiline inputs.
                .child(view().app_class(tw("mr-auto")))
                .child(button(
                    ButtonContent::Label(t("Cancel")),
                    ButtonVariant::GhostMuted,
                    ButtonSize::Xs,
                    || false,
                    on_cancel,
                ))
                .child(button(
                    ButtonContent::Label(t("Comment")),
                    ButtonVariant::Default,
                    ButtonSize::Xs,
                    move || comments.text.with(|t| t.trim().is_empty()),
                    move || comment(),
                )),
        )
}
