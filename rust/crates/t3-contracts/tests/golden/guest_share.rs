use super::super::{Registry, register};
use t3_contracts::guest_share::*;
pub fn register_module(registry: &mut Registry) {
    register::<GuestRedeemRequest>(registry, "GuestRedeemRequest");
    register::<GuestShareCreateError>(registry, "GuestShareCreateError");
    register::<GuestShareCreateInput>(registry, "GuestShareCreateInput");
    register::<GuestShareCreateResult>(registry, "GuestShareCreateResult");
    register::<GuestSubmitRequest>(registry, "GuestSubmitRequest");
    register::<ThreadGuestMessage>(registry, "ThreadGuestMessage");
    register::<ThreadGuestMessageStatus>(registry, "ThreadGuestMessageStatus");
    register::<ThreadGuestShare>(registry, "ThreadGuestShare");
}
#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("guest_share", register_module)
}
