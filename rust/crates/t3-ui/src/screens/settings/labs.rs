//! LabsSettingsPanel uses client preferences for the sidebar and scoped server flags elsewhere.
use super::shell::{
    SettingsContext, SettingsRowProps, region, settings_page_container, settings_row,
    settings_section, use_settings,
};
use crate::{
    i18n::{Text, t},
    state::nav::SettingsPage,
    ui::{
        button::{ButtonContent, ButtonSize, ButtonVariant, button},
        switch::{ControlState, Size, switch},
    },
};
use nsh_core::prelude::*;
use serde_json::json;
use t3_client::view_logic::provider_readiness::{ManagedCapability, managed_capability_denial};

struct Flag {
    id: &'static str,
    key: &'static str,
    label: &'static str,
    description: &'static str,
    server_keys: &'static [&'static str],
    link: Option<(SettingsPage, &'static str)>,
}

const BORING_SIDEBAR: Flag = Flag {
    id: "boring-sidebar",
    key: "boringSidebarEnabled",
    label: "Boring sidebar (group by project)",
    description: "Group sidebar threads into project folders. Pinned threads stay on top, and Snoozed and Settled stay below the folders.",

    server_keys: &[],
    link: None,
};

const BORING_SIDEBAR_MULTILINE: Flag = Flag {
    id: "boring-sidebar-multiline",
    key: "boringSidebarMultilineRows",
    label: "Two-line folder rows",
    description: "Show folder threads on two lines: the title, then the branch, pull request, and agent.",
    server_keys: &[],
    link: None,
};

const GUEST_LINKS: Flag = Flag {
    id: "guest-links",
    key: "guestSharingEnabled",
    label: "Guest links",
    description: "Share a thread with a guest who watches it live in a browser and sends messages you approve. Needs T3 Connect: sign in and link this machine in Sharing & access. Turning it off stops every guest link.",

    server_keys: &["guestSharingEnabled"],
    link: Some((SettingsPage::Access, "Sharing & access")),
};

const LINKED_ENVIRONMENTS_BETA: Flag = Flag {
    id: "linked-environments-beta",
    key: "linkedEnvironmentsEnabled",
    label: "Linked environments",
    description: "Let agents here work on your other machines, and theirs here. Turn it on for each machine, then link them in Connections. While off, nothing new starts over a link; running tasks finish.",

    server_keys: &["linkedEnvironmentsEnabled"],
    link: Some((SettingsPage::Connections, "Connections")),
};

const PROMPT_SUGGESTIONS_BETA: Flag = Flag {
    id: "prompt-suggestions-beta",
    key: "promptSuggestionsEnabled",
    label: "Suggest next prompt",
    description: "Show a predicted next message in the empty composer after a Claude or Codex turn. Tab uses it. Each is an extra cached request on your plan. Needs Codex 0.165+. Claude threads update on restart.",

    server_keys: &["promptSuggestionsEnabled"],
    link: None,
};

const SIMLOCK_HOSTS: Flag = Flag {
    id: "simlock-hosts",
    key: "simlockDeviceHostsEnabled",
    label: "Simlock device hosts",
    description: "Let the Device panel lease simulators and emulators from your own Simlock pool, added in Devices. An alpha that may change or break. T3 Code never installs Simlock.",

    server_keys: &["simlockDeviceHostsEnabled"],
    link: Some((SettingsPage::Devices, "Devices")),
};

const WORKTREE_FAST_CHECKOUT: Flag = Flag {
    id: "worktree-fast-checkout",
    key: "fastWorktreeCheckout",
    label: "Fast worktree checkout",
    description: "On a Mac, clone new worktrees from a checkout cached in the T3 home folder instead of writing every file. Uses Git’s normal checkout when it can’t. Turning it off deletes the cache.",

    server_keys: &["fastWorktreeCheckout"],
    link: None,
};

fn flag(c: SettingsContext, f: Flag, denied: Option<String>) -> AnyElement {
    let mut props =
        SettingsRowProps::searchable(f.id).description(move || Text::from(f.description));
    if !f.server_keys.is_empty() {
        props = props.server(f.server_keys);
    }
    if let Some(reason) = denied {
        props = props.managed(Text::content(reason));
    }
    if let Some((page, label)) = f.link {
        props = props.children(move || {
            button(
                ButtonContent::Label(t(label)),
                ButtonVariant::Ghost,
                ButtonSize::Sm,
                || false,
                move || c.open_page(page, Default::default()),
            )
        });
    }
    settings_row(props.control(move || {
        switch(
            f.label,
            ControlState {
                checked: Prop::bind(move || !c.mixed(f.server_keys) && c.setting(f.key) == true),
                mixed: Prop::bind(move || !f.server_keys.is_empty() && c.mixed(f.server_keys)),
                ..Default::default()
            },
            Size::Default,
            move |checked| c.update(json!({f.key:checked})),
        )
    }))
    .into_element()
}

fn guest_links(c: SettingsContext) -> AnyElement {
    let signed_out = signal(false);
    if let Some(service) =
        use_context::<t3_client::services::settings::sessions::SettingsSessions>()
    {
        crate::runtime::tasks::watch(async move {
            signed_out.set(service.guest_sign_in_required().await);
        });
    }
    region(
        move || {
            let denial = if c.setting("guestSharingEnabled") == true {
                None
            } else {
                c.view().and_then(|scope| {
                    scope.selected.connected.iter().find_map(|e| {
                        managed_capability_denial(
                            e.config.as_ref()?.managed_policy.as_ref(),
                            ManagedCapability::Connect,
                        )
                    })
                })
            };
            (signed_out.get(), denial)
        },
        move |(signed_out, denial)| {
            if signed_out {
                return settings_row(
                    SettingsRowProps::searchable("guest-links")
                        .description(|| Text::from("Share a thread with a guest who watches it live in a browser and sends messages you approve. Sign in to T3 Connect to use guest links."))
                        .control(move || button(ButtonContent::Label(t("Sharing & access")), ButtonVariant::Outline, ButtonSize::Sm, || false, move || c.open_page(SettingsPage::Access, Default::default()))),
                )
                .into_element();
            }
            flag(c, GUEST_LINKS, denial)
        },
    )
}

pub fn screen() -> AnyElement {
    let c = use_settings();
    let multiline = region(
        move || c.setting("boringSidebarEnabled") == true,
        move |enabled| enabled.then(|| flag(c, BORING_SIDEBAR_MULTILINE, None)),
    );
    let fast_checkout = region(
        move || {
            c.view().is_some_and(|scope| {
                !scope.scope.is_project_scope()
                    && !scope.selected.connected.is_empty()
                    && scope.selected.connected.iter().all(|e| {
                        e.config.as_ref().is_some_and(|config| {
                            config.environment.capabilities.fast_worktree_checkout == Some(true)
                        })
                    })
            })
        },
        move |supported| supported.then(|| flag(c, WORKTREE_FAST_CHECKOUT, None)),
    );
    let rows = view()
        .child(flag(c, BORING_SIDEBAR, None))
        .child(multiline)
        .child(guest_links(c))
        .child(flag(c, LINKED_ENVIRONMENTS_BETA, None))
        .child(flag(c, PROMPT_SUGGESTIONS_BETA, None))
        .child(flag(c, SIMLOCK_HOSTS, None))
        .child(fast_checkout);
    settings_page_container(settings_section("beta", Text::from("Beta"), rows)).into_element()
}
