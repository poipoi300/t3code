use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::LazyLock};
use t3_contracts::{
    model_selection::ModelSelection, orchestration_v2::OrchestrationV2Command,
    server::ServerProvider,
};

pub type Properties = BTreeMap<String, Value>;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureUsage {
    pub feature: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
}
fn usage(feature: &str, variant: Option<&str>) -> Option<FeatureUsage> {
    Some(FeatureUsage {
        feature: feature.into(),
        variant: variant.map(str::to_owned),
    })
}
pub fn command_feature_usage(command: &OrchestrationV2Command) -> Option<FeatureUsage> {
    command_feature_value(&serde_json::to_value(command).ok()?)
}
fn command_feature_value(command: &Value) -> Option<FeatureUsage> {
    match command["type"].as_str()? {
        "message.dispatch" => command
            .get("guestMessageId")
            .and_then(|_| usage("guest_message.send", None)),
        "checkpoint.rollback" => usage(
            "checkpoint.rollback",
            Some(if command["restoreFiles"] == false {
                "conversation_only"
            } else {
                "with_files"
            }),
        ),
        "thread.fork" => usage("thread.fork", None),
        "thread.merge_back" => usage("thread.merge_back", None),
        "provider.switch" => usage("provider.switch", None),
        "thread.archive" => usage("thread.archive", None),
        "thread.snooze" => usage("thread.snooze", None),
        "thread.pin" => usage("thread.pin", None),
        "thread.settle" => usage("thread.settle", None),
        "thread.pull-request.link" => usage("thread.pull_request.link", None),
        "thread.pull-request.watch" => usage(
            "thread.pull_request.watch",
            Some(if command["watching"] == true {
                "on"
            } else {
                "off"
            }),
        ),
        "thread.runtime-mode.set" => {
            usage("thread.runtime_mode.set", command["runtimeMode"].as_str())
        }
        "thread.interaction-mode.set" => usage(
            "thread.interaction_mode.set",
            command["interactionMode"].as_str(),
        ),
        "run.interrupt" => usage("run.interrupt", None),
        "queued-message.promote-to-steer" => usage("queue.steer", None),
        "queued-run.edit" => usage("queue.edit", None),
        "thread.guest-share.close" => usage("guest_share.close", None),
        "thread.guest-message.dismiss" => usage("guest_message.dismiss", None),
        _ => None,
    }
}

