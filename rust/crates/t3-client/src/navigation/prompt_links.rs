//! packages/shared/src/promptLinks.ts; URLSearchParams always retains the first duplicate.
use crate::view_logic::JsTrim;

pub const MAX_PROMPT_LENGTH: usize = 8_000;
pub const HOSTED_APP_URL: &str = "https://app.t3.codes";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkedPrompt {
    pub text: String,
    pub hidden_characters: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub owner: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PullRequest {
    pub repository: Repository,
    pub number: u64,
}

impl PullRequest {
    pub fn url(&self) -> String {
        format!(
            "https://github.com/{}/{}/pull/{}",
            self.repository.owner, self.repository.name, self.number
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewThreadLink {
    New {
        prompt: LinkedPrompt,
        repository: Option<Repository>,
        provider: Option<String>,
        model: Option<String>,
    },
    PullRequest {
        pull_request: PullRequest,
        prompt: Option<LinkedPrompt>,
    },
}

fn hidden(ch: char) -> bool {
    matches!(ch as u32,
        0x0..=0x8 | 0xb..=0xc | 0xe..=0x1f | 0x7f..=0x9f | 0xad | 0x34f | 0x61c |
        0x115f..=0x1160 | 0x17b4..=0x17b5 | 0x180b..=0x180f | 0x200b..=0x200c |
        0x200e..=0x200f | 0x202a..=0x202e | 0x2060..=0x206f | 0x3164 | 0xfe00..=0xfe0d |
        0xfeff | 0xffa0 | 0xfff0..=0xfffb | 0x1bca0..=0x1bca3 | 0x1d173..=0x1d17a | 0xe0000..=0xe0fff)
}

fn emoji(ch: Option<&char>) -> bool {
    ch.is_some_and(|ch| matches!(*ch as u32, 0x1f000..=0x1faff | 0x2190..=0x2bff | 0xfe0f))
}

pub fn sanitize_linked_prompt(raw: &str) -> LinkedPrompt {
    let mut hidden_characters = 0;
    let normalized = raw
        .replace("\r\n", "\n")
        .replace(['\r', '\u{2028}', '\u{2029}'], "\n");
    let visible = normalized
        .chars()
        .filter(|ch| {
            if hidden(*ch) {
                hidden_characters += 1;
                false
            } else {
                true
            }
        })
        .collect::<Vec<_>>();
    let text = visible
        .iter()
        .enumerate()
        .filter_map(|(index, ch)| {
            if *ch == '\u{200d}'
                && !(emoji(index.checked_sub(1).and_then(|i| visible.get(i)))
                    && emoji(visible.get(index + 1)))
            {
                hidden_characters += 1;
                None
            } else {
                Some(*ch)
            }
        })
        .collect::<String>();
    LinkedPrompt {
        text: text.js_trim().into(),
        hidden_characters,
    }
}

fn repository(owner: &str, name: &str) -> Option<Repository> {
    let name = name
        .get(..name.len().saturating_sub(4))
        .filter(|_| {
            name.get(name.len().saturating_sub(4)..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".git"))
        })
        .unwrap_or(name);
    if owner.is_empty()
        || owner.len() > 39
        || !owner.as_bytes()[0].is_ascii_alphanumeric()
        || !owner
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
        || name.is_empty()
        || name.len() > 100
        || name.bytes().all(|ch| ch == b'.')
        || !name
            .bytes()
            .all(|ch| ch.is_ascii_alphanumeric() || b"._-".contains(&ch))
    {
        return None;
    }
    Some(Repository {
        owner: owner.into(),
        name: name.into(),
    })
}

fn github_path(raw: &str) -> Option<&str> {
    let (scheme, rest) = raw.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return None;
    }
    let (host, path) = rest.split_once('/')?;
    (host.eq_ignore_ascii_case("github.com") || host.eq_ignore_ascii_case("www.github.com"))
        .then_some(path)
}

pub fn parse_repository(value: &str) -> Option<Repository> {
    let trimmed = value.js_trim();
    let path = if trimmed.contains("://") {
        let path = github_path(trimmed)?;
        if path.contains(['?', '#']) {
            return None;
        }
        path.strip_suffix('/').unwrap_or(path)
    } else {
        trimmed
    };
    let parts = path.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [owner, name] => repository(owner, name),
        _ => None,
    }
}

fn parse_pull_request(value: &str) -> Option<PullRequest> {
    let trimmed = value.js_trim();
    let (repo, number) = if trimmed.contains("://") {
        let path = github_path(trimmed)?;
        let parts = path.split('/').collect::<Vec<_>>();
        let [owner, name, "pull", rest @ ..] = parts.as_slice() else {
            return None;
        };
        if owner.contains(['?', '#']) || name.contains(['?', '#']) {
            return None;
        }
        (
            repository(owner, name)?,
            rest.first()?.split(['?', '#']).next()?,
        )
    } else {
        let parts = trimmed.split(['/', '#']).collect::<Vec<_>>();
        let [owner, name, number] = parts.as_slice() else {
            return None;
        };
        (repository(owner, name)?, *number)
    };
    if number.is_empty() || !number.bytes().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let number = number.parse::<u64>().ok()?;
    (number > 0 && number <= 9_007_199_254_740_991).then_some(PullRequest {
        repository: repo,
        number,
    })
}

fn params(search: &str) -> Vec<(String, String)> {
    url::form_urlencoded::parse(search.strip_prefix('?').unwrap_or(search).as_bytes())
        .into_owned()
        .collect()
}

fn get<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

fn read_prompt(params: &[(String, String)]) -> Result<Option<LinkedPrompt>, &'static str> {
    let Some(raw) = get(params, "prompt") else {
        return Ok(None);
    };
    if raw.encode_utf16().count() > MAX_PROMPT_LENGTH {
        return Err("This prompt is longer than 8,000 characters, so T3 Code did not open it.");
    }
    let prompt = sanitize_linked_prompt(raw);
    if prompt.text.is_empty() && prompt.hidden_characters > 0 {
        return Err(
            "This prompt had only hidden characters, so it may be malicious. T3 Code did not open it.",
        );
    }
    Ok((!prompt.text.is_empty()).then_some(prompt))
}

