//! `/pair`: connect to a server with a pairing link, or a host and a pairing code.
//!
//! Input validation and its messages are the client's (`resolve_pairing_target`); this screen
//! only collects the fields and shows the outcome.

use nsh_core::prelude::*;
use t3_client::connection::PairingInput;

use crate::i18n::t;
use crate::runtime::demo::DEMO_PAIRING_URL;
use crate::runtime::rt;
use crate::state::nav::{Route, navigator};
use crate::theme::palette;
use crate::ui::button::{ButtonContent, ButtonSize, ButtonVariant, button};
use crate::ui::input::{InputSize, text_field};
use crate::ui::surface::card;
use crate::ui::text::{TextRole, Tone, label};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Method {
    Link,
    Code,
}

#[derive(Clone, Debug, PartialEq)]
enum Attempt {
    Idle,
    Connecting,
    Failed(String),
}

fn pairing_input(method: Method, link: &str, host: &str, code: &str) -> PairingInput {
    match method {
        Method::Link => PairingInput {
            pairing_url: Some(link.trim().to_owned()),
            ..PairingInput::default()
        },
        Method::Code => PairingInput {
            host: Some(host.trim().to_owned()),
            pairing_code: Some(code.trim().to_owned()),
            ..PairingInput::default()
        },
    }
}

pub fn screen() -> AnyElement {
    let method = signal(Method::Link);
    let link = signal(String::new());
    let host = signal(String::new());
    let code = signal(String::new());
    let attempt = signal(Attempt::Idle);
    let connecting = move || attempt.with(|a| *a == Attempt::Connecting);
    let nav = navigator();
    let runtime = rt();

    let submit = move || {
        if connecting() {
            return;
        }
        let input = pairing_input(
            method.get_untracked(),
            &link.get_untracked(),
            &host.get_untracked(),
            &code.get_untracked(),
        );
        attempt.set(Attempt::Connecting);
        let backend = runtime.backend();
        spawn(async move {
            match backend.pair(input).await {
                Ok(_) => {
                    attempt.set(Attempt::Idle);
                    nav.open(Route::Home);
                }
                Err(failure) => attempt.set(Attempt::Failed(failure.detail)),
            }
        });
    };

    let fields = view()
        .gap(px(12.0))
        .child(when(
            move || method.get() == Method::Link,
            move || {
                text_field(
                    link,
                    t("https://host/pair#token=…"),
                    t("Pairing link"),
                    InputSize::Lg,
                    submit,
                )
            },
        ))
        .child(when(
            move || method.get() == Method::Code,
            move || {
                view()
                    .gap(px(12.0))
                    .child(text_field(
                        host,
                        t("host:port"),
                        t("Server address"),
                        InputSize::Lg,
                        submit,
                    ))
                    .child(text_field(
                        code,
                        t("Pairing code"),
                        t("Pairing code"),
                        InputSize::Lg,
                        submit,
                    ))
            },
        ));

    let error = dynamic(move || {
        attempt.with(|a| match a {
            Attempt::Failed(detail) => {
                let detail = t(detail);
                Some(label(TextRole::Caption, Tone::Destructive, move || {
                    detail.clone()
                }))
            }
            _ => None,
        })
    });

    let actions = view()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap(px(8.0))
        .child(button(
            ButtonContent::Label(t("Connect")),
            ButtonVariant::Default,
            ButtonSize::Default,
            connecting,
            submit,
        ))
        .child(button(
            ButtonContent::Label(t("Use a pairing code")),
            ButtonVariant::Ghost,
            ButtonSize::Default,
            || false,
            move || {
                method.update(|m| {
                    *m = match m {
                        Method::Link => Method::Code,
                        Method::Code => Method::Link,
                    }
                })
            },
        ))
        .child(button(
            ButtonContent::Label(t("Fill in the demo server")),
            ButtonVariant::GhostMuted,
            ButtonSize::Default,
            || false,
            move || {
                method.set(Method::Link);
                link.set(DEMO_PAIRING_URL.to_owned());
            },
        ));

    let status = dynamic(move || {
        connecting().then(|| label(TextRole::Caption, Tone::Muted, || t("Connecting…")))
    });

    view()
        .flex_1()
        .w_full()
        .items_center()
        .justify_center()
        .p(px(16.0))
        .bg(move || palette().background)
        .child(
            view().w_full().max_w(px(440.0)).gap(px(16.0)).child(card(
                view()
                    .gap(px(16.0))
                    .child(
                        view()
                            .gap(px(4.0))
                            .child(label(TextRole::Title, Tone::Default, || t("Connect to T3 Code")))
                            .child(label(TextRole::Caption, Tone::Muted, || {
                                t("Paste the pairing link your server printed, or enter its address and a pairing code.")
                            })),
                    )
                    .child(fields)
                    .child(error)
                    .child(status)
                    .child(actions),
            )),
        )
        .into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use t3_client::connection::resolve_pairing_target;

    #[test]
    fn the_link_method_sends_only_the_trimmed_link() {
        let input = pairing_input(
            Method::Link,
            "  https://h.test/pair#token=abc ",
            "ignored",
            "ignored",
        );
        assert_eq!(
            input.pairing_url.as_deref(),
            Some("https://h.test/pair#token=abc")
        );
        assert!(input.host.is_none() && input.pairing_code.is_none());
        let target = resolve_pairing_target(&input)
            .map_err(|f| f.detail)
            .expect("valid");
        assert_eq!(target.http_base, "https://h.test/");
    }

    #[test]
    fn the_code_method_sends_host_and_code_and_the_client_validates_them() {
        let input = pairing_input(Method::Code, "ignored", " h.test:3773 ", " 1234 ");
        assert_eq!(input.pairing_url, None);
        assert_eq!(input.host.as_deref(), Some("h.test:3773"));
        assert_eq!(input.pairing_code.as_deref(), Some("1234"));
        let empty = pairing_input(Method::Code, "", "h.test", "  ");
        let error = resolve_pairing_target(&empty).err().map(|f| f.detail);
        assert_eq!(error.as_deref(), Some("Enter a pairing code."));
    }
}