static RPC_FEATURES: LazyLock<BTreeMap<String, (String, Option<String>)>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../fixtures/rpc-features.json"))
        .expect("reviewed RPC feature fixture")
});
pub fn feature_usage(method: &str, payload: &Value) -> Option<FeatureUsage> {
    if !payload.is_object() {
        return None;
    }
    let (feature, key) = RPC_FEATURES.get(method)?;
    let variant = key.as_ref().and_then(|key| payload.get(key));
    let valid = variant.is_some_and(|value| match method {
        "shell.openInEditor" => {
            serde_json::from_value::<t3_contracts::editor::EditorId>(value.clone()).is_ok()
        }
        "git.runStackedAction" => {
            serde_json::from_value::<t3_contracts::git::GitStackedAction>(value.clone()).is_ok()
        }
        "pullRequests.runAction" => {
            serde_json::from_value::<t3_contracts::pull_request::PullRequestAction>(value.clone())
                .is_ok()
        }
        _ => false,
    });
    usage(feature, variant.filter(|_| valid).and_then(Value::as_str))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProviderDimensions {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}
fn driver(provider: &ServerProvider) -> String {
    serde_json::to_value(&provider.driver)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "unknown".into())
}
pub fn provider_dimensions(
    providers: &[ServerProvider],
    selection: &ModelSelection,
) -> ProviderDimensions {
    let Some(provider) = providers
        .iter()
        .find(|p| p.instance_id == selection.instance_id)
    else {
        return ProviderDimensions {
            provider: "unknown".into(),
            model: None,
        };
    };
    let driver = driver(provider);
    let model =
        if ["codex", "claudeAgent", "cursor", "grok", "antigravity"].contains(&driver.as_str()) {
            provider
                .models
                .iter()
                .find(|m| m.slug.as_str() == selection.model && !m.is_custom)
                .map(|m| m.slug.to_string())
        } else {
            None
        };
    ProviderDimensions {
        provider: driver,
        model,
    }
}
static CATALOG: LazyLock<BTreeMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../fixtures/catalog-model-labels.json"))
        .expect("reviewed vendor catalog")
});
pub fn catalog_model_label(providers: &[ServerProvider], model: &str) -> Option<String> {
    let leaf = model.rsplit('/').next().unwrap_or(model);
    let dated = leaf
        .rsplit_once('-')
        .filter(|(_, suffix)| suffix.len() == 8 && suffix.bytes().all(|c| c.is_ascii_digit()))
        .map(|(head, _)| head)
        .unwrap_or(leaf);
    let candidates = [model, leaf, dated];
    for name in candidates {
        if let Some(label) = CATALOG.get(name) {
            return Some(label.clone());
        }
    }
    providers
        .iter()
        .filter(|p| ["cursor", "grok"].contains(&driver(p).as_str()))
        .flat_map(|p| &p.models)
        .find(|m| !m.is_custom && candidates.contains(&m.slug.as_str()))
        .map(|m| m.slug.to_string())
}
pub fn cross_provider(caller: &str, target: &str) -> Option<bool> {
    if caller == "unknown" || target == "unknown" {
        None
    } else {
        Some(caller != target)
    }
}
fn one_of<'a>(value: Option<&'a str>, allowed: &[&str]) -> Option<&'a str> {
    value.filter(|v| allowed.contains(v))
}
pub fn handoff_settings(tool: &str, args: &Value, result: &Value) -> Properties {
    let v = match tool {
        "delegate_task" => {
            json!({"targetChosen":args.get("target").is_some(),"mode":one_of(args["mode"].as_str(),&["async","wait"]).unwrap_or("async"),"waitTimedOut":(result["waitTimedOut"]==true).then_some(true)})
        }
        "create_threads" => {
            let threads = args["threads"].as_array().cloned().unwrap_or_default();
            json!({"batchSize":threads.len(),"targetChosen":threads.iter().any(|t|t.get("target").is_some())})
        }
        "t3_thread_launch" => {
            json!({"targetChosen":args.get("modelSelection").is_some(),"workspace":if args["scratch"]==true{"scratch"}else{one_of(args["workspaceStrategy"]["type"].as_str(),&["root","worktree","existing_worktree"]).unwrap_or("root")}})
        }
        "t3_thread_send" => json!({"delivery":result["delivery"]}),
        "t3_thread_send_attachments" => json!({"delivery":"auto"}),
        "schedule_task" => json!({"bindToCurrentThread":args["bindToCurrentThread"]!=false}),
        _ => json!({}),
    };
    compact(v)
}
pub fn tool_outcome(result: Option<&Value>) -> Properties {
    let Some(result) = result else {
        return compact(json!({"outcome":"error","errorCode":"exception"}));
    };
    let content = &result["structuredContent"];
    let code = content["code"]
        .as_str()
        .or_else(|| content["error"]["_tag"].as_str());
    if result["isError"] == true || (content.get("_tag").is_some() && code.is_some()) {
        compact(json!({"outcome":"error","errorCode":code.unwrap_or("unknown")}))
    } else {
        compact(json!({"outcome":"ok"}))
    }
}
pub(crate) fn compact(value: Value) -> Properties {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThread {
    pub model_selection: ModelSelection,
    pub runtime_mode: String,
    pub interaction_mode: String,
    pub created_by: String,
}
pub struct AgentToolInput<'a> {
    pub tool: &'a str,
    pub providers: &'a [ServerProvider],
    pub caller: Option<&'a AgentThread>,
    pub caller_selection: Option<&'a ModelSelection>,
    pub caller_depth: Option<usize>,
    pub caller_scheduled: bool,
    pub targets: &'a [AgentThread],
    pub outcome: Properties,
    pub settings: Properties,
    pub duration_ms: Option<u64>,
}
pub fn agent_tool_properties(input: AgentToolInput<'_>) -> Vec<Properties> {
    let caller = input
        .caller
        .map(|c| &c.model_selection)
        .or(input.caller_selection)
        .map(|s| provider_dimensions(input.providers, s))
        .unwrap_or(ProviderDimensions {
            provider: "unknown".into(),
            model: None,
        });
    let mut base = compact(
        json!({"tool":input.tool,"callerProvider":caller.provider,"callerModel":caller.model,"callerDepth":input.caller_depth.map(|d|d.min(5)),"durationMs":input.duration_ms,
        "callerOrigin":input.caller.map(|c|if input.caller_scheduled{"scheduler"}else if c.created_by=="agent"{"agent"}else if c.created_by=="system"{"system"}else{"user"})}),
    );
    base.extend(input.outcome);
    base.extend(input.settings);
    if input.targets.is_empty() {
        return vec![base];
    }
    input.targets.iter().map(|thread|{
        let target=provider_dimensions(input.providers,&thread.model_selection);let mut properties=base.clone();
        properties.extend(compact(json!({"targetProvider":target.provider,"targetModel":target.model,"targetRuntimeMode":thread.runtime_mode,"targetInteractionMode":thread.interaction_mode,"crossProvider":cross_provider(&caller.provider,&target.provider)})));properties
    }).collect()
}
