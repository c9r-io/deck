//! Wire request envelope and argument shapes for the control socket.
//!
//! Split out of the one-file `mcp.rs` on 2026-09-23; the contract stays in
//! `mcp/mod.rs`.

use super::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct WireRequest {
    pub(super) version: u32,
    pub(super) client_id: String,
    pub(super) credential: String,
    pub(super) tool: String,
    pub(super) arguments: Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Empty {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CreateArgs {
    pub(super) request_id: String,
    pub(super) project_id: String,
    pub(super) cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) title: Option<String>,
    pub(super) create_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct OperationArgs {
    pub(super) operation_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SessionArgs {
    pub(super) session_id: String,
    #[serde(default)]
    pub(super) holder_id: Option<String>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ControlAction {
    Request,
    Renew,
    Release,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ControlArgs {
    pub(super) request_id: String,
    pub(super) session_id: String,
    pub(super) expected_generation: String,
    pub(super) action: ControlAction,
    pub(super) holder_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) control_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) lease_ms: Option<u64>,
    pub(super) control_sequence: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ExecCommon {
    pub(super) request_id: String,
    pub(super) session_id: String,
    pub(super) expected_generation: String,
    pub(super) control_epoch: u64,
    pub(super) holder_id: String,
    #[serde(default)]
    pub(super) cwd: Option<String>,
    #[serde(default)]
    pub(super) wait_ms: Option<u64>,
    #[serde(default)]
    pub(super) execution_timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectExecArgs {
    #[serde(flatten)]
    pub(super) common: ExecCommon,
    pub(super) executable: String,
    #[serde(default)]
    pub(super) args: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReadArgs {
    pub(super) job_id: String,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) max_bytes: Option<usize>,
    #[serde(default)]
    pub(super) wait_ms: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InputArgs {
    pub(super) request_id: String,
    pub(super) job_id: String,
    pub(super) session_generation: String,
    pub(super) control_epoch: u64,
    pub(super) holder_id: String,
    pub(super) input: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InterruptArgs {
    pub(super) request_id: String,
    pub(super) job_id: String,
    pub(super) session_generation: String,
    pub(super) control_epoch: u64,
    pub(super) holder_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CloseArgs {
    pub(super) request_id: String,
    pub(super) session_id: String,
    pub(super) expected_generation: String,
    pub(super) control_epoch: u64,
    pub(super) holder_id: String,
    #[serde(default)]
    pub(super) confirm_running: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectPathArgs {
    pub(super) project_id: String,
    #[serde(default)]
    pub(super) root_index: usize,
    #[serde(default)]
    pub(super) path: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct FileReadArgs {
    pub(super) project_id: String,
    #[serde(default)]
    pub(super) root_index: usize,
    pub(super) path: String,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) max_bytes: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SearchArgs {
    pub(super) project_id: String,
    #[serde(default)]
    pub(super) root_index: usize,
    #[serde(default)]
    pub(super) path: String,
    pub(super) query: String,
    #[serde(default)]
    pub(super) cursor: Option<String>,
    #[serde(default)]
    pub(super) max_results: Option<usize>,
}

pub(super) fn parse<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, Value> {
    serde_json::from_value(value).map_err(|_| {
        error_value(
            "INVALID_ARGUMENTS",
            "arguments do not match the tool schema",
            "Correct the arguments and retry only after checking any prior request id.",
        )
    })
}
