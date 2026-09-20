//! Thin STDIO MCP adapter for Deck's local authenticated control service.
//!
//! stdout belongs exclusively to the official MCP SDK transport. The adapter
//! does not launch Deck, tmux, a model, or an agent; it validates tool inputs,
//! forwards one bounded request to Deck's user-only Unix socket, and maps the
//! structured response back to MCP.

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
    request_id: String,
    session_id: String,
    expected_generation: String,
    action: ControlAction,
    /// Stable identity for this control flow; another flow cannot replace it.
    holder_id: String,
    #[serde(default)]
    control_epoch: Option<u64>,
    #[serde(default)]
    lease_ms: Option<u64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExecInput {
    request_id: String,
    session_id: String,
    expected_generation: String,
    control_epoch: u64,
    holder_id: String,
    script: String,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    wait_ms: Option<u64>,
    #[serde(default)]
    execution_timeout_ms: Option<u64>,
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
        let mutating = annotations(false, true, true);
        let tools = vec![
            tool::<Empty>(
                "deck_capabilities",
                "Report Deck connection, trusted-host execution semantics, limits, and the workspaces authorized for this client. Call this first.",
                ro.clone(),
            ),
            tool::<ProjectPathInput>(
                "deck_project_list",
                "List one approved project directory through Deck's bounded descriptor-relative reader. This does not start a shell or follow symbolic links.",
                ro.clone(),
            ),
            tool::<ProjectReadInput>(
                "deck_project_read",
                "Read a bounded UTF-8 segment of one regular file below an approved project root. Continue only with the returned version-bound cursor.",
                ro.clone(),
            ),
            tool::<ProjectSearchInput>(
                "deck_project_search",
                "Perform a bounded literal source search below an approved project root without a shell, regex engine, Git helper, or repository script.",
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
                "Inspect one authorized MCP session, its generation, control owner, active job, and bounded terminal context.",
                ro.clone(),
            ),
            tool::<ControlInput>(
                "deck_session_control",
                "Request, renew, or release a fenced write-control lease. A read never acquires control, and user takeover cannot be overridden by this tool.",
                mutating.clone(),
            ),
            tool::<ExecInput>(
                "deck_exec",
                "Execute an arbitrary user-authorized script as one fresh zsh job in the visible managed Deck pane. Files persist, but cd, export, aliases, and functions do not cross calls.",
                annotations(false, true, true),
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
        let result = tokio::task::spawn_blocking(move || {
            let mut stream = UnixStream::connect(socket).map_err(|_| "DECK_UNAVAILABLE")?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .map_err(|_| "DECK_UNAVAILABLE")?;
            stream
                .set_write_timeout(Some(std::time::Duration::from_secs(10)))
                .map_err(|_| "DECK_UNAVAILABLE")?;
            let request = DeckRequest {
                version: 3,
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
            stream.write_all(&frame).map_err(|_| "DECK_UNAVAILABLE")?;
            stream.flush().map_err(|_| "DECK_UNAVAILABLE")?;
            let mut line = Vec::new();
            BufReader::new(stream)
                .take(MAX_RESPONSE + 1)
                .read_until(b'\n', &mut line)
                .map_err(|_| "DECK_UNAVAILABLE")?;
            if line.len() as u64 > MAX_RESPONSE || !line.ends_with(b"\n") {
                return Err("INTERNAL_ERROR");
            }
            serde_json::from_slice::<Value>(&line).map_err(|_| "INTERNAL_ERROR")
        })
        .await;
        match result {
            Ok(Ok(value)) if value.get("ok").and_then(Value::as_bool) == Some(false) => {
                CallToolResult::structured_error(value)
            }
            Ok(Ok(value)) => CallToolResult::structured(value),
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
                self.invoke::<ControlInput>("deck_session_control", arguments)
                    .await
            }
            "deck_exec" => self.invoke::<ExecInput>("deck_exec", arguments).await,
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

impl ServerHandler for DeckServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Use Deck MCP only for authorized development work. Call deck_capabilities first; retain operation, session, job, generation, cursor, and control epoch values; never blindly retry ambiguous work.")
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

fn parse_args() -> Result<(String, PathBuf, Option<u32>), &'static str> {
    let mut args = std::env::args_os().skip(1);
    let mut client_id = None;
    let mut socket = std::env::var_os("DECK_MCP_SOCKET").map(PathBuf::from);
    let mut credential_fd = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--client-id") => {
                client_id = args.next().and_then(|value| value.into_string().ok())
            }
            Some("--socket") => socket = args.next().map(PathBuf::from),
            Some("--credential-fd") => {
                credential_fd = args.next().and_then(|value| value.to_str()?.parse().ok())
            }
            _ => return Err("usage: deck-mcp --client-id ID [--socket PATH]"),
        }
    }
    let client_id = client_id.filter(|value| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    });
    let socket =
        socket.or_else(|| dirs::home_dir().map(|home| home.join(".deck/mcp-control.sock")));
    match (client_id, socket) {
        (Some(client), Some(path)) if path.is_absolute() => Ok((client, path, credential_fd)),
        _ => Err("usage: deck-mcp --client-id ID [--socket PATH]"),
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
