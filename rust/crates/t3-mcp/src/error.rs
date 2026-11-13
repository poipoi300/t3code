use serde_json::{Value, json};

#[derive(Clone, Debug, thiserror::Error)]
pub enum McpError {
    #[error("{tool} is not available: {method} has not been implemented or connected.")]
    NotAvailable { tool: String, method: String },
    #[error("{message}")]
    Failure { code: String, message: String },
    #[error("{message}")]
    Domain {
        tag: String,
        message: String,
        fields: Value,
    },
    #[error("{0}")]
    InvalidInput(String),
    #[error("Tool execution failed due to an internal server error.")]
    Internal,
}

impl McpError {
    pub fn failure(code: &str, message: impl Into<String>) -> Self {
        Self::Failure {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn unavailable() -> Self {
        Self::failure(
            "orchestration_error",
            "The operation could not be completed.",
        )
    }

    pub fn encoded(&self) -> Value {
        match self {
            Self::Failure { code, message } => {
                json!({"_tag": "OrchestratorMcpFailure", "code": code, "message":message})
            }
            Self::NotAvailable { .. } => {
                json!({"_tag":"OrchestratorMcpFailure","code":"orchestration_error","message":self.to_string()})
            }
            Self::Domain { tag, fields, .. } => {
                let mut value = fields.clone();
                value["_tag"] = json!(tag);
                value
            }
            _ => Value::Null,
        }
    }
}

pub type Result<T> = std::result::Result<T, McpError>;
