use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdvertisedEndpointProviderKind {
    #[serde(rename = "core")]
    Core,
    #[serde(rename = "private-network")]
    PrivateNetwork,
    #[serde(rename = "tunnel")]
    Tunnel,
    #[serde(rename = "manual")]
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdvertisedEndpointReachability {
    #[serde(rename = "loopback")]
    Loopback,
    #[serde(rename = "lan")]
    Lan,
    #[serde(rename = "private-network")]
    PrivateNetwork,
    #[serde(rename = "public")]
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdvertisedEndpointHostedHttpsCompatibility {
    #[serde(rename = "compatible")]
    Compatible,
    #[serde(rename = "mixed-content-blocked")]
    MixedContentBlocked,
    #[serde(rename = "requires-configuration")]
    RequiresConfiguration,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdvertisedEndpointStatus {
    #[serde(rename = "available")]
    Available,
    #[serde(rename = "unavailable")]
    Unavailable,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdvertisedEndpointSource {
    #[serde(rename = "desktop-core")]
    DesktopCore,
    #[serde(rename = "desktop-addon")]
    DesktopAddon,
    #[serde(rename = "server")]
    Server,
    #[serde(rename = "user")]
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DesktopAppEndpointCompatibility {
    #[serde(rename = "compatible")]
    Compatible,
    #[serde(rename = "unknown")]
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertisedEndpointProvider {
    pub id: String,
    pub label: String,
    pub kind: AdvertisedEndpointProviderKind,
    pub is_addon: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertisedEndpointCompatibility {
    pub hosted_https_app: AdvertisedEndpointHostedHttpsCompatibility,
    pub desktop_app: DesktopAppEndpointCompatibility,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvertisedEndpoint {
    pub id: String,
    pub label: String,
    pub provider: AdvertisedEndpointProvider,
    pub http_base_url: String,
    pub ws_base_url: String,
    pub reachability: AdvertisedEndpointReachability,
    pub compatibility: AdvertisedEndpointCompatibility,
    pub source: AdvertisedEndpointSource,
    pub status: AdvertisedEndpointStatus,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub is_default: Option<Option<bool>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::wire::double_option"
    )]
    pub description: Option<Option<String>>,
}
