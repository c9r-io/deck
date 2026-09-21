//! Thin STDIO MCP adapter for Deck's local authenticated control service.
//!
//! stdout belongs exclusively to the official MCP SDK transport. The adapter
//! does not launch Deck, tmux, a model, or an agent; it validates tool inputs,
//! forwards one bounded request to Deck's user-only Unix socket, and maps the
//! structured response back to MCP.
//!
//! Tools are one static registry: `tools/list` and `capabilities.tools` are
//! the same list (`tests/stdio.rs` checks). Mutating tools are annotated
//! `readOnlyHint=false, destructiveHint=true, idempotentHint=false`; a client
//! may choose to hide them, which is a client policy, not a Deck state.
//!
//! The bearer credential is sent only to a socket at the fixed private path
//! (`<home from getpwuid>/.deck/mcp-control.sock`) whose peer has this
//! process's effective uid. Release builds accept no socket, environment or
//! credential-descriptor override; those exist only in debug builds for the
//! synthetic harnesses. Once a request has been written, a transport failure
//! is `OPERATION_AMBIGUOUS` for side-effecting tools: the caller must query
//! or replay with the SAME request_id, never a new one.

#![allow(dead_code)] // schema carriers are intentionally validated then forwarded as JSON

use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use schemars::JsonSchema;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD: Option<&str> = match option_env!("DECK_BUILD_SHA") {
    Some(value) => Some(value),
    None => option_env!("GITHUB_SHA"),
};
/// Deck may legitimately take longer than one runner round trip: a request
/// can wait for the delivery lock behind an in-flight job dispatch.
const RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const MUTATING: [&str; 6] = [
    "deck_session_create",
    "deck_session_control",
    "deck_exec",
    "deck_job_input",
    "deck_job_interrupt",
    "deck_session_close",
];
const MAX_REQUEST: usize = 256 * 1024;
const MAX_RESPONSE: u64 = 128 * 1024;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(JsonSchema)]
struct StructuredOutput {
    ok: bool,
    #[schemars(flatten)]
    fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateInput {
    /// Stable idempotency key for this side effect.
    request_id: String,
    /// Deck project identifier returned by deck_capabilities.
    project_id: String,
    /// Authorized absolute working directory.
    cwd: String,
    /// Optional visible card title.
    #[serde(default)]
    title: Option<String>,
    /// This client's current create sequence (`nextCreateSequence` from
    /// deck_capabilities or deck_sessions_list). Deck accepts a create only at
    /// the current value; a retry of the same logical create reuses it.
    create_sequence: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OperationInput {
    operation_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SessionInput {
    session_id: String,
    /// Optional current flow holder used to evaluate mayStartNextJob.
    #[serde(default)]
    holder_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectPathInput {
    project_id: String,
    #[serde(default)]
    root_index: usize,
    #[serde(default)]
    path: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectReadInput {
    project_id: String,
    #[serde(default)]
    root_index: usize,
    path: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProjectSearchInput {
    project_id: String,
    #[serde(default)]
    root_index: usize,
    #[serde(default)]
    path: String,
    query: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ControlAction {
    Request,
    Renew,
    Release,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ControlInput {
    /// Stable idempotency key. ASCII letters, digits, underscores, and hyphens; at most 128 bytes.
    request_id: String,
    session_id: String,
    expected_generation: String,
    action: ControlAction,
    /// Stable identity for this control flow; another flow cannot replace it.
    holder_id: String,
    /// Required for renew and release; omitted for the initial request.
    #[serde(default)]
    control_epoch: Option<u64>,
    /// The session's current control sequence (`controlSequence` from
    /// deck_session_inspect, deck_sessions_list, or the last control
    /// response). Every accepted control change advances it; a retry of the
    /// same logical change reuses the value it was first sent with.
    control_sequence: u64,
    /// Optional for request and renew; 1000..=300000 milliseconds. Not allowed for release.
    #[serde(default)]
    #[schemars(range(min = 1000, max = 300000))]
    lease_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecCommonInput {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    holder_id: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    execution_timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DirectExecInput {
    #[serde(flatten)]
    common: ExecCommonInput,
    /// Executable path or name resolved through Deck's sanitized PATH. The
    /// execution grant permits arbitrary programs, including interpreters and shells.
    executable: String,
    /// Exact argv entries. They are visible in host process metadata, so do
    /// not place secrets in arguments.
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadInput {
    job_id: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    max_bytes: Option<usize>,
    #[serde(default)]
    wait_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct JobInputInput {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
    holder_id: String,
    input: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InterruptInput {
    request_id: String,
    job_id: String,
    session_generation: String,
    control_epoch: u64,
    holder_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseInput {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    holder_id: String,
    #[serde(default)]
    confirm_running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeckRequest<'a> {
    version: u32,
    client_id: &'a str,
    credential: &'a str,
    tool: &'a str,
    arguments: Value,
}

#[derive(Clone)]
struct DeckServer {
    client_id: String,
    credential: String,
    socket: PathBuf,
    tools: Vec<Tool>,
}

fn annotations(read_only: bool, destructive: bool, idempotent: bool) -> ToolAnnotations {
    ToolAnnotations::new()
        .read_only(read_only)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(!read_only)
}

fn tool<T: JsonSchema + 'static>(
    name: &'static str,
    description: &'static str,
    hints: ToolAnnotations,
) -> Tool {
    Tool::new(name, description, Map::new())
        .with_input_schema::<T>()
        .with_output_schema::<StructuredOutput>()
        .with_annotations(hints)
}

impl DeckServer {
    fn new(client_id: String, credential: String, socket: PathBuf) -> Self {
        let ro = annotations(true, false, true);
        // Exact replay is idempotent only inside Deck's retention window, so
        // no side-effecting tool claims idempotence.
        let mutating = annotations(false, true, false);
        let tools = vec![
            tool::<Empty>(
                "deck_capabilities",
                "Report Deck connection, trusted-host execution semantics, limits, and the workspaces authorized for this client. Call this first.",
                ro.clone(),
            ),
            tool::<ProjectPathInput>(
                "deck_project_list",
                "List one approved project directory through Deck's bounded descriptor-relative reader (at most 32 entries by name; truncated=true when more exist). This does not start a shell or follow symbolic links.",
                ro.clone(),
            ),
            tool::<ProjectReadInput>(
                "deck_project_read",
                "Read a bounded UTF-8 segment of one regular file below an approved project root. Continue only with the returned version-bound cursor.",
                ro.clone(),
            ),
            tool::<ProjectSearchInput>(
                "deck_project_search",
                "Perform a bounded literal source search of one directory tree or one regular file below an approved project root, without a shell, regex engine, Git helper, or repository script. complete=false with a stopReason and skipped count means some files were not covered.",
                ro.clone(),
            ),
            tool::<Empty>(
                "deck_sessions_list",
                "List only MCP-managed Deck sessions authorized for this client. Quiet output is not reported as readiness.",
                ro.clone(),
            ),
            tool::<CreateInput>(
                "deck_session_create",
                "Request creation of a visible dedicated Deck shell card. This has side effects and returns an operation whose committed state must be checked before execution.",
                mutating.clone(),
            ),
            tool::<OperationInput>(
                "deck_operation_get",
                "Read a Deck control operation. Operation commitment is distinct from shell job completion.",
                ro.clone(),
            ),
            tool::<SessionInput>(
                "deck_session_inspect",
                "Inspect one authorized MCP session: its generation, control owner and epoch, human lock, execution window, output-sharing gate, and active job metadata. It never returns terminal screen content.",
                ro.clone(),
            ),
            tool::<ControlInput>(
                "deck_session_control",
                "Request, renew, or release a fenced write-control lease. A read never acquires control, and user takeover cannot be overridden by this tool.",
                mutating.clone(),
            ),
            tool::<DirectExecInput>(
                "deck_exec",
                "Launch any program, including an interpreter or shell, with an exact argument vector in the visible managed Deck pane. This runs as the logged-in user and is not a sandbox. No implicit shell parses the arguments; argv is visible in host process metadata, so do not put secrets there.",
                mutating.clone(),
            ),
            tool::<ReadInput>(
                "deck_job_read",
                "Incrementally read retained PTY-combined output and the independently reported exit state of one authorized job. This never re-executes the script.",
                ro.clone(),
            ),
            tool::<JobInputInput>(
                "deck_job_input",
                "Send bytes to the exact still-running managed job through its bound stdin pipe. Refuses rather than falling back to terminal typing.",
                mutating.clone(),
            ),
            tool::<InterruptInput>(
                "deck_job_interrupt",
                "Request SIGINT for the exact active job process group. A successful request is not a confirmed process exit; read the job afterward.",
                mutating.clone(),
            ),
            tool::<CloseInput>(
                "deck_session_close",
                "Close an authorized MCP-managed session through Deck's ordinary schedule, tmux, and Board transaction path. A running job is refused unless explicitly confirmed.",
                mutating,
            ),
        ];
        Self {
            client_id,
            credential,
            socket,
            tools,
        }
    }

    async fn invoke<T: DeserializeOwned>(
        &self,
        tool_name: &'static str,
        arguments: Value,
    ) -> CallToolResult {
        if serde_json::from_value::<T>(arguments.clone()).is_err() {
            return CallToolResult::structured_error(json!({
                "ok": false,
                "error": {"code":"INVALID_ARGUMENTS","message":"Tool arguments do not match the advertised schema.","nextAction":"Correct the arguments and retry with the same request_id only if the prior request was not accepted."}
            }));
        }
        let socket = self.socket.clone();
        let client_id = self.client_id.clone();
        let credential = self.credential.clone();
        let mutating = MUTATING.contains(&tool_name);
        let result = tokio::task::spawn_blocking(move || {
            let mut stream = UnixStream::connect(socket).map_err(|_| "DECK_UNAVAILABLE")?;
            // The bearer goes only to a peer running as this same user.
            if !peer_is_same_user(&stream) {
                return Err("DECK_UNAVAILABLE");
            }
            stream
                .set_read_timeout(Some(RESPONSE_TIMEOUT))
                .map_err(|_| "DECK_UNAVAILABLE")?;
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                .map_err(|_| "DECK_UNAVAILABLE")?;
            let request = DeckRequest {
                version: 5,
                client_id: &client_id,
                credential: &credential,
                tool: tool_name,
                arguments,
            };
            let mut frame = serde_json::to_vec(&request).map_err(|_| "INTERNAL_ERROR")?;
            frame.push(b'\n');
            if frame.len() > MAX_REQUEST {
                return Err("REQUEST_TOO_LARGE");
            }
            // From the first written byte on, Deck may have acted.
            let sent_failure = if mutating {
                "OPERATION_AMBIGUOUS"
            } else {
                "DECK_UNAVAILABLE"
            };
            stream.write_all(&frame).map_err(|_| sent_failure)?;
            stream.flush().map_err(|_| sent_failure)?;
            let mut line = Vec::new();
            BufReader::new(stream)
                .take(MAX_RESPONSE + 1)
                .read_until(b'\n', &mut line)
                .map_err(|_| sent_failure)?;
            if line.is_empty() {
                // The connection closed without an answer.
                return Err(sent_failure);
            }
            if line.len() as u64 > MAX_RESPONSE || !line.ends_with(b"\n") {
                return Err(if mutating {
                    "OPERATION_AMBIGUOUS"
                } else {
                    "INTERNAL_ERROR"
                });
            }
            serde_json::from_slice::<Value>(&line).map_err(|_| "INTERNAL_ERROR")
        })
        .await;
        match result {
            Ok(Ok(value)) if value.get("ok").and_then(Value::as_bool) == Some(false) => {
                CallToolResult::structured_error(value)
            }
            Ok(Ok(mut value)) => {
                if tool_name == "deck_capabilities" {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("adapterVersion".into(), json!(VERSION));
                        object.insert("adapterBuild".into(), json!(BUILD));
                        object.insert(
                            "tools".into(),
                            json!(self
                                .tools
                                .iter()
                                .map(|tool| tool.name.as_ref())
                                .collect::<Vec<_>>()),
                        );
                        object.insert(
                            "toolsSemantics".into(),
                            json!("Adapter-exposed tools; each call remains subject to client authorization and runtime admission."),
                        );
                    }
                }
                CallToolResult::structured(value)
            }
            Ok(Err("OPERATION_AMBIGUOUS")) => CallToolResult::structured_error(json!({
                "ok": false,
                "error": {"code":"OPERATION_AMBIGUOUS","message":"The request reached Deck but its answer was lost; the side effect may or may not have happened.","nextAction":"Inspect with deck_operation_get / deck_session_inspect, or repeat this call with the SAME request_id. Never retry with a new request_id."}
            })),
            Ok(Err(code)) => CallToolResult::structured_error(json!({
                "ok": false,
                "error": {"code":code,"message":"Deck is not available through its local control socket.","nextAction":"Open Deck, enable MCP control, and verify this client id is authorized."}
            })),
            Err(_) => CallToolResult::structured_error(json!({
                "ok": false,
                "error": {"code":"INTERNAL_ERROR","message":"The adapter worker failed.","nextAction":"Restart the adapter; do not repeat a side effect until checking its request_id."}
            })),
        }
    }

    async fn dispatch(&self, name: &str, arguments: Value) -> CallToolResult {
        match name {
            "deck_capabilities" => self.invoke::<Empty>("deck_capabilities", arguments).await,
            "deck_project_list" => {
                self.invoke::<ProjectPathInput>("deck_project_list", arguments)
                    .await
            }
            "deck_project_read" => {
                self.invoke::<ProjectReadInput>("deck_project_read", arguments)
                    .await
            }
            "deck_project_search" => {
                self.invoke::<ProjectSearchInput>("deck_project_search", arguments)
                    .await
            }
            "deck_sessions_list" => self.invoke::<Empty>("deck_sessions_list", arguments).await,
            "deck_session_create" => {
                if let Err(issue) = validate_sequence(
                    &arguments,
                    "create_sequence",
                    "non-negative integer (nextCreateSequence from deck_capabilities or deck_sessions_list)",
                ) {
                    return invalid_arguments(issue);
                }
                self.invoke::<CreateInput>("deck_session_create", arguments)
                    .await
            }
            "deck_operation_get" => {
                self.invoke::<OperationInput>("deck_operation_get", arguments)
                    .await
            }
            "deck_session_inspect" => {
                self.invoke::<SessionInput>("deck_session_inspect", arguments)
                    .await
            }
            "deck_session_control" => {
                if let Err(issue) = validate_control_arguments(&arguments) {
                    return invalid_arguments(issue);
                }
                self.invoke::<ControlInput>("deck_session_control", arguments)
                    .await
            }
            "deck_exec" => self.invoke::<DirectExecInput>("deck_exec", arguments).await,
            "deck_job_read" => self.invoke::<ReadInput>("deck_job_read", arguments).await,
            "deck_job_input" => {
                self.invoke::<JobInputInput>("deck_job_input", arguments)
                    .await
            }
            "deck_job_interrupt" => {
                self.invoke::<InterruptInput>("deck_job_interrupt", arguments)
                    .await
            }
            "deck_session_close" => {
                self.invoke::<CloseInput>("deck_session_close", arguments)
                    .await
            }
            _ => CallToolResult::structured_error(json!({
                "ok": false,
                "error": {"code":"UNSUPPORTED","message":"Unknown Deck tool.","nextAction":"Refresh tools/list and use one of the advertised tools."}
            })),
        }
    }
}

#[derive(Debug)]
struct ArgumentIssue {
    field_path: &'static str,
    category: &'static str,
    expected: &'static str,
}

fn invalid_arguments(issue: ArgumentIssue) -> CallToolResult {
    CallToolResult::structured_error(json!({
        "ok": false,
        "error": {
            "code": "INVALID_ARGUMENTS",
            "message": "Tool arguments do not match the advertised schema.",
            "details": {
                "fieldPath": issue.field_path,
                "category": issue.category,
                "expected": issue.expected
            },
            "nextAction": if issue.category == "required" {
                "Supply the identified field. If your tool list does not show it, the list is stale: refresh tool discovery (reconnect the MCP server) and retry."
            } else {
                "Correct the identified field and use a new request_id unless retrying the exact same request."
            }
        }
    }))
}

fn valid_control_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// A missing sequence names the field: a client that omits it is usually
/// holding a tool list discovered before the field existed, and a generic
/// schema error would not say so.
fn validate_sequence(
    arguments: &Value,
    field: &'static str,
    expected: &'static str,
) -> Result<(), ArgumentIssue> {
    let value = arguments.as_object().and_then(|object| object.get(field));
    if value.and_then(Value::as_u64).is_some() {
        return Ok(());
    }
    Err(ArgumentIssue {
        field_path: field,
        category: if value.is_some() { "type" } else { "required" },
        expected,
    })
}

fn validate_control_arguments(arguments: &Value) -> Result<(), ArgumentIssue> {
    let object = arguments.as_object().ok_or(ArgumentIssue {
        field_path: "$",
        category: "type",
        expected: "object",
    })?;
    for field in [
        "request_id",
        "session_id",
        "expected_generation",
        "action",
        "holder_id",
    ] {
        if object.get(field).and_then(Value::as_str).is_none() {
            return Err(ArgumentIssue {
                field_path: field,
                category: if object.contains_key(field) {
                    "type"
                } else {
                    "required"
                },
                expected: "non-null string",
            });
        }
    }
    for field in ["request_id", "holder_id"] {
        if !valid_control_id(object[field].as_str().unwrap_or_default()) {
            return Err(ArgumentIssue {
                field_path: field,
                category: "format",
                expected: "1..=128 ASCII letters, digits, underscores, or hyphens",
            });
        }
    }
    validate_sequence(
        arguments,
        "control_sequence",
        "non-negative integer (controlSequence from deck_session_inspect)",
    )?;
    let action = object["action"].as_str().unwrap_or_default();
    if !matches!(action, "request" | "renew" | "release") {
        return Err(ArgumentIssue {
            field_path: "action",
            category: "enum",
            expected: "request, renew, or release",
        });
    }
    let epoch = object.get("control_epoch").filter(|value| !value.is_null());
    if epoch.is_some_and(|value| value.as_u64().is_none()) {
        return Err(ArgumentIssue {
            field_path: "control_epoch",
            category: "type",
            expected: "non-negative integer or null",
        });
    }
    if action == "request" && epoch.is_some() {
        return Err(ArgumentIssue {
            field_path: "control_epoch",
            category: "not_allowed",
            expected: "omitted or null for request",
        });
    }
    if matches!(action, "renew" | "release") && epoch.is_none() {
        return Err(ArgumentIssue {
            field_path: "control_epoch",
            category: "required",
            expected: "non-negative integer for renew and release",
        });
    }
    let lease = object.get("lease_ms").filter(|value| !value.is_null());
    if action == "release" && lease.is_some() {
        return Err(ArgumentIssue {
            field_path: "lease_ms",
            category: "not_allowed",
            expected: "omitted or null for release",
        });
    }
    if let Some(value) = lease {
        let Some(value) = value.as_u64() else {
            return Err(ArgumentIssue {
                field_path: "lease_ms",
                category: "type",
                expected: "integer in 1000..=300000",
            });
        };
        if !(1_000..=300_000).contains(&value) {
            return Err(ArgumentIssue {
                field_path: "lease_ms",
                category: "range",
                expected: "integer in 1000..=300000",
            });
        }
    }
    Ok(())
}

impl ServerHandler for DeckServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Use Deck MCP only for authorized development work. Call deck_capabilities first; retain operation, session, job, generation, cursor, control epoch, control sequence and create sequence values; never blindly retry ambiguous work.")
            .with_server_info(rmcp::model::Implementation::new("deck-mcp", VERSION))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tools = self.tools.clone();
        async move {
            Ok(ListToolsResult {
                tools,
                ..Default::default()
            })
        }
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.tools.iter().find(|tool| tool.name == name).cloned()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        Ok(self.dispatch(&request.name, arguments).await.into())
    }
}

#[cfg(target_os = "macos")]
fn peer_is_same_user(stream: &UnixStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut uid = 0;
    let mut gid = 0;
    // SAFETY: getpeereid writes two scalars for this connected socket.
    unsafe {
        libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) == 0 && uid == libc::geteuid()
    }
}

#[cfg(not(target_os = "macos"))]
fn peer_is_same_user(_stream: &UnixStream) -> bool {
    false
}

/// The account's home directory from the user database, not `$HOME`: an
/// inherited environment must not redirect the credential.
fn account_home() -> Option<PathBuf> {
    use std::ffi::CStr;
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: zeroed passwd is a valid out-parameter for getpwuid_r.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    // SAFETY: every pointer refers to live, correctly sized storage.
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    // SAFETY: pw_dir points into `buffer`, NUL-terminated by getpwuid_r.
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    use std::os::unix::ffi::OsStrExt;
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes())))
}

const USAGE: &str = "usage: deck-mcp --client-id ID";

fn parse_args() -> Result<(String, PathBuf, Option<u32>), &'static str> {
    let mut args = std::env::args_os().skip(1);
    let mut client_id = None;
    // Test seams exist only in debug builds; a release adapter always talks
    // to the one private Deck socket and reads the Keychain.
    #[cfg(debug_assertions)]
    let mut socket = std::env::var_os("DECK_MCP_SOCKET").map(PathBuf::from);
    #[cfg(not(debug_assertions))]
    let socket: Option<PathBuf> = None;
    #[cfg_attr(not(debug_assertions), allow(unused_mut))]
    let mut credential_fd = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--client-id") => {
                client_id = args.next().and_then(|value| value.into_string().ok())
            }
            #[cfg(debug_assertions)]
            Some("--socket") => socket = args.next().map(PathBuf::from),
            #[cfg(debug_assertions)]
            Some("--credential-fd") => {
                credential_fd = args.next().and_then(|value| value.to_str()?.parse().ok())
            }
            _ => return Err(USAGE),
        }
    }
    let client_id = client_id.filter(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    });
    let socket = socket.or_else(|| account_home().map(|home| home.join(".deck/mcp-control.sock")));
    match (client_id, socket) {
        (Some(client), Some(path)) if path.is_absolute() => Ok((client, path, credential_fd)),
        _ => Err(USAGE),
    }
}

#[tokio::main]
async fn main() {
    let (client_id, socket, credential_fd) = match parse_args() {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            std::process::exit(64);
        }
    };
    let credential = if let Some(fd) = credential_fd {
        let mut value = String::new();
        match std::fs::File::open(format!("/dev/fd/{fd}"))
            .and_then(|file| file.take(129).read_to_string(&mut value))
        {
            Ok(_) if value.len() <= 128 => value,
            _ => {
                eprintln!("deck-mcp credential pipe is invalid");
                std::process::exit(78);
            }
        }
    } else {
        match security_framework::passwords::get_generic_password("io.c9r.deck.mcp", &client_id) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(value) => value,
                Err(_) => {
                    eprintln!("deck-mcp credential is invalid");
                    std::process::exit(78);
                }
            },
            Err(_) => {
                eprintln!(
                    "deck-mcp credential is unavailable; reauthorize this integration in Deck"
                );
                std::process::exit(78);
            }
        }
    };
    let service = DeckServer::new(client_id, credential, socket)
        .serve(rmcp::transport::stdio())
        .await;
    match service {
        Ok(service) => {
            if service.waiting().await.is_err() {
                eprintln!("deck-mcp transport ended unexpectedly");
                std::process::exit(1);
            }
        }
        Err(_) => {
            eprintln!("deck-mcp could not initialize the STDIO transport");
            std::process::exit(1);
        }
    }
}
