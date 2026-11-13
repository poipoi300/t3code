use super::super::{Registry, register};
use t3_contracts::peer_link::*;

pub fn register_module(registry: &mut Registry) {
    register::<PeerLink>(registry, "PeerLink");
    register::<PeerLinkCreateInput>(registry, "PeerLinkCreateInput");
    register::<PeerLinkError>(registry, "PeerLinkError");
    register::<PeerLinkRemoveInput>(registry, "PeerLinkRemoveInput");
    register::<PeerLinkRemoveResult>(registry, "PeerLinkRemoveResult");
    register::<PeerLinkRequestAnswerInput>(registry, "PeerLinkRequestAnswerInput");
    register::<PeerLinkRequestError>(registry, "PeerLinkRequestError");
    register::<PeerLinkRequestFailureReason>(registry, "PeerLinkRequestFailureReason");
    register::<PeerLinkStatus>(registry, "PeerLinkStatus");
    register::<PeerLinkSummary>(registry, "PeerLinkSummary");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("peer_link", register_module)
}
