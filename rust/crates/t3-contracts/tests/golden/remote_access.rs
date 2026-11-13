use super::super::{Registry, register};
use t3_contracts::remote_access::*;

pub fn register_module(registry: &mut Registry) {
    register::<AdvertisedEndpointProviderKind>(registry, "AdvertisedEndpointProviderKind");
    register::<AdvertisedEndpointReachability>(registry, "AdvertisedEndpointReachability");
    register::<AdvertisedEndpointHostedHttpsCompatibility>(
        registry,
        "AdvertisedEndpointHostedHttpsCompatibility",
    );
    register::<AdvertisedEndpointStatus>(registry, "AdvertisedEndpointStatus");
    register::<AdvertisedEndpointSource>(registry, "AdvertisedEndpointSource");
    register::<AdvertisedEndpointProvider>(registry, "AdvertisedEndpointProvider");
    register::<AdvertisedEndpointCompatibility>(registry, "AdvertisedEndpointCompatibility");
    register::<AdvertisedEndpoint>(registry, "AdvertisedEndpoint");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("remote_access", register_module)
}
