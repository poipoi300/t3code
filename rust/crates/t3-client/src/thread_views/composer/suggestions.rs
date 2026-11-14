//! Query identities and guarded source edits for useComposerMenuItems / onSelectComposerItem.
use super::{
    model_search::{PickerSearchItem, rank_model_picker_search},
    policy::{self, Trigger, TriggerKind},
    send::Changes,
};
use crate::{
    ClientError,
    composition::ClientRuntime,
    platform::Platform,
    rpc::RpcEndpoint,
    types::ThreadKey,
    view_logic::{JsTrim, shell::skill_search},
};
use futures::{FutureExt, future::LocalBoxFuture, stream::LocalBoxStream};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use t3_contracts::{
    rpc::WsProjectsSearchEntriesRpc as WsProjectSearchEntriesRpc,
    server::{ServerProviderSkill, ServerProviderSlashCommand},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestionRequest {
    pub trigger: Trigger,
    pub generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Path {
        directory: bool,
    },
    Slash,
    ProviderSlash,
    Skill {
        source: SkillSource,
        display_name: String,
    },
    Model {
        driver: String,
        provider_name: String,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SkillSource {
    App,
    Repo,
    Project,
    Personal,
    System,
    Other,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestionItem {
    pub id: String,
    pub label: String,
    pub description: String,
    pub kind: SuggestionKind,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuggestionProjection {
    pub request: SuggestionRequest,
    pub items: Vec<SuggestionItem>,
    pub loading: bool,
    pub empty_state: String,
    pub error: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionOutcome {
    Edited { prompt: String, cursor: usize },
    OpenModelPicker { prompt: String, cursor: usize },
}
#[derive(Default)]
struct State {
    request: Option<SuggestionRequest>,
    view: Option<Rc<SuggestionProjection>>,
    replacements: BTreeMap<String, String>,
    changes: Changes,
}
#[derive(Clone)]
pub struct ComposerSuggestionsSession<P: Platform + Clone + 'static> {
    runtime: ClientRuntime<P>,
    key: ThreadKey,
    cwd: Option<String>,
    state: Rc<RefCell<State>>,
}
fn source(skill: &ServerProviderSkill) -> SkillSource {
    let path = skill.path.0.replace('\\', "/");
    if path.contains("/.codex/plugins/") || path.contains("/.agents/plugins/") {
        return SkillSource::App;
    }
    match skill
        .scope
        .as_ref()
        .and_then(Option::as_ref)
        .map(|scope| scope.0.js_trim().to_lowercase())
        .as_deref()
    {
        Some("repo" | "repository") => SkillSource::Repo,
        Some("project" | "workspace" | "local") => SkillSource::Project,
        Some("user" | "personal") => SkillSource::Personal,
        Some("system") => SkillSource::System,
        _ => SkillSource::Other,
    }
}
pub fn rank_model_mentions(items: &[PickerSearchItem], query: &str) -> Vec<usize> {
    let spaced = query.replace([':', '/'], " ");
    let spaced = spaced.js_trim();
    let unhyphenated = spaced.replace('-', " ");
    let unhyphenated = unhyphenated.js_trim();
    if unhyphenated
        .chars()
        .filter(|ch| !crate::view_logic::is_js_whitespace(*ch))
        .map(char::len_utf16)
        .sum::<usize>()
        < 2
    {
        return vec![];
    }
    for query in [spaced, unhyphenated] {
        let tokens = query
            .split(crate::view_logic::is_js_whitespace)
            .filter(|word| !word.is_empty())
            .map(str::to_lowercase)
            .collect::<Vec<_>>();
        let indices = items
            .iter()
            .enumerate()
            .filter(|(_, item)| {
                let haystack = format!(
                    "{} {} {} {} {}",
                    item.name,
                    item.short_name.as_deref().unwrap_or_default(),
                    item.sub_provider.as_deref().unwrap_or_default(),
                    item.driver_kind,
                    item.provider_display_name
                )
                .to_lowercase();
                tokens.iter().all(|token| {
                    haystack.match_indices(token).any(|(index, _)| {
                        index == 0
                            || haystack[..index]
                                .chars()
                                .next_back()
                                .is_none_or(|ch| !ch.is_alphanumeric())
                    })
                })
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let filtered = indices
            .iter()
            .map(|index| items[*index].clone())
            .collect::<Vec<_>>();
        let ranked = rank_model_picker_search(&filtered, query);
        if !ranked.is_empty() {
            return ranked
                .into_iter()
                .take(8)
                .map(|index| indices[index])
                .collect();
        }
    }
    vec![]
}
pub fn serialize_file_link(path: &str) -> String {
    let label = path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]");
    let mut destination = String::new();
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b";/,:@&=+$-_.!~*'".contains(&byte) {
            destination.push(char::from(byte));
        } else {
            destination.push_str(&format!("%{byte:02X}"));
        }
    }
    format!("[{label}]({destination})")
}
fn slash_score(
    item: &SuggestionItem,
    skill: Option<&ServerProviderSkill>,
    query: &str,
) -> Option<usize> {
    if let Some(skill) = skill {
        if query == "skill" {
            return Some(0);
        }
        let query = query.strip_prefix("skill:").unwrap_or(query);
        return if query.is_empty() {
            Some(0)
        } else {
            skill_search::score_skill(skill, query)
                .or_else(|| "skill".starts_with(query).then_some(usize::MAX))
        };
    }
    [
        skill_search::score(
            item.label
                .trim_start_matches('/')
                .trim_end()
                .to_lowercase()
                .as_str(),
            query,
            (0, Some(2), Some(4), Some(6), Some(100)),
            &['-', '_', '/'],
        ),
        skill_search::score(
            &item.description.to_lowercase(),
            query,
            (20, Some(22), Some(24), Some(26), None),
            &['-', '_', '/'],
        ),
    ]
    .into_iter()
    .flatten()
    .min()
}
impl<P: Platform + Clone + 'static> ComposerSuggestionsSession<P> {
    pub fn new(runtime: ClientRuntime<P>, key: ThreadKey, cwd: Option<String>) -> Self {
        Self {
            runtime,
            key,
            cwd,
            state: Rc::default(),
        }
    }
    pub fn snapshot(&self) -> Option<Rc<SuggestionProjection>> {
        self.state.borrow().view.clone()
    }
    pub fn changes(&self) -> LocalBoxStream<'static, ()> {
        self.state.borrow().changes.subscribe()
    }
    pub fn query(
        &self,
        request: Option<SuggestionRequest>,
    ) -> LocalBoxFuture<'static, Result<(), ClientError>> {
        {
            let mut state = self.state.borrow_mut();
            state.request = request.clone();
            state.replacements.clear();
            state.view = request.as_ref().map(|request| {
                Rc::new(SuggestionProjection {
                    request: request.clone(),
                    items: vec![],
                    loading: true,
                    empty_state: String::new(),
                    error: None,
                })
            });
            state.changes.notify();
        }
        let this = self.clone();
        async move{let Some(request)=request else{return Ok(());};
            let mut items=vec![];let mut replacements=BTreeMap::new();let mut error=None;
            let models=this.runtime.composer_models(&this.key).snapshot();let provider=models.providers.iter().find(|provider|provider.snapshot.instance_id==models.selection.instance_id);
            let workspace=provider.and_then(|provider|provider.snapshot.workspace_snapshots.as_ref()?.iter().find(|snapshot|Some(snapshot.cwd.0.as_str())==this.cwd.as_deref()));
            let skills=workspace.map(|workspace|workspace.skills.as_slice()).or_else(||provider.map(|provider|provider.snapshot.skills.as_slice())).unwrap_or_default();
            match request.trigger.kind {
                TriggerKind::Path=>{
                    let mut candidates=vec![];let mut metadata=vec![];
                    for provider in models.providers.iter().filter(|provider|provider.ready&&provider.enabled&&provider.available){for model in provider.options.iter().filter(|model|!model.is_legacy&&!model.is_unavailable){candidates.push(PickerSearchItem{driver_kind:provider.snapshot.driver.0.clone(),provider_display_name:provider.display_name.clone(),name:model.name.clone(),short_name:model.short_name.clone(),sub_provider:model.sub_provider.clone(),is_favorite:false});metadata.push((provider,model));}}
                    for index in rank_model_mentions(&candidates,&request.trigger.query){let(provider,model)=metadata[index];let id=format!("model:{}:{}",provider.snapshot.instance_id.0,model.slug);replacements.insert(id.clone(),format!("model:{}/{} ",provider.snapshot.instance_id.0,model.slug));items.push(SuggestionItem{id,label:model.short_name.as_ref().unwrap_or(&model.name).clone(),description:model.sub_provider.as_ref().unwrap_or(&provider.display_name).clone(),kind:SuggestionKind::Model{driver:provider.snapshot.driver.0.clone(),provider_name:provider.display_name.clone()}});}
                    if let Some(cwd)=&this.cwd {let endpoint=this.runtime.client(&this.key.environment).and_then(|client|client.session()).ok_or(ClientError::Disconnected)?;
                        let target=crate::services::views::ComposerPathSearchTarget{environment:Some(this.key.environment.clone()),cwd:Some(cwd.clone()),query:Some(request.trigger.query.clone())};
                        if let Some(search)=super::path_search::request(&target,true){match endpoint.call::<WsProjectSearchEntriesRpc>(search.input).await{Ok(result)=>for entry in result.entries{let path=entry.path.0;let id=format!("path:{}:{}",if matches!(entry.kind,t3_contracts::project::ProjectEntryKind::Directory){"directory"}else{"file"},path);replacements.insert(id.clone(),format!("{} ",serialize_file_link(&path)));items.push(SuggestionItem{id,label:path.rsplit(['/', '\\']).next().unwrap_or(&path).into(),description:path.rsplit_once('/').map_or("",|(parent,_)|parent).into(),kind:SuggestionKind::Path{directory:matches!(entry.kind,t3_contracts::project::ProjectEntryKind::Directory)}});},Err(failure)=>error=Some(failure.to_string())}}
                    }
                },
                TriggerKind::Skill|TriggerKind::SlashCommand=>{
                    let driver=provider.map(|provider|provider.snapshot.driver.0.as_str()).unwrap_or_default();
                    let query=request.trigger.query.js_trim().trim_start_matches('/').to_lowercase();
                    let visible=if request.trigger.kind==TriggerKind::Skill||this.runtime.composer_preferences().snapshot().show_skills_in_slash_menu{skill_search::search_skills(skills,"",None,|left,right|left.cmp(right))}else{vec![]};let mut rows=vec![];
                    if request.trigger.kind==TriggerKind::SlashCommand {
                        rows.push((SuggestionItem{id:"slash:model".into(),label:"/model".into(),description:"Switch response model for this thread".into(),kind:SuggestionKind::Slash},String::new(),None));
                        let commands:&[ServerProviderSlashCommand]=workspace.map(|workspace|workspace.slash_commands.as_slice()).or_else(||provider.map(|provider|provider.snapshot.slash_commands.as_slice())).unwrap_or_default();
                        if request.trigger.range.anchor==0{for command in commands {if visible.iter().any(|skill|skill.name.0.js_trim().eq_ignore_ascii_case(command.name.0.js_trim())){continue;}let id=format!("provider-slash-command:{driver}:{}",command.name.0);let raw=serde_json::to_value(command)?;rows.push((SuggestionItem{id,label:format!("/{}",command.name.0),description:raw["description"].as_str().or_else(||raw["input"]["hint"].as_str()).unwrap_or("Run provider command").into(),kind:SuggestionKind::ProviderSlash},format!("/{} ",command.name.0),None));}}
                    }
                    for skill in &visible {let raw=serde_json::to_value(skill)?;let display=raw["displayName"].as_str().filter(|name|!name.js_trim().is_empty()).unwrap_or(&skill.name.0);rows.push((SuggestionItem{id:format!("skill:{driver}:{}",skill.name.0),label:if request.trigger.kind==TriggerKind::SlashCommand{format!("/skill:{}",skill.name.0)}else{display.into()},description:raw["shortDescription"].as_str().or_else(||raw["description"].as_str()).map(str::to_owned).unwrap_or_else(||raw["scope"].as_str().map_or_else(||if request.trigger.kind==TriggerKind::Skill{"Run provider skill".into()}else{String::new()},|scope|format!("{scope} skill"))),kind:SuggestionKind::Skill{source:source(skill),display_name:display.into()}},format!("${} ",skill.name.0),Some(*skill)));}
                    if !query.is_empty(){rows.retain(|(item,_,skill)|slash_score(item,*skill,&query).is_some());rows.sort_by_key(|(item,_,skill)|(slash_score(item,*skill,&query).unwrap_or(usize::MAX),match item.kind{SuggestionKind::Slash=>0,SuggestionKind::ProviderSlash=>1,_=>2},skill.map(|skill|skill.name.0.clone()).unwrap_or_else(||item.label.trim_start_matches('/').into()),item.id.clone()));}
                    if request.trigger.kind==TriggerKind::Skill{let rank=skill_search::search_skills(skills,&request.trigger.query,None,|left,right|left.cmp(right));rows.retain(|(item,_,_)|rank.iter().any(|skill|item.id==format!("skill:{driver}:{}",skill.name.0)));rows.sort_by_key(|(item,_,_)|rank.iter().position(|skill|item.id==format!("skill:{driver}:{}",skill.name.0)));}
                    for (item,replacement,_) in rows{replacements.insert(item.id.clone(),replacement);items.push(item);}
                },
                TriggerKind::PullRequest=>{error=Some("Pull requests are not available for this project.".into());},
            }
            let mut state=this.state.borrow_mut();if state.request.as_ref()!=Some(&request){return Err(ClientError::Cancelled);}
            let empty_state=match request.trigger.kind{TriggerKind::Path=>"No matching files or folders.",TriggerKind::Skill=>"No skills found. Try / to browse provider commands.",TriggerKind::SlashCommand=>"No matching command.",TriggerKind::PullRequest=>"Pull requests are not available for this project."};
            state.replacements=replacements;state.view=Some(Rc::new(SuggestionProjection{request,items,loading:false,empty_state:empty_state.into(),error}));state.changes.notify();Ok(())
        }.boxed_local()
    }
    pub fn select(
        &self,
        request: &SuggestionRequest,
        id: &str,
        source: &str,
        cursor: usize,
    ) -> Result<SuggestionOutcome, ClientError> {
        let state = self.state.borrow();
        if state.request.as_ref() != Some(request)
            || policy::detect_trigger(source, cursor).as_ref() != Some(&request.trigger)
        {
            return Err(ClientError::Cancelled);
        }
        let replacement = state.replacements.get(id).ok_or(ClientError::Cancelled)?;
        let drafts = self.runtime.composer_drafts();
        let view = drafts.view(&self.key);
        if view.prompt.as_ref() != source {
            return Err(ClientError::Cancelled);
        }
        let end =
            policy::extend_replacement_for_space(source, request.trigger.range.head, replacement);
        let from =
            crate::thread_views::document::utf16_byte_offset(source, request.trigger.range.anchor)
                .ok_or(ClientError::Cancelled)?;
        let to = crate::thread_views::document::utf16_byte_offset(source, end)
            .ok_or(ClientError::Cancelled)?;
        let prompt = format!("{}{}{}", &source[..from], replacement, &source[to..]);
        let cursor = request.trigger.range.anchor + replacement.encode_utf16().count();
        drafts.set_prompt(&self.key, prompt.clone(), cursor)?;
        Ok(if id == "slash:model" {
            SuggestionOutcome::OpenModelPicker { prompt, cursor }
        } else {
            SuggestionOutcome::Edited { prompt, cursor }
        })
    }
}