pub fn parse_new_thread_link(search: &str) -> Result<NewThreadLink, &'static str> {
    let params = params(search);
    let prompt = read_prompt(&params)?;
    if let Some(pr) = get(&params, "pr") {
        return Ok(NewThreadLink::PullRequest {
            pull_request: parse_pull_request(pr)
                .ok_or("This link names a pull request T3 Code cannot read.")?,
            prompt,
        });
    }
    let prompt = prompt.ok_or("This link has no prompt.")?;
    let repository = get(&params, "repo")
        .map(|repo| {
            parse_repository(repo).ok_or("This link names a repository T3 Code cannot read.")
        })
        .transpose()?;
    let optional = |key| {
        get(&params, key)
            .map(JsTrim::js_trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    Ok(NewThreadLink::New {
        prompt,
        repository,
        provider: optional("provider").map(|provider| provider.to_lowercase()),
        model: optional("model"),
    })
}

pub fn parse_thread_link_prompt(search: &str) -> Result<Option<LinkedPrompt>, &'static str> {
    read_prompt(&params(search))
}

pub fn without_prompt_link_params(search: &str) -> String {
    let params = params(search);
    let mut result = url::form_urlencoded::Serializer::new(String::new());
    result.extend_pairs(
        params.iter().filter(|(key, _)| {
            !["prompt", "repo", "pr", "provider", "model"].contains(&key.as_str())
        }),
    );
    let result = result.finish();
    if result.is_empty() {
        result
    } else {
        format!("?{result}")
    }
}

pub fn build_new_thread_link(
    prompt: &str,
    repository: Option<&Repository>,
    provider: Option<&str>,
    model: Option<&str>,
    origin: Option<&str>,
) -> Result<String, url::ParseError> {
    let mut url = url::Url::parse(origin.unwrap_or(HOSTED_APP_URL))?.join("/new")?;
    {
        let mut params = url.query_pairs_mut();
        params.append_pair("prompt", prompt);
        if let Some(repository) = repository {
            params.append_pair("repo", &format!("{}/{}", repository.owner, repository.name));
        }
        if let Some(provider) = provider.filter(|p| !p.is_empty()) {
            params.append_pair("provider", provider);
            if let Some(model) = model.filter(|m| !m.is_empty()) {
                params.append_pair("model", model);
            }
        }
    }
    Ok(url.into())
}

fn thread_path(path: &str) -> Option<String> {
    let parts = path
        .strip_suffix('/')
        .unwrap_or(path)
        .strip_prefix('/')?
        .split('/')
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [environment, thread] if !environment.is_empty() && !thread.is_empty() => {
            Some(format!("/{environment}/{thread}"))
        }
        _ => None,
    }
}

pub fn prompt_link_app_path(value: &str, hosted_app_url: Option<&str>) -> Option<String> {
    let url = url::Url::parse(value).ok()?;
    let app_scheme = super::is_app_scheme(url.scheme());
    let search = url
        .query()
        .filter(|q| !q.is_empty())
        .map_or(String::new(), |q| format!("?{q}"));
    let path = if app_scheme {
        if url.host_str() == Some("new") && matches!(url.path(), "" | "/") {
            Some("/new".into())
        } else if url.host_str() == Some("threads") {
            thread_path(url.path())
        } else {
            None
        }
    } else {
        let hosted = url::Url::parse(hosted_app_url.unwrap_or(HOSTED_APP_URL)).ok()?;
        if url.origin() != hosted.origin() {
            return None;
        }
        if url.path() == "/new" {
            Some("/new".into())
        } else if let Some(path) = url.path().strip_prefix("/threads") {
            thread_path(path)
        } else if url.query_pairs().any(|(key, _)| key == "prompt") {
            thread_path(url.path())
        } else {
            None
        }
    }?;
    Some(format!("{path}{search}"))
}

pub fn prompt_link_deep_link(
    app_path: &str,
    scheme: Option<&str>,
) -> Result<String, url::ParseError> {
    let url = url::Url::parse("https://app.invalid")?.join(app_path)?;
    let host = if url.path() == "/new" {
        "new".into()
    } else {
        format!("threads{}", url.path())
    };
    let search = url
        .query()
        .filter(|q| !q.is_empty())
        .map_or(String::new(), |q| format!("?{q}"));
    Ok(format!("{}://{host}{search}", scheme.unwrap_or("t3code")))
}

#[cfg(test)]
#[path = "prompt_links_tests.rs"]
mod tests;
