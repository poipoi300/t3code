//! Guest entry URLs, independent of the authenticated workspace and its navigation history.

pub mod http;
pub mod session;
pub mod transport;

use t3_contracts::guest_share::GUEST_SHARE_PARAM;
use url::Url;

pub struct GuestAddress {
    share_id: Option<String>,
    /// The unspent invite, kept in memory only until the page redeems it or ends.
    credential: Option<String>,
}

impl std::fmt::Debug for GuestAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuestAddress")
            .field("share_id", &self.share_id)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl GuestAddress {
    /// Capture only the public share id. The invite is in the fragment and must
    /// never enter workspace navigation or the replacement history entry.
    pub fn parse(address: &str) -> Option<Self> {
        let url = Url::parse(address).ok()?;
        if !matches!(url.scheme(), "http" | "https")
            || crate::navigation::parse(&crate::navigation::normalize_app_path(url.path()))
                != crate::navigation::Route::Guest
        {
            return None;
        }
        let invite = url
            .fragment()
            .and_then(decode_fragment)
            .filter(|invite| !invite.trim().is_empty());
        let mut credential = None;
        let share_id = if let Some(invite) = invite {
            let mut parts = invite.trim().split('.');
            match (parts.next(), parts.next()) {
                (Some(id), Some(signature)) if !id.is_empty() && !signature.is_empty() => {
                    credential = Some(invite.trim().to_owned());
                    Some(id.to_owned())
                }
                _ => None,
            }
        } else {
            url.query_pairs()
                .find(|(key, _)| key == GUEST_SHARE_PARAM)
                .map(|(_, id)| id.into_owned())
        };
        Some(Self {
            share_id,
            credential,
        })
    }

    /// Hand the invite to the one place that redeems it. It cannot be read twice.
    pub fn take_credential(&mut self) -> Option<String> {
        self.credential.take()
    }

    pub fn share_id(&self) -> Option<&str> {
        self.share_id.as_deref()
    }

    pub fn history_path(&self) -> String {
        self.share_id().map_or_else(
            || "/guest".into(),
            |id| {
                let query = url::form_urlencoded::Serializer::new(String::new())
                    .append_pair(GUEST_SHARE_PARAM, id)
                    .finish();
                format!("/guest?{query}")
            },
        )
    }
}

fn decode_fragment(fragment: &str) -> Option<std::borrow::Cow<'_, str>> {
    // decodeURIComponent rejects malformed escapes, while percent_decode_str
    // leaves them intact. Match the browser's fallback to the query parameter.
    if fragment.split('%').skip(1).any(|escape| {
        !escape
            .as_bytes()
            .get(..2)
            .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
    }) {
        return None;
    }
    percent_encoding::percent_decode_str(fragment)
        .decode_utf8()
        .ok()
}
