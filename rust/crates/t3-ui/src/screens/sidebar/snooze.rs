//! Port of CustomSnoozeDialog.tsx, using the client's date and duration validation.

use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use nsh_core::prelude::*;
use t3_client::thread_views::shelves::{
    resolve_custom_snooze_date, resolve_custom_snooze_duration,
};
use t3_contracts::settings::TimestampFormat;
use t3_contracts::wire::WireDateTime;
use t3_i18n::format::civil;

use crate::{
    i18n::{Text, t},
    screens::settings::snooze::{snooze_time_zone, with_snooze_calendar},
    theme::tw,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        calendar::calendar_day,
        dialog::{self, FooterVariant, MaxWidth, PopupOptions},
    },
};

pub(super) type Choice = LocalBoxFuture<'static, Option<WireDateTime>>;

#[derive(Clone, Copy)]
struct Request {
    open: Signal<bool>,
    reply: Stored<Option<oneshot::Sender<Option<WireDateTime>>>>,
}

pub(super) fn request() -> Option<Choice> {
    let request = use_context::<Request>()?;
    let (send, receive) = oneshot::channel();
    request.reply.replace(Some(send));
    request.open.set(true);
    Some(Box::pin(async move { receive.await.ok().flatten() }))
}

pub(super) fn dialog_host() -> impl IntoElement {
    let request = Request {
        open: signal(false),
        reply: stored(None),
    };
    provide_context(request);
    let close = move |value| {
        if let Some(send) = request.reply.with(Option::take) {
            let _ = send.send(value);
        }
        request.open.set(false);
    };
    dialog::dialog(request.open, move |state| {
        state.on_open_change(move |open| {
            if !open {
                close(None);
            }
        });
        dialog::dialog_popup(
            state,
            PopupOptions {
                max_width: MaxWidth::Md,
                ..Default::default()
            },
            move || {
                let initial = local_date_time(now() + 3_600_000);
                let date = signal(initial.0);
                let time = signal(initial.1);
                let amount = signal(String::from("2"));
                let unit = signal(3_600_000_u64);
                let duration = signal(false);
                let invalid = signal(false);
                let mode = |is_duration, label| {
                    nsh_base::button()
                        .accessibility_label(t(label))
                        .selected(move || duration.get() == is_duration)
                        .on_press(move || {
                            duration.set(is_duration);
                            invalid.set(false);
                        })
                        .app_class(tw("flex-1 rounded-md px-2 py-1 text-sm"))
                        .child(text(t(label)))
                };
                let select_unit = move |label, value| {
                    nsh_base::button()
                        .accessibility_label(t(label))
                        .selected(move || unit.get() == value)
                        .on_press(move || {
                            unit.set(value);
                            invalid.set(false);
                        })
                        .app_class(tw("flex-1 rounded-md px-2 py-1 text-sm"))
                        .child(text(t(label)))
                };
                view()
                    .app_class(tw("flex min-h-0 flex-col"))
                    .child(dialog::dialog_header(
                        state,
                        view()
                            .child(dialog::dialog_title(state, || Text::from("Custom snooze")))
                            .child(dialog::dialog_description(state, || {
                                Text::from("Choose when snoozed threads return to your inbox.")
                            })),
                    ))
                    .child(dialog::dialog_panel(
                        state,
                        view()
                            .app_class(tw("flex flex-col gap-4"))
                            .child(
                                view()
                                    .app_class(tw("flex w-full flex-row gap-2"))
                                    .child(mode(false, "Date and time"))
                                    .child(mode(true, "Duration")),
                            )
                            .child(
                                when(
                                    move || duration.get(),
                                    move || {
                                        view()
                                            .app_class(tw("flex flex-col gap-2"))
                                            .child(field("Snooze for", amount, invalid))
                                            .child(
                                                view()
                                                    .app_class(tw("flex flex-row gap-2"))
                                                    .child(select_unit("Minutes", 60_000))
                                                    .child(select_unit("Hours", 3_600_000))
                                                    .child(select_unit("Days", 86_400_000)),
                                            )
                                    },
                                )
                                .otherwise(move || {
                                    view()
                                        .app_class(tw("grid grid-cols-1 gap-3 sm:grid-cols-2"))
                                        .child(field("Date", date, invalid))
                                        .child(field("Time", time, invalid))
                                }),
                            )
                            .child(when(
                                move || invalid.get(),
                                move || {
                                    text(move || {
                                        if duration.get() {
                                            t("Enter a positive duration.")
                                        } else {
                                            t("Choose a valid date and time in the future.")
                                        }
                                    })
                                    .app_class(tw("text-sm text-destructive"))
                                },
                            )),
                    ))
                    .child(dialog::dialog_footer(
                        state,
                        FooterVariant::Default,
                        view()
                            .child(dialog::dialog_close(
                                state,
                                ButtonContent::Label(t("Cancel")),
                                ButtonVariant::Outline,
                                ButtonSize::Default,
                                || false,
                            ))
                            .child(button(
                                ButtonContent::Label(t("Snooze")),
                                ButtonVariant::Default,
                                ButtonSize::Default,
                                || false,
                                move || {
                                    let now = now();
                                    let wake = if duration.get_untracked() {
                                        resolve_custom_snooze_duration(
                                            &amount.get_untracked(),
                                            unit.get_untracked(),
                                            now,
                                        )
                                    } else {
                                        resolve_custom_snooze_date(
                                            &date.get_untracked(),
                                            &time.get_untracked(),
                                            now,
                                            |date, time| local_epoch(date, time, now),
                                            local_date_time,
                                        )
                                    };
                                    if let Some(wake) = wake {
                                        close(Some(WireDateTime(
                                            t3_client::time::iso_from_epoch_ms(wake as u64),
                                        )));
                                    } else {
                                        invalid.set(true);
                                    }
                                },
                            )),
                    ))
            },
        )
    })
}

fn field(label: &'static str, value: Signal<String>, invalid: Signal<bool>) -> impl IntoElement {
    view()
        .app_class(tw("flex min-w-0 flex-col gap-2"))
        .child(text(t(label)).app_class(tw("text-sm font-medium")))
        .child(
            nsh_base::input()
                .label(t(label))
                .value(move || value.get())
                .on_change(move |text: &str| {
                    value.set(text.into());
                    invalid.set(false);
                })
                .app_class(tw(
                    "h-9 min-w-0 rounded-md border border-input px-2 text-sm sm:h-8",
                )),
        )
}

fn now() -> i64 {
    nsh_core::time::unix_ms() as i64
}

fn local_date_time(at: i64) -> (String, String) {
    let local = civil(at, &snooze_time_zone(locale_settings().time_zone.as_str()));
    let day = local.day;
    (
        format!("{:04}-{:02}-{:02}", day.year, day.month, day.day),
        format!("{:02}:{:02}", local.hour, local.minute),
    )
}

fn local_epoch(date: &str, time: &str, now: i64) -> Option<i64> {
    let year = date.get(..4)?.parse().ok()?;
    let month = date.get(5..7)?.parse().ok()?;
    let day = date.get(8..)?.parse().ok()?;
    let hour = time.get(..2)?.parse().ok()?;
    let minute = time.get(3..)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let zone = snooze_time_zone(locale_settings().time_zone.as_str());
    let target = calendar_day(year, month, day);
    let today = civil(now, &zone).day;
    with_snooze_calendar(now, &TimestampFormat::Locale, |calendar| {
        (calendar.local_wake)(target.index - today.index, hour, minute)
    })
}
