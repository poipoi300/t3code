#![allow(clippy::unwrap_used)]
use futures::executor::block_on;
use serde_json::json;
use std::collections::BTreeSet;
use t3_client::{
    types::{EnvironmentId, ThreadId, ThreadKey},
    view_logic::shell::{closed_views::*, panels::*},
};
fn key(e: &str, t: &str) -> ThreadKey {
    ThreadKey {
        environment: EnvironmentId(e.into()),
        thread: ThreadId(t.into()),
    }
}
fn current<'a>(s: &'a PanelStore, k: &ThreadKey) -> &'a Panel {
    &s.by_thread[k]
}
fn ids(s: &Panel) -> Vec<&str> {
    s.surfaces.iter().map(|s| s.id.as_str()).collect()
}
fn file(s: &mut PanelStore, k: &ThreadKey, path: &str) {
    s.apply(
        k,
        Action::OpenFile {
            path: path.into(),
            line: None,
        },
    );
}
fn device(host: &str) -> DeviceTarget {
    DeviceTarget {
        host_id: host.into(),
        device_id: "same:id".into(),
        platform: "ios".into(),
        name: "Phone".into(),
    }
}
#[test]
fn hosted_manual_actions_follow_the_latest_lease_but_reconciliation_is_local() {
    let mut s = PanelStore::default();
    let parent = key("e", "parent");
    let child = key("e", "child");
    let old = s.host(child.clone(), parent.clone());
    let lease = s.host(child.clone(), parent.clone());
    s.unhost(&child, old);
    file(&mut s, &child, "a.ts");
    assert_eq!(ids(current(&s, &parent)), ["file:a.ts"]);
    assert!(!s.by_thread.contains_key(&child));
    s.apply(
        &child,
        Action::ReconcileBrowsers {
            tabs: vec!["b".into()],
            hidden: BTreeSet::new(),
        },
    );
    assert_eq!(ids(current(&s, &child)), ["browser:b"]);
    s.unhost(&child, lease);
    file(&mut s, &child, "b.ts");
    assert_eq!(ids(current(&s, &child)), ["browser:b", "file:b.ts"]);
}
#[test]
fn close_history_excludes_incidental_replacements_session_tabs_and_panel_hiding() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    s.apply(&k, Action::Open("files".into()));
    assert!(
        s.apply(
            &k,
            Action::OpenFile {
                path: "a".into(),
                line: None
            }
        )
        .is_empty()
    );
    s.apply(&k, Action::OpenSurface(Surface::terminal("term")));
    s.apply(&k, Action::OpenBrowser(Some("tab".into())));
    assert!(s.apply(&k, Action::Close).is_empty());
    assert_eq!(current(&s, &k).surfaces.len(), 3);
    assert!(
        s.apply(&k, Action::CloseSurface("terminal:term".into()))
            .is_empty()
    );
    assert!(
        s.apply(&k, Action::CloseSurface("browser:tab".into()))
            .is_empty()
    );
    s.apply(&k, Action::OpenBrowser(None));
    let closed = s.apply(&k, Action::CloseAll);
    assert_eq!(
        ids(&Panel {
            surfaces: closed.clone(),
            ..Default::default()
        }),
        ["file:a", "browser:new"]
    );
    let mut history = ClosedViews::default();
    for (i, surface) in closed.into_iter().enumerate() {
        history.remember(
            i.to_string(),
            ClosedView::PanelTab {
                thread_ref: serde_json::from_value(json!({"environmentId":"e","threadId":"t"}))
                    .unwrap(),
                surface,
            },
        );
    }
    assert!(
        matches!(&history.entries[0].view,ClosedView::PanelTab{surface,..}if surface.id=="browser:new")
    );
}
#[test]
fn bulk_close_reopens_active_first_and_neighbor_activation_is_stable() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    for p in ["a", "b", "c"] {
        file(&mut s, &k, p);
    }
    s.apply(&k, Action::Activate("file:b".into()));
    let closed = s.apply(&k, Action::CloseAll);
    assert_eq!(
        closed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["file:a", "file:c", "file:b"]
    );
    for p in ["a", "b", "c"] {
        file(&mut s, &k, p);
    }
    s.apply(&k, Action::CloseSurface("file:c".into()));
    assert_eq!(current(&s, &k).active_surface_id.as_deref(), Some("file:b"));
    s.apply(&k, Action::CloseSurface("file:a".into()));
    assert_eq!(current(&s, &k).active_surface_id.as_deref(), Some("file:b"));
    file(&mut s, &k, "c");
    s.apply(&k, Action::CloseRight("file:b".into()));
    assert_eq!(current(&s, &k).active_surface_id.as_deref(), Some("file:b"));
    file(&mut s, &k, "d");
    s.apply(&k, Action::CloseOthers("file:d".into()));
    assert_eq!(ids(current(&s, &k)), ["file:d"]);
}
#[test]
fn device_identity_renaming_and_dismissal_survive_other_panel_changes() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    s.apply(&k, Action::Open("device".into()));
    s.apply(
        &k,
        Action::OpenDevice {
            target: device("a"),
            automatic: true,
        },
    );
    let a = current(&s, &k).surfaces[0].id.clone();
    s.apply(
        &k,
        Action::RenameDevice {
            surface: a.clone(),
            title: " My Phone ".into(),
        },
    );
    s.apply(
        &k,
        Action::OpenDevice {
            target: device("b"),
            automatic: true,
        },
    );
    assert_eq!(current(&s, &k).surfaces.len(), 2);
    s.apply(
        &k,
        Action::OpenDevice {
            target: device("a"),
            automatic: false,
        },
    );
    assert!(
        matches!(&current(&s,&k).selected().unwrap().content,SurfaceContent::Device{title:Some(t),..}if t=="My Phone")
    );
    s.apply(&k, Action::CloseSurface(a.clone()));
    file(&mut s, &k, "a");
    s.apply(
        &k,
        Action::OpenDevice {
            target: device("a"),
            automatic: true,
        },
    );
    assert!(!ids(current(&s, &k)).contains(&a.as_str()));
    s.apply(
        &k,
        Action::OpenDevice {
            target: device("a"),
            automatic: false,
        },
    );
    assert!(ids(current(&s, &k)).contains(&a.as_str()));
}
#[test]
fn proactive_offers_observe_manual_revision_and_each_completed_turn_once() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    let other = key("remote", "t");
    let diff = Surface::singleton("diff").unwrap();
    let expected = s.user_revision(&k);
    assert!(s.open_proactive(&k, diff.clone(), Some("run1"), expected, true));
    s.apply(&k, Action::Close);
    assert!(!s.open_proactive(&k, diff.clone(), Some("run1"), s.user_revision(&k), true));
    let s2 = PanelStore::decode(&s.encode().unwrap()).unwrap();
    assert_eq!(
        current(&s2, &k).proactive_diff_turn_id.as_deref(),
        Some("run1")
    );
    file(&mut s, &other, "b");
    let rev = s.user_revision(&k);
    s.apply(&k, Action::Maximize(true));
    assert_eq!(s.user_revision(&k), rev);
    s.apply(
        &k,
        Action::ReconcileBrowsers {
            tabs: vec![],
            hidden: BTreeSet::new(),
        },
    );
    assert_eq!(s.user_revision(&k), rev);
    assert!(!s.open_proactive(&k, diff.clone(), None, expected, true));
    assert!(s.open_proactive(&k, diff.clone(), Some("run2"), rev, true));
    file(&mut s, &k, "manual");
    assert!(s.open_proactive(&k, diff.clone(), Some("run3"), s.user_revision(&k), true));
    assert!(!s.open_proactive(&k, diff, Some("run4"), s.user_revision(&k), true));
}
#[test]
fn visibility_promotes_popover_on_close_and_only_inline_preference_persists() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    s.apply(
        &k,
        Action::SetThreadPanel {
            inline: false,
            open: true,
        },
    );
    s.apply(
        &k,
        Action::SetThreadPanel {
            inline: true,
            open: false,
        },
    );
    file(&mut s, &k, "a");
    assert_eq!(
        s.visibility[&k],
        ThreadPanelVisibility {
            inline_open: false,
            popover_open: false
        }
    );
    s.apply(
        &k,
        Action::SetThreadPanel {
            inline: false,
            open: true,
        },
    );
    s.apply(&k, Action::Close);
    assert_eq!(
        s.visibility[&k],
        ThreadPanelVisibility {
            inline_open: true,
            popover_open: true
        }
    );
    s.apply(
        &k,
        Action::SetThreadPanel {
            inline: true,
            open: false,
        },
    );
    s.apply(&k, Action::Maximize(true));
    let restored = PanelStore::decode(&s.encode().unwrap()).unwrap();
    assert_eq!(
        restored.visibility[&k],
        ThreadPanelVisibility {
            inline_open: false,
            popover_open: false
        }
    );
    assert_eq!(current(&restored, &k).maximized, Some(true));
}
#[test]
fn folder_navigation_reuses_peers_keeps_trail_and_ignores_closed_source() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    file(&mut s, &k, ".");
    assert_eq!(ids(current(&s, &k)), ["files"]);
    s.apply(
        &k,
        Action::NavigateFolder {
            surface: "files".into(),
            path: "src".into(),
            trail: "src/lib".into(),
            keep_source: false,
        },
    );
    assert!(
        matches!(&current(&s,&k).selected().unwrap().content,SurfaceContent::File{folder_trail:Some(t),..}if t=="src/lib")
    );
    file(&mut s, &k, "src/lib/a.rs");
    s.apply(
        &k,
        Action::NavigateFolder {
            surface: "file:src/lib/a.rs".into(),
            path: "src".into(),
            trail: "src/lib/a.rs".into(),
            keep_source: true,
        },
    );
    assert_eq!(ids(current(&s, &k)), ["file:src", "file:src/lib/a.rs"]);
    assert_eq!(
        current(&s, &k).active_surface_id.as_deref(),
        Some("file:src")
    );
    s.apply(
        &k,
        Action::NavigateFolder {
            surface: "file:src".into(),
            path: String::new(),
            trail: "src/lib".into(),
            keep_source: false,
        },
    );
    assert_eq!(ids(current(&s, &k)), ["files", "file:src/lib/a.rs"]);
    let before = current(&s, &k).clone();
    s.apply(
        &k,
        Action::NavigateFolder {
            surface: "closed".into(),
            path: "other".into(),
            trail: "other".into(),
            keep_source: false,
        },
    );
    assert_eq!(current(&s, &k), &before);
}
#[test]
fn file_paths_line_reveals_and_attachment_ids_are_distinct() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    for (path, expected) in [
        ("folder///", "file:folder"),
        ("/", "file:/"),
        ("C:///", "file:C:///"),
    ] {
        file(&mut s, &k, path);
        assert_eq!(current(&s, &k).selected().unwrap().id, expected);
    }
    s.apply(
        &k,
        Action::OpenFile {
            path: "a".into(),
            line: Some(2.9),
        },
    );
    s.apply(
        &k,
        Action::OpenFile {
            path: "a".into(),
            line: Some(-2.0),
        },
    );
    assert!(matches!(
        current(&s, &k).selected().unwrap().content,
        SurfaceContent::File {
            reveal_line: Some(1),
            reveal_request_id: 2,
            ..
        }
    ));
    let attachment=serde_json::from_value(json!({"id":"id","type":"file","name":"a","mimeType":"text/plain","sizeBytes":1,"attachmentId":"uploaded"})).unwrap();
    s.apply(&k, Action::OpenAttachment(attachment));
    assert!(ids(current(&s, &k)).contains(&"file:a"));
    assert!(ids(current(&s, &k)).contains(&"attachment:id"));
    s.apply(
        &k,
        Action::ReconcileFiles {
            workspace_available: false,
        },
    );
    assert_eq!(ids(current(&s, &k)), ["attachment:id"]);
}
#[test]
fn pull_requests_are_scoped_reveals_settle_conditionally_and_detail_tab_survives_reload() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    let mut pr = Surface::pull_request(Some("e"), "p", Some("GitHub.COM"), "o/r", 42);
    let id = pr.id.clone();
    s.apply(&k, Action::OpenSurface(pr.clone()));
    s.apply(
        &k,
        Action::DetailTab {
            surface: id.clone(),
            tab: "files".into(),
        },
    );
    if let SurfaceContent::PullRequest { reveal_url, .. } = &mut pr.content {
        *reveal_url = Some("first".into());
    }
    s.apply(&k, Action::OpenSurface(pr.clone()));
    if let SurfaceContent::PullRequest { reveal_url, .. } = &mut pr.content {
        *reveal_url = Some("second".into());
    }
    s.apply(&k, Action::OpenSurface(pr));
    s.apply(
        &k,
        Action::SettleReveal {
            surface: id.clone(),
            url: "first".into(),
        },
    );
    assert!(
        matches!(&current(&s,&k).selected().unwrap().content,SurfaceContent::PullRequest{reveal_url:Some(u),detail_tab:Some(t),..}if u=="second"&&t=="files")
    );
    s.apply(
        &k,
        Action::OpenSurface(Surface::pull_request(
            Some("e"),
            "p",
            Some("enterprise"),
            "o/r",
            42,
        )),
    );
    assert_eq!(current(&s, &k).surfaces.len(), 2);
    let restored = PanelStore::decode(&s.encode().unwrap()).unwrap();
    assert!(
        matches!(&current(&restored,&k).surfaces[0].content,SurfaceContent::PullRequest{reveal_url:None,detail_tab:Some(t),host:Some(h),..}if t=="files"&&h=="github.com")
    );
    assert_ne!(
        id,
        Surface::pull_request(Some("remote"), "p", Some("github.com"), "o/r", 42).id
    );
}
#[test]
fn split_terminal_closes_final_surface_without_recording_session_metadata() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    s.apply(&k, Action::OpenSurface(Surface::terminal("one")));
    s.apply(
        &k,
        Action::SplitTerminal {
            surface: "terminal:one".into(),
            terminal: "two".into(),
            vertical: true,
        },
    );
    s.apply(
        &k,
        Action::ActivateTerminal {
            surface: "terminal:one".into(),
            terminal: "one".into(),
        },
    );
    assert!(
        matches!(&current(&s,&k).selected().unwrap().content,SurfaceContent::Terminal{terminal_ids,active_terminal_id,split_direction:Some(d),..}if terminal_ids==&["one","two"]&&active_terminal_id=="one"&&d=="vertical")
    );
    assert!(
        s.apply(
            &k,
            Action::CloseTerminal {
                surface: "terminal:one".into(),
                terminal: "one".into()
            }
        )
        .is_empty()
    );
    assert!(
        s.apply(
            &k,
            Action::CloseTerminal {
                surface: "terminal:one".into(),
                terminal: "two".into()
            }
        )
        .is_empty()
    );
    assert!(!s.by_thread.contains_key(&k));
}
#[test]
fn browser_reconciliation_preserves_explicit_hidden_tabs_and_manual_order() {
    let mut s = PanelStore::default();
    let k = key("e", "t");
    s.apply(&k, Action::OpenBrowser(None));
    s.apply(
        &k,
        Action::ReconcileBrowsers {
            tabs: vec!["a".into(), "b".into()],
            hidden: BTreeSet::from(["b".into()]),
        },
    );
    assert_eq!(ids(current(&s, &k)), ["browser:a"]);
    s.apply(&k, Action::OpenBrowser(Some("b".into())));
    s.apply(
        &k,
        Action::Move {
            surface: "browser:b".into(),
            to: 0,
        },
    );
    s.apply(
        &k,
        Action::ReconcileBrowsers {
            tabs: vec!["a".into(), "b".into()],
            hidden: BTreeSet::from(["b".into()]),
        },
    );
    assert_eq!(ids(current(&s, &k)), ["browser:b", "browser:a"]);
    s.apply(
        &k,
        Action::ReconcileBrowsers {
            tabs: vec![],
            hidden: BTreeSet::new(),
        },
    );
    assert!(s.by_thread.get(&k).is_none_or(|p| !p.is_open));
}
#[test]
fn persisted_migrations_drop_obsolete_views_and_upgrade_reference_tabs() {
    let raw=json!({"version":1,"state":{"byThreadKey":{"e:t":{"isOpen":true,"maximized":true,"activeSurfaceId":"pull-request","surfaces":[
 {"id":"terminal","kind":"terminal"},{"id":"terminal:one","kind":"terminal","resourceId":"one"},
 {"id":"plan","kind":"plan"},{"id":"agents","kind":"agents"},
 {"id":"pull-request","kind":"pull-request","projectId":"p","repository":"o/r","number":42,"detailTab":"files"},
 {"id":"file:a","kind":"file","relativePath":"a","revealLine":0,"revealRequestId":null,"folderTrail":42}
 ]},"e:pull-requests-panel":{"isOpen":true,"surfaces":[{"id":"diff","kind":"diff"}]}}}}).to_string();
    let s = PanelStore::decode(&raw).unwrap();
    let k = key("e", "t");
    let p = current(&s, &k);
    assert_eq!(p.surfaces.len(), 3);
    assert_eq!(p.maximized, Some(true));
    assert!(
        p.active_surface_id
            .as_ref()
            .unwrap()
            .starts_with("pull-request:")
    );
    assert!(
        matches!(&p.surfaces[0].content,SurfaceContent::Terminal{terminal_ids,active_terminal_id,..}if terminal_ids==&["one"]&&active_terminal_id=="one")
    );
    assert!(matches!(
        p.surfaces[2].content,
        SurfaceContent::File {
            reveal_line: Some(1),
            reveal_request_id: 0,
            folder_trail: None,
            ..
        }
    ));
    assert!(!s.by_thread.contains_key(&key("e", "pull-requests-panel")));
}
#[test]
fn migration_preserves_future_surface_payloads_without_reopening_them_from_history() {
    let unknown = json!({"id":"future:one","kind":"future-kind","payload":{"data":[1,2,3]}});
    let raw=json!({"version":15,"state":{"byThreadKey":{"e:t":{"isOpen":true,"activeSurfaceId":"future:one","surfaces":[unknown]}}}}).to_string();
    let store = PanelStore::decode(&raw).unwrap();
    let encoded: serde_json::Value = serde_json::from_str(&store.encode().unwrap()).unwrap();
    assert_eq!(
        encoded["state"]["byThreadKey"]["e:t"]["surfaces"][0],
        unknown
    );
    let closed=json!({"state":{"entries":[{"id":"future","kind":"panel-tab","threadRef":{"environmentId":"e","threadId":"t"},"surface":unknown}]}}).to_string();
    assert!(ClosedViews::decode(&closed).unwrap().entries.is_empty());
}
#[test]
fn reopen_requires_workspace_and_browser_success_before_panel_mutation() {
    block_on(async {
        let k = key("e", "t");
        let thread_ref =
            serde_json::from_value(json!({"environmentId":"e","threadId":"t"})).unwrap();
        let view = ClosedView::PanelTab {
            thread_ref,
            surface: Surface::file("a", Some(12.0), 1),
        };
        let mut s = PanelStore::default();
        assert!(
            !reopen(&view, &mut s, false, async |_| Ok::<_, &str>(
                "unused".into()
            ))
            .await
            .unwrap()
        );
        assert!(s.by_thread.is_empty());
        assert!(
            reopen(&view, &mut s, true, async |_| Ok::<_, &str>(
                "unused".into()
            ))
            .await
            .unwrap()
        );
        assert_eq!(ids(current(&s, &k)), ["file:a"]);
        let browser=serde_json::from_value(json!({"kind":"browser","threadRef":{"environmentId":"e","threadId":"t"},"snapshot":{"threadId":"t","tabId":"old","navStatus":{"_tag":"Success","url":"http://localhost:3000/","title":"App"},"canGoBack":false,"canGoForward":false,"viewport":{"_tag":"freeform","width":800,"height":600},"profileId":"profile","updatedAt":"now"}})).unwrap();
        let before = current(&s, &k).clone();
        assert_eq!(
            reopen(&browser, &mut s, true, async |_| Err::<String, _>("failed")).await,
            Err("failed")
        );
        assert_eq!(current(&s, &k), &before);
        reopen(&browser, &mut s, true, async |snapshot| {
            assert_eq!(
                snapshot.profile_id.as_ref().and_then(Option::as_deref),
                Some("profile")
            );
            assert!(snapshot.viewport.is_some());
            Ok::<_, &str>("new".into())
        })
        .await
        .unwrap();
        assert_eq!(
            current(&s, &k).active_surface_id.as_deref(),
            Some("browser:new")
        );
    });
}
#[test]
fn history_privacy_validation_capacity_and_reopen_owner_guards() {
    let public = json!({"id":"one","kind":"browser","threadRef":{"environmentId":"e","threadId":"t"},"snapshot":{"threadId":"t","tabId":"tab","navStatus":{"_tag":"Idle"},"canGoBack":false,"canGoForward":false,"updatedAt":"now"}});
    let mut private = public.clone();
    private["id"] = json!("private");
    private["snapshot"]["tabId"] = json!("private-tab");
    private["snapshot"]["profileId"] =
        json!(t3_contracts::browser_profile::INCOGNITO_BROWSER_PROFILE_ID);
    let public: Entry = serde_json::from_value(public).unwrap();
    let private: Entry = serde_json::from_value(private).unwrap();
    let mut history = ClosedViews::default();
    history.remember(public.id.clone(), public.view.clone());
    history.remember(private.id.clone(), private.view);
    assert_eq!(history.entries.len(), 2);
    assert!(!history.encode().unwrap().contains("private"));
    assert_eq!(
        ClosedViews::decode(&history.encode().unwrap())
            .unwrap()
            .entries
            .len(),
        1
    );
    let restored =
        ClosedViews::decode(&json!({"state":{"entries":[{"id":"broken"},public]}}).to_string())
            .unwrap();
    assert_eq!(restored.entries.len(), 1);
    let panel = Panel::default();
    let owner = |_: &Entry| OwnerState {
        environment_known: false,
        catalog_ready: false,
        owner_exists: false,
        side_chat_exists: true,
        shell_live: false,
        panel: &panel,
    };
    assert!(plan_next_reopen(&restored.entries, owner).drop.is_empty());
    let gone = |_: &Entry| OwnerState {
        environment_known: false,
        catalog_ready: true,
        owner_exists: false,
        side_chat_exists: true,
        shell_live: false,
        panel: &panel,
    };
    assert_eq!(plan_next_reopen(&restored.entries, gone).drop.len(), 1);
    for i in 0..22 {
        history.remember(
            i.to_string(),
            ClosedView::PanelTab {
                thread_ref: serde_json::from_value(json!({"environmentId":"e","threadId":"t"}))
                    .unwrap(),
                surface: Surface::file(&i.to_string(), None, 0),
            },
        );
    }
    assert_eq!(history.entries.len(), 20);
    history.defer("21");
    assert_eq!(history.entries.last().unwrap().id, "21");
    history.remove("21");
    assert_eq!(history.entries.len(), 19);
}
