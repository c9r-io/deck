use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CAPABILITIES: [&str; 5] = ["status", "start", "stop", "setup", "remove"];

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolResponse<'a> {
    pub protocol_version: u32,
    pub tool_version: &'a str,
    pub capabilities: &'a [&'a str],
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    TunnelClientMissing,
    NotConfigured,
    KeyMissing,
    Stopped,
    Starting,
    Ready,
    Unhealthy,
    Stale,
    Error,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub protocol_version: u32,
    pub state: TunnelState,
    pub runtime_alias: String,
    pub runtime_exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionResponse {
    pub protocol_version: u32,
    pub ok: bool,
    pub state: TunnelState,
    pub runtime_alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<&'static str>,
}

pub fn protocol_response() -> ProtocolResponse<'static> {
    ProtocolResponse {
        protocol_version: PROTOCOL_VERSION,
        tool_version: env!("CARGO_PKG_VERSION"),
        capabilities: &CAPABILITIES,
    }
}
