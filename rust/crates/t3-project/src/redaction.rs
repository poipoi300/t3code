use std::sync::LazyLock;
use t3_contracts::environment::RepositoryIdentity;

pub(crate) fn remote_url(value: &str) -> String {
    if value.contains("://") {
        let Ok(mut url) = url::Url::parse(value) else {
            return "[redacted URL]".into();
        };
        if url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
        {
            return value.into();
        }
        if url.set_username("").is_err() || url.set_password(None).is_err() {
            return "[redacted URL]".into();
        }
        url.set_query(None);
        url.set_fragment(None);
        return url.into();
    }
    if let Some((user, remote)) = value.rsplit_once('@')
        && remote.contains(':')
    {
        let remote = remote.split(['?', '#']).next().unwrap_or(remote);
        return if user == "git" {
            format!("git@{remote}")
        } else {
            remote.into()
        };
    }
    value.into()
}

pub(crate) fn text(value: &str) -> String {
    static URLS: LazyLock<Option<regex::Regex>> = LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)\b[a-z][a-z0-9+.-]*://[^\s<>]+|[^\s'"<>/@]+@[^\s'"<>/:]+:[^\s<>]+"#,
        )
        .ok()
    });
    let Some(urls) = URLS.as_ref() else {
        return "[redacted output]".into();
    };
    urls.replace_all(value, |captures: &regex::Captures<'_>| {
        remote_url(&captures[0])
    })
    .into_owned()
}

pub(crate) fn identity(mut identity: RepositoryIdentity) -> RepositoryIdentity {
    let original = t3_git::remote_repository_key(&identity.locator.remote_url.0);
    identity.locator.remote_url.0 = remote_url(&identity.locator.remote_url.0);
    identity.canonical_key.0 = if identity.canonical_key.0 == original {
        t3_git::remote_repository_key(&identity.locator.remote_url.0)
    } else {
        text(&identity.canonical_key.0)
    };
    if let Some(web) = &mut identity.web_url {
        web.0 = remote_url(&web.0);
    }
    if let Some(origin) = &mut identity.origin {
        if let Some(remote) = &mut origin.remote_url {
            let original = t3_git::remote_repository_key(&remote.0);
            remote.0 = remote_url(&remote.0);
            if origin.canonical_key.0 == original {
                origin.canonical_key.0 = t3_git::remote_repository_key(&remote.0);
            }
        }
        origin.canonical_key.0 = text(&origin.canonical_key.0);
    }
    identity
}
