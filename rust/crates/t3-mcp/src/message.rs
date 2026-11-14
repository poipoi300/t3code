use crate::{DecodedInput, McpError, Result};
use serde_json::{Value, json};

pub fn send_to_user(input: DecodedInput) -> Result<Value> {
    let message = input.value()["message"]
        .as_str()
        .ok_or(McpError::Internal)?;
    Ok(
        json!({"sentMessage":message,"message":crate::instructions::fixture()["sendToUserAcknowledgement"]}),
    )
}
