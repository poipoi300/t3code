//! Device descriptions and the single approval queue shared by client surfaces.
use std::collections::{BTreeSet, VecDeque};
use t3_contracts::auth::{AuthClientMetadata, AuthPairingLink};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectDeviceApprovalPrompt {
    pub pairing_link_id: String,
    pub title: String,
    pub message: String,
}

pub fn connect_device_name(client: Option<&AuthClientMetadata>) -> &str {
    client
        .and_then(|client| {
            client
                .label
                .as_ref()
                .or(client.os.as_ref())
                .or(client.browser.as_ref())
        })
        .map_or("this device", |value| value.0.as_str())
}

pub fn describe_connect_device_client(client: Option<&AuthClientMetadata>) -> String {
    let Some(client) = client else {
        return "Unknown device".into();
    };
    let mut bits = Vec::new();
    for value in [
        &client.label,
        &client.os,
        &client.browser,
        &client.ip_address,
    ]
    .into_iter()
    .flatten()
    {
        if !value.0.is_empty() && !bits.contains(&value.0.as_str()) {
            bits.push(value.0.as_str());
        }
    }
    if bits.is_empty() {
        "Unknown device".into()
    } else {
        bits.join(" · ")
    }
}

pub fn describe_connect_device(client: Option<&AuthClientMetadata>) -> String {
    let Some(client) = client else {
        return "A new device".into();
    };
    if let Some(label) = &client.label {
        return label.0.clone();
    }
    match (&client.browser, &client.os) {
        (Some(browser), Some(os)) => format!("{} on {}", browser.0, os.0),
        (Some(value), None) | (None, Some(value)) => value.0.clone(),
        (None, None) => "A new device".into(),
    }
}

pub fn connect_device_approval_prompts(
    environment_label: &str,
    pairing_links: &[AuthPairingLink],
    shown_ids: &BTreeSet<String>,
) -> Vec<ConnectDeviceApprovalPrompt> {
    let mut waiting = pairing_links
        .iter()
        .filter_map(|link| {
            link.connect_device_request
                .as_ref()
                .filter(|_| !shown_ids.contains(&link.id.0))
                .map(|request| (link, request))
        })
        .collect::<Vec<_>>();
    waiting.sort_by_key(|(link, _)| crate::time::epoch_ms(&link.created_at.0));
    waiting.into_iter().map(|(link, request)| ConnectDeviceApprovalPrompt {
        pairing_link_id: link.id.0.clone(),
        title: "Approve this device?".into(),
        message: format!("{} wants to connect to {environment_label} through T3 Connect. It shows code {}. Approve it only if that code matches.", describe_connect_device(request.client.as_ref()), request.verification_code.0),
    }).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Done,
    Retry,
    Later,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresentationId(u64);

struct Showing<P> {
    id: PresentationId,
    prompt: P,
    dropped: bool,
}

pub struct ApprovalPromptQueue<P> {
    known: BTreeSet<String>,
    waiting: VecDeque<P>,
    showing: Option<Showing<P>>,
    next_id: u64,
}

pub trait ApprovalPrompt {
    fn pairing_link_id(&self) -> &str;
}

impl ApprovalPrompt for ConnectDeviceApprovalPrompt {
    fn pairing_link_id(&self) -> &str {
        &self.pairing_link_id
    }
}

impl<P> Default for ApprovalPromptQueue<P> {
    fn default() -> Self {
        Self {
            known: BTreeSet::new(),
            waiting: VecDeque::new(),
            showing: None,
            next_id: 0,
        }
    }
}

impl<P: ApprovalPrompt> ApprovalPromptQueue<P> {
    pub fn known(&self) -> &BTreeSet<String> {
        &self.known
    }

    pub fn showing(&self) -> Option<(PresentationId, &P)> {
        self.showing
            .as_ref()
            .map(|showing| (showing.id, &showing.prompt))
    }

    pub fn offer(&mut self, prompts: impl IntoIterator<Item = P>) {
        for prompt in prompts {
            if self.known.insert(prompt.pairing_link_id().to_owned()) {
                self.waiting.push_back(prompt);
            }
        }
        self.next();
    }

    pub fn answer(&mut self, id: PresentationId, outcome: ApprovalOutcome) {
        if self.showing.as_ref().is_none_or(|showing| showing.id != id) {
            return;
        }
        let Some(current) = self.showing.take() else {
            return;
        };
        if outcome != ApprovalOutcome::Done && !current.dropped {
            self.waiting.push_front(current.prompt);
        }
        if outcome != ApprovalOutcome::Later {
            self.next()
        }
    }

    pub fn drop_matching(&mut self, matches: impl Fn(&P) -> bool) {
        if let Some(current) = &mut self.showing
            && matches(&current.prompt)
        {
            current.dropped = true;
            self.known.remove(current.prompt.pairing_link_id());
        }
        self.waiting.retain(|prompt| {
            if !matches(prompt) {
                return true;
            }
            self.known.remove(prompt.pairing_link_id());
            false
        });
    }

    fn next(&mut self) {
        if self.showing.is_some() {
            return;
        }
        if let Some(prompt) = self.waiting.pop_front() {
            self.next_id = self.next_id.wrapping_add(1);
            self.showing = Some(Showing {
                id: PresentationId(self.next_id),
                prompt,
                dropped: false,
            });
        }
    }
}

#[cfg(test)]
mod tests;

impl super::EnvironmentHttpClient {
    pub async fn approve_connect_device(
        &self,
        input: &t3_contracts::auth::AuthApproveConnectDeviceInput,
    ) -> Result<
        t3_contracts::auth::AuthApproveConnectDeviceResult,
        crate::authorization::AuthorizationError,
    > {
        let body = serde_json::to_value(input)
            .map_err(|_| crate::authorization::AuthorizationError::InvalidResponse)?;
        self.execute(
            "POST",
            "/api/auth/connect-devices/approve",
            Some(&body),
            8_000,
        )
        .await
        .map(crate::authorization::Sensitive::into_inner)
    }

    pub async fn revoke_connect_device(
        &self,
        input: &t3_contracts::auth::AuthRevokeConnectDeviceInput,
    ) -> Result<
        t3_contracts::auth::AuthRevokeConnectDeviceResult,
        crate::authorization::AuthorizationError,
    > {
        let body = serde_json::to_value(input)
            .map_err(|_| crate::authorization::AuthorizationError::InvalidResponse)?;
        self.execute(
            "POST",
            "/api/auth/connect-devices/revoke",
            Some(&body),
            8_000,
        )
        .await
        .map(crate::authorization::Sensitive::into_inner)
    }
}
