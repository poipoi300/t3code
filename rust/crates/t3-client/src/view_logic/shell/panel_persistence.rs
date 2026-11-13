use super::*;
use serde_json::{Value, json};
fn migrate_surface(mut value: Value) -> Option<Surface> {
    let kind = value.get("kind")?.as_str()?.to_string();
    if matches!(kind.as_str(), "plan" | "agents") {
        return None;
    }
    if matches!(kind.as_str(), "file" | "files")
        && value.get("folderTrail").is_some_and(|v| !v.is_string())
    {
        value.as_object_mut()?.remove("folderTrail");
    }
    match kind.as_str() {
        "file" => {
            value["revealLine"] = value["revealLine"]
                .as_f64()
                .filter(|n| n.is_finite())
                .map_or(Value::Null, |n| json!(n.trunc().max(1.0) as i64));
            value["revealRequestId"] = json!(
                value["revealRequestId"]
                    .as_u64()
                    .filter(|n| *n <= 9_007_199_254_740_991)
                    .unwrap_or(0)
            );
        }
        "pull-request" => {
            let number = value["number"]
                .as_u64()
                .filter(|n| *n > 0 && *n <= 9_007_199_254_740_991)?;
            let mut surface = Surface::pull_request(
                value["environmentId"].as_str(),
                value["projectId"].as_str()?,
                value["host"].as_str(),
                value["repository"].as_str()?,
                number,
            );
            if let SurfaceContent::PullRequest {
                url, detail_tab, ..
            } = &mut surface.content
            {
                *url = value["url"].as_str().map(str::to_owned);
                *detail_tab = value["detailTab"].as_str().map(str::to_owned);
            }
            return Some(surface);
        }
        "terminal" => {
            let id = value["resourceId"].as_str()?.to_string();
            if value["id"].as_str() != Some(format!("terminal:{id}").as_str()) {
                return None;
            }
            let mut seen = BTreeSet::new();
            let mut ids = if let Some(raw) = value["terminalIds"].as_array() {
                raw.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| seen.insert(*s))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            } else {
                vec![id.clone()]
            };
            let active = value["activeTerminalId"]
                .as_str()
                .filter(|s| ids.iter().any(|id| id == s))
                .map(str::to_owned)
                .or_else(|| ids.first().cloned())
                .unwrap_or_else(|| id.clone());
            if ids.is_empty() {
                ids.push(id);
            }
            value["terminalIds"] = json!(ids);
            value["activeTerminalId"] = json!(active);
        }
        _ => {}
    }
    serde_json::from_value(value).ok()
}
impl PanelStore {
    pub fn decode(raw: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(raw)?;
        let state = value.get("state").unwrap_or(&value);
        let mut store = Self::default();
        for (key, value) in state["byThreadKey"].as_object().into_iter().flatten() {
            if key.ends_with(":pull-requests-panel") {
                continue;
            }
            let Some(key) = parse_legacy_thread_key(key) else {
                continue;
            };
            let surfaces = value["surfaces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|s| migrate_surface(s.clone()))
                .collect::<Vec<_>>();
            let raw_active = value["activeSurfaceId"].as_str();
            let active = surfaces
                .iter()
                .find(|s| Some(s.id.as_str()) == raw_active)
                .or_else(|| {
                    (raw_active == Some("pull-request"))
                        .then(|| surfaces.iter().find(|s| s.kind() == "pull-request"))
                        .flatten()
                })
                .map(|s| s.id.clone());
            let open =
                !surfaces.is_empty() && value["isOpen"].as_bool().unwrap_or(active.is_some());
            let active = active.or_else(|| {
                (open || raw_active == Some("agents"))
                    .then(|| surfaces.first().map(|s| s.id.clone()))
                    .flatten()
            });
            let panel = Panel {
                is_open: open,
                active_surface_id: active,
                surfaces,
                dismissed_device_surface_ids: value["dismissedDeviceSurfaceIds"].as_array().map(
                    |ids| {
                        ids.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    },
                ),
                maximized: (value["maximized"] == true).then_some(true),
                proactive_diff_turn_id: value["proactiveDiffTurnId"].as_str().map(str::to_owned),
            };
            store.by_thread.insert(key, panel);
        }
        for (key, value) in state["threadPanelVisibilityByThreadKey"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if value["inlineOpen"] == false
                && let Some(key) = parse_legacy_thread_key(key)
            {
                store.visibility.insert(
                    key,
                    ThreadPanelVisibility {
                        inline_open: false,
                        popover_open: false,
                    },
                );
            }
        }
        Ok(store)
    }
    pub fn encode(&self) -> Result<String, serde_json::Error> {
        let rows = self
            .by_thread
            .iter()
            .filter(|(key, _)| key.thread.0 != "pull-requests-panel")
            .map(|(key, p)| {
                let mut p = p.clone();
                p.surfaces = p
                    .surfaces
                    .into_iter()
                    .map(Surface::without_reveal)
                    .collect();
                (legacy_thread_key(key), p)
            })
            .collect::<BTreeMap<_, _>>();
        let visibility = self
            .visibility
            .iter()
            .filter(|(_, v)| !v.inline_open)
            .map(|(key, _)| {
                (
                    legacy_thread_key(key),
                    ThreadPanelVisibility {
                        inline_open: false,
                        popover_open: false,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        serde_json::to_string(
            &json!({"version":14,"state":{"byThreadKey":rows,"threadPanelVisibilityByThreadKey":visibility}}),
        )
    }
}
