use super::*;
use crate::rpc::RpcEndpoint;
use crate::{
    services::filesystem::ReadAccess,
    services::views::ComposerPathSearchTarget,
    thread_views::composer::path_search,
    types::QueryState,
    view_logic::shell::palette_files::{ContentSearchView, FileSearchView, SearchOptions},
};
use t3_contracts::{base_schemas::*, project::*, rpc::*};

impl<P: Platform + Clone + 'static> PaletteService<P> {
    fn guarded_query<M, V>(
        &self,
        target: SearchTarget,
        input: M::Input,
        view: impl Fn(Option<&QueryState<Rc<M::Output>>>) -> V + 'static,
    ) -> LocalBoxStream<'static, V>
    where
        M: crate::rpc::RpcMethod + 'static,
        M::Input: Clone + serde::Serialize + 'static,
        M::Output: PartialEq + serde::Serialize + 'static,
        V: 'static,
    {
        let this = self.clone();
        let changes = self.changes();
        let view = Rc::new(view);
        let lease: Option<crate::composition::QueryLease<M::Output>> = None;
        let updates: Option<LocalBoxStream<'static, ()>> = None;
        futures::stream::unfold(
            (this, target, input, view, changes, lease, updates, 0u8),
            |(this, target, input, view, mut changes, mut lease, mut updates, phase)| async move {
                if phase == 2 {
                    return None;
                }
                let mut ended = false;
                if phase == 1 {
                    if let Some(events) = updates.as_mut() {
                        match futures::future::select(events.next(), changes.next()).await {
                            futures::future::Either::Left((None, _))
                            | futures::future::Either::Right((None, _)) => ended = true,
                            _ => {}
                        }
                    } else {
                        ended = changes.next().await.is_none();
                    }
                }
                if !ended && this.access(&target).allowed {
                    if lease.is_none() {
                        let query = this.runtime.query::<M>(
                            &target.thread.environment,
                            input.clone(),
                            ResourcePolicy::default(),
                        );
                        updates = Some(
                            query
                                .resource()
                                .observe(this.platform.monotonic_ms())
                                .map(|_| ())
                                .boxed_local(),
                        );
                        lease = Some(query);
                    }
                } else if !ended {
                    updates = None;
                    lease = None;
                }
                let mut snapshot = lease.as_ref().map(|lease| lease.resource().snapshot());
                if ended {
                    let state = snapshot.get_or_insert_with(|| QueryState {
                        value: None,
                        loading: false,
                        error: None,
                        revision: 0,
                    });
                    state.loading = false;
                    if state.error.is_none() {
                        state.error = Some(ClientError::Disconnected.to_string());
                    }
                }
                let value = view(snapshot.as_ref());
                Some((
                    value,
                    (
                        this,
                        target,
                        input,
                        view,
                        changes,
                        lease,
                        updates,
                        if ended { 2 } else { 1 },
                    ),
                ))
            },
        )
        .boxed_local()
    }
    fn access(&self, target: &SearchTarget) -> ReadAccess {
        self.runtime
            .project_files()
            .read_access(&target.thread.environment)
    }
    fn file_view(
        &self,
        target: &SearchTarget,
        query: &str,
        state: Option<&QueryState<Rc<ProjectSearchEntriesResult>>>,
    ) -> FileSearchView {
        let access = self.access(target);
        let error = if access.pending {
            None
        } else if !access.allowed {
            Some(
                access
                    .error
                    .unwrap_or_else(|| "This connection cannot search host files.".into()),
            )
        } else {
            state
                .and_then(|s| s.error.as_ref())
                .map(ToString::to_string)
        };
        let pending = access.pending
            || (access.allowed
                && state.is_none_or(|s| s.loading || (s.value.is_none() && s.error.is_none())));
        FileSearchView {
            entries: if pending || !access.allowed {
                Rc::default()
            } else {
                state
                    .and_then(|s| s.value.as_ref())
                    .map_or_else(Rc::default, |v| Rc::new(v.entries.clone()))
            },
            matched_query: query.into(),
            pending,
            error,
        }
    }
    fn content_view(
        &self,
        target: &SearchTarget,
        query: &str,
        options: SearchOptions,
        state: Option<&QueryState<Rc<ProjectSearchContentsResult>>>,
    ) -> ContentSearchView {
        let access = self.access(target);
        let has_query = crate::view_logic::shell::palette_files::has_query(query);
        let long = query.encode_utf16().count() > PROJECT_SEARCH_CONTENTS_QUERY_MAX_LENGTH;
        let error = if access.pending {
            None
        } else if !access.allowed {
            Some(
                access
                    .error
                    .unwrap_or_else(|| "This connection cannot search host files.".into()),
            )
        } else if long {
            Some(format!(
                "Search is limited to {PROJECT_SEARCH_CONTENTS_QUERY_MAX_LENGTH} characters."
            ))
        } else {
            state
                .and_then(|s| s.error.as_ref())
                .map(ToString::to_string)
        };
        let value = state
            .and_then(|s| s.value.as_ref())
            .filter(|_| access.allowed && has_query && !long)
            .cloned();
        if let Some(value) = &value {
            *self.content_cache.borrow_mut() = Some((target.clone(), value.clone()));
        }
        let value = value.or_else(|| {
            (access.allowed && has_query && !long)
                .then(|| {
                    self.content_cache
                        .borrow()
                        .as_ref()
                        .filter(|(prior, _)| prior == target)
                        .map(|(_, value)| value.clone())
                })
                .flatten()
        });
        ContentSearchView {
            matches: value
                .as_ref()
                .map_or_else(Rc::default, |v| Rc::new(v.matches.clone())),
            can_read_files: access.allowed,
            checking_access: access.pending,
            has_query,
            pending: access.pending
                || (access.allowed
                    && has_query
                    && !long
                    && state.is_none_or(|s| s.loading || (s.value.is_none() && s.error.is_none()))),
            error,
            invalid_regex: options.use_regex
                && value.as_ref().is_some_and(|v| {
                    v.regex_fallback_error
                        .as_ref()
                        .and_then(Option::as_ref)
                        .is_some_and(|error| !error.is_empty())
                }),
            truncated: value.as_ref().is_some_and(|v| v.truncated),
        }
    }
    /// Unsupported message search has the web's empty-result behavior; other failures are typed.
    pub async fn message_matches(&self, raw: &str) -> Result<Vec<Item>, ClientError> {
        let query = t3_contracts::wire::trim_js(raw);
        if !(2..=t3_contracts::thread_search::THREAD_SEARCH_QUERY_MAX_LENGTH)
            .contains(&query.encode_utf16().count())
        {
            return Ok(vec![]);
        }
        self.platform.sleep(120).await;
        let mut jobs = futures::stream::FuturesUnordered::new();
        for environment in self.runtime.environments() {
            if environment.state.phase != crate::connection::Phase::Connected {
                continue;
            }
            let Some(endpoint) = self
                .runtime
                .client(&environment.id)
                .and_then(|c| c.session())
            else {
                continue;
            };
            let Some(input) = crate::thread_views::threads::search::thread_search_input(
                query,
                &Default::default(),
                &environment.id,
                self.runtime
                    .store()
                    .shared_shell(&environment.id)
                    .and_then(|shell| {
                        shell
                            .older_settled
                            .as_ref()
                            .map(|older| older.before.clone())
                    }),
            ) else {
                continue;
            };
            jobs.push(async move {
                let result = endpoint
                    .call::<WsOrchestrationV2SearchThreadsRpc>(input)
                    .await;
                (environment.id, result)
            });
        }
        let mut items = vec![];
        let mut candidates = vec![];
        let mut older_titles = vec![];
        while let Some((environment, result)) = jobs.next().await {
            let reply = match result {
                Ok(value) => value,
                Err(ClientError::Unsupported(_)) => continue,
                Err(error) => return Err(error),
            };
            for thread in reply.threads.as_deref().unwrap_or_default() {
                if sidebar::is_hidden_thread(&thread.lineage) {
                    continue;
                }
                let key = ThreadKey {
                    environment: environment.clone(),
                    thread: thread.id.clone(),
                };
                let project = ProjectKey {
                    environment: environment.clone(),
                    project: thread.project_id.clone(),
                };
                let shell = self.runtime.store().shared_shell(&environment);
                let project_title = shell
                    .as_ref()
                    .and_then(|s| s.projects.iter().find(|p| p.id == thread.project_id))
                    .map(|p| p.title.0.as_str());
                let subject = self.subject(
                    &sidebar::SidebarThread {
                        key: key.clone(),
                        project,
                        shell: thread,
                    },
                    project_title,
                );
                if search::score_thread(
                    query,
                    &subject,
                    self.platform.now_epoch_ms() as f64,
                    false,
                    false,
                )
                .is_none()
                {
                    continue;
                }
                let mut item = Item::action(
                    format!("thread:{}:{}", environment.0, key.thread.0),
                    &thread.title,
                );
                item.thread = Some(key.clone());
                item.thread_search = Some(subject);
                item.description = project_title.map(str::to_owned);
                self.message_targets.borrow_mut().insert(key);
                older_titles.push(item);
            }
            for found in reply.matches {
                let key = ThreadKey {
                    environment: environment.clone(),
                    thread: found.thread_id.clone(),
                };
                let shell = self.runtime.store().shared_shell(&environment);
                let thread = shell
                    .as_ref()
                    .and_then(|s| {
                        s.threads
                            .iter()
                            .find(|t| t.id == found.thread_id)
                            .map(Rc::as_ref)
                    })
                    .or_else(|| {
                        reply
                            .threads
                            .as_ref()?
                            .iter()
                            .find(|t| t.id == found.thread_id)
                    });
                let Some(thread) = thread.filter(|t| !sidebar::is_hidden_thread(&t.lineage)) else {
                    continue;
                };
                self.message_targets.borrow_mut().insert(key.clone());
                let project = ProjectKey {
                    environment: environment.clone(),
                    project: found.project_id.clone(),
                };
                let title = shell
                    .as_ref()
                    .and_then(|s| s.projects.iter().find(|p| p.id == found.project_id))
                    .map(|p| p.title.0.as_str());
                let subject = self.subject(
                    &sidebar::SidebarThread {
                        key: key.clone(),
                        project,
                        shell: thread,
                    },
                    title,
                );
                let subject_view = subject.clone();
                let canonical = t3_shared::palette_search::ThreadSubject {
                    id: subject.id,
                    title: subject.title,
                    branch: subject.branch,
                    project_title: subject.project_title,
                    environment_label: subject.environment_label,
                    activity_at: subject.activity_at,
                    opens: subject.opens,
                    pull_requests: subject
                        .pull_requests
                        .into_iter()
                        .map(|p| t3_shared::palette_search::SearchPullRequest {
                            number: p.number,
                            title: p.title,
                            references: p.references,
                        })
                        .collect(),
                };
                let value = serde_json::to_value(&found)?;
                let title_match = t3_shared::palette_search::score_thread(
                    query,
                    &canonical,
                    self.platform.now_epoch_ms() as f64,
                    false,
                    false,
                )
                .is_some();
                candidates.push(t3_shared::palette_search::MessageMatchCandidate {
                    subject: Some(canonical),
                    quality: value["quality"].as_f64(),
                    hits: value["hits"].as_u64(),
                    message_created_at: value["messageCreatedAt"].as_str().map(str::to_owned),
                });
                let mut item = Item::action(
                    format!("thread:{}:{}", environment.0, key.thread.0),
                    &thread.title,
                );
                item.thread_search = title_match.then_some(subject_view);
                item.thread = Some(key);
                item.description = Some(found.snippet);
                items.push(item);
            }
        }
        let title_keys = older_titles
            .iter()
            .filter_map(|item| item.thread.clone())
            .collect::<BTreeSet<_>>();
        let mut result = older_titles;
        result.extend(
            items
                .iter()
                .filter(|item| {
                    item.thread_search.is_some()
                        && !item
                            .thread
                            .as_ref()
                            .is_some_and(|key| title_keys.contains(key))
                })
                .cloned(),
        );
        result.extend(
            t3_shared::palette_search::rank_message_matches(
                &candidates,
                query,
                self.platform.now_epoch_ms() as f64,
                true,
                true,
            )
            .into_iter()
            .filter(|index| {
                items[*index].thread_search.is_none()
                    && !items[*index]
                        .thread
                        .as_ref()
                        .is_some_and(|key| title_keys.contains(key))
            })
            .map(|index| items[index].clone()),
        );
        Ok(result)
    }
}
impl<P: Platform + Clone + 'static> PaletteSearchClient for PaletteService<P> {
    fn watch_target(&self) -> LocalBoxStream<'static, Option<SearchTarget>> {
        let this = self.clone();
        let mut previous = None;
        self.changes()
            .filter_map(move |_| {
                let value = this.search_target();
                let emit = previous.as_ref() != Some(&value);
                previous = Some(value.clone());
                futures::future::ready(emit.then_some(value))
            })
            .boxed_local()
    }
    fn can_open(&self, target: &SearchTarget) -> bool {
        self.search_target().as_ref() == Some(target) && self.access(target).allowed
    }
    fn files(
        &self,
        target: SearchTarget,
        query: String,
    ) -> LocalBoxStream<'static, FileSearchView> {
        let normalized = path_search::normalize_query(Some(&query)).unwrap_or_default();
        let initial = self.file_view(&target, &normalized, None);
        let this = self.clone();
        let tail = async move {
            this.platform.sleep(120).await;
            let input = path_search::request(
                &ComposerPathSearchTarget {
                    environment: Some(target.thread.environment.clone()),
                    cwd: Some(target.cwd.clone()),
                    query: Some(normalized.clone()),
                },
                true,
            )
            .expect("complete file search target")
            .input;
            let input = ProjectSearchEntriesInput {
                limit: 200,
                kind: Some(Some(ProjectEntryKind::File)),
                ..input
            };
            let service = this.clone();
            let view_target = target.clone();
            this.guarded_query::<WsProjectsSearchEntriesRpc, _>(target, input, move |state| {
                service.file_view(&view_target, &normalized, state)
            })
        };
        futures::stream::once(futures::future::ready(initial))
            .chain(futures::stream::once(tail).flatten())
            .boxed_local()
    }
    fn contents(
        &self,
        target: SearchTarget,
        query: String,
        options: SearchOptions,
    ) -> LocalBoxStream<'static, ContentSearchView> {
        let initial = self.content_view(&target, &query, options, None);
        let this = self.clone();
        if !initial.has_query
            || query.encode_utf16().count() > PROJECT_SEARCH_CONTENTS_QUERY_MAX_LENGTH
        {
            return this
                .changes()
                .map(move |_| this.content_view(&target, &query, options, None))
                .boxed_local();
        }
        let tail = async move {
            this.platform.sleep(120).await;
            let input = ProjectSearchContentsInput {
                cwd: TrimmedNonEmptyString(target.cwd.clone()),
                query: query.clone(),
                limit: 500,
                case_sensitive: options.case_sensitive,
                whole_word: options.whole_word,
                use_regex: options.use_regex,
            };
            let service = this.clone();
            let view_target = target.clone();
            this.guarded_query::<WsProjectsSearchContentsRpc, _>(target, input, move |state| {
                service.content_view(&view_target, &query, options, state)
            })
        };
        futures::stream::once(futures::future::ready(initial))
            .chain(futures::stream::once(tail).flatten())
            .boxed_local()
    }
}
