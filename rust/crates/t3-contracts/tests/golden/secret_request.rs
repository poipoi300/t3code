use super::super::{Registry, register};
use t3_contracts::secret_request::*;

pub fn register_module(registry: &mut Registry) {
    register::<SecretRequestAnswerInput>(registry, "SecretRequestAnswerInput");
    register::<SecretRequestFailureReason>(registry, "SecretRequestFailureReason");
    register::<SecretRequestError>(registry, "SecretRequestError");
}

#[test]
fn samples() -> Result<(), Box<dyn std::error::Error>> {
    super::super::check_module("secret_request", register_module)
}
