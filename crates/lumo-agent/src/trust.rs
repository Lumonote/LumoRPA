use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentOrigin {
    CodeOwned,
    User,
    Model,
    McpTool,
    ToolResult,
    Web,
    Email,
    Trace,
}

impl ContentOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CodeOwned => "code_owned",
            Self::User => "user",
            Self::Model => "model",
            Self::McpTool => "mcp_tool",
            Self::ToolResult => "tool_result",
            Self::Web => "web",
            Self::Email => "email",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentEnvelope {
    pub origin: ContentOrigin,
    pub content: String,
}

impl ContentEnvelope {
    pub fn new(origin: ContentOrigin, content: impl Into<String>) -> Self {
        Self {
            origin,
            content: content.into(),
        }
    }

    pub fn as_prompt_data(&self) -> String {
        format!(
            "<untrusted-data origin=\"{}\">{}</untrusted-data>",
            self.origin.as_str(),
            escape_xml(&self.content)
        )
    }
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlPlaneField {
    Policy,
    Budget,
    Approval,
    ToolVisibility,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    #[error("untrusted {origin:?} content cannot change {field:?}")]
    UntrustedControlChange {
        origin: ContentOrigin,
        field: ControlPlaneField,
    },
}

pub struct TrustGuard;

impl TrustGuard {
    pub fn authorize_control_change(
        origin: ContentOrigin,
        field: ControlPlaneField,
    ) -> Result<(), TrustError> {
        if origin == ContentOrigin::CodeOwned {
            Ok(())
        } else {
            Err(TrustError::UntrustedControlChange { origin, field })
        }
    }
}
