//! `lumo mcp` — Model Context Protocol server over stdio.
//!
//! Speaks JSON-RPC 2.0 (line-delimited JSON) on stdin/stdout so LumoRPA can be
//! mounted as a tool inside Claude Desktop / Cursor / any MCP-aware client.
//! The exposed tools cover the same surface as `lumo run` and `lumo runs`:
//!   * `list_flows`      — list `*.lumoflow.{yaml,yml}` under `--flows`
//!   * `validate_flow`   — parse + structural validate
//!   * `run_flow`        — execute and return the run report
//!   * `list_runs`       — browse persistent run history
//!   * `get_run`         — fetch a single run with its step rows
//!
//! Resources surface every flow file as `file://…` URIs:
//!   * `resources/list`  — enumerate flow files with metadata
//!   * `resources/read`  — return raw YAML; path-traversal guarded against
//!     `--flows` root
//!
//! Notifications (no `id`) are silently consumed so `initialized`, `cancelled`
//! and `progress` frames don't trip the dispatcher.

use clap::Args as ClapArgs;
use lumo_core::{FlowVm, RunOptions};
use lumo_storage::Repo;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::build_action_registry;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Directory containing flow YAML files exposed via MCP tools.
    #[arg(long, default_value = "./flows")]
    pub flows: PathBuf,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    std::fs::create_dir_all(&home)?;
    let server = Server::new(home, args.flows);
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF — client disconnected
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = server.handle_line(trimmed).await;
        if let Some(payload) = response {
            stdout.write_all(payload.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

// ─── Server ─────────────────────────────────────────────────────────────────

pub(crate) struct Server {
    home: PathBuf,
    flows: PathBuf,
}

impl Server {
    pub(crate) fn new(home: PathBuf, flows: PathBuf) -> Self {
        Self { home, flows }
    }

    /// Parse one JSON-RPC line and return the serialized response (or `None`
    /// for notifications). Errors are surfaced as JSON-RPC errors so the
    /// peer always gets structured feedback.
    pub(crate) async fn handle_line(&self, line: &str) -> Option<String> {
        let req: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return Some(encode(JsonRpcResponse::error(
                    Value::Null,
                    -32700,
                    format!("parse error: {e}"),
                )));
            }
        };
        // Notifications have no `id` and want no response.
        let id = req.id.clone()?;
        let outcome = self.dispatch(&req).await;
        let resp = match outcome {
            Ok(value) => JsonRpcResponse::result(id, value),
            Err((code, message)) => JsonRpcResponse::error(id, code, message),
        };
        Some(encode(resp))
    }

    async fn dispatch(&self, req: &JsonRpcRequest) -> Result<Value, (i32, String)> {
        match req.method.as_str() {
            "initialize" => Ok(self.initialize()),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(req.params.clone()).await,
            "resources/list" => Ok(self.resources_list()),
            "resources/read" => self.resources_read(req.params.clone()),
            "ping" => Ok(json!({})),
            other => Err((-32601, format!("method not found: {other}"))),
        }
    }

    fn initialize(&self) -> Value {
        json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "subscribe": false, "listChanged": false }
            },
            "serverInfo": {
                "name": "lumorpa",
                "version": env!("CARGO_PKG_VERSION"),
            },
        })
    }

    fn tools_list(&self) -> Value {
        json!({
            "tools": [
                {
                    "name": "list_flows",
                    "description": "List available LumoFlow files under the configured flows directory.",
                    "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
                },
                {
                    "name": "validate_flow",
                    "description": "Parse and structurally validate a flow YAML file. Returns metadata, step count, and any warnings.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["path"],
                        "properties": { "path": { "type": "string" } },
                        "additionalProperties": false
                    }
                },
                {
                    "name": "run_flow",
                    "description": "Execute a flow file with optional JSON inputs. Returns the run report (run_id, success, step counts, outputs).",
                    "inputSchema": {
                        "type": "object",
                        "required": ["path"],
                        "properties": {
                            "path": { "type": "string" },
                            "inputs": { "type": "object" }
                        },
                        "additionalProperties": false
                    }
                },
                {
                    "name": "list_runs",
                    "description": "Browse persisted run history. `limit` defaults to 20.",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "limit": { "type": "integer", "minimum": 1, "maximum": 500 } },
                        "additionalProperties": false
                    }
                },
                {
                    "name": "get_run",
                    "description": "Fetch a single run with its step rows by run id.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["run_id"],
                        "properties": { "run_id": { "type": "string" } },
                        "additionalProperties": false
                    }
                }
            ]
        })
    }

    async fn tools_call(&self, params: Value) -> Result<Value, (i32, String)> {
        let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
        let value = match name {
            "list_flows" => self.tool_list_flows()?,
            "validate_flow" => self.tool_validate_flow(arguments)?,
            "run_flow" => self.tool_run_flow(arguments).await?,
            "list_runs" => self.tool_list_runs(arguments)?,
            "get_run" => self.tool_get_run(arguments)?,
            "" => return Err((-32602, "tools/call requires `name`".into())),
            other => return Err((-32602, format!("unknown tool: {other}"))),
        };
        Ok(wrap_text(value))
    }

    /// MCP `resources/list` — expose every flow file under `--flows` as a
    /// readable resource so Claude/Cursor can pull the YAML directly without
    /// a tool round-trip.
    fn resources_list(&self) -> Value {
        let mut resources: Vec<Value> = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.flows) else {
            return json!({ "resources": resources });
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_flow_path(&path) {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let (display_name, description) = match lumo_dsl::parse_file(&path) {
                Ok(f) => (
                    f.metadata.name.unwrap_or_else(|| f.metadata.id.clone()),
                    f.metadata.description.unwrap_or_default(),
                ),
                Err(_) => (name.to_string(), String::new()),
            };
            resources.push(json!({
                "uri": format!("file://{}", path.display()),
                "name": display_name,
                "description": description,
                "mimeType": "application/x-yaml",
            }));
        }
        resources.sort_by(|a, b| {
            a["uri"]
                .as_str()
                .unwrap_or("")
                .cmp(b["uri"].as_str().unwrap_or(""))
        });
        json!({ "resources": resources })
    }

    /// MCP `resources/read` — return the raw YAML body for a flow URI. The
    /// URI must start with `file://` and resolve to a path under `--flows`.
    fn resources_read(&self, params: Value) -> Result<Value, (i32, String)> {
        let uri = params
            .get("uri")
            .and_then(|v| v.as_str())
            .ok_or((-32602, "resources/read requires `uri`".into()))?;
        let path_str = uri
            .strip_prefix("file://")
            .ok_or((-32602, format!("unsupported uri scheme: {uri}")))?;
        let path = PathBuf::from(path_str);
        let canonical_path = path
            .canonicalize()
            .map_err(|e| (-32001, format!("canonicalize {}: {e}", path.display())))?;
        let canonical_root = self.flows.canonicalize().map_err(|e| {
            (
                -32001,
                format!("canonicalize {}: {e}", self.flows.display()),
            )
        })?;
        if !canonical_path.starts_with(&canonical_root) {
            return Err((
                -32002,
                format!(
                    "resource path escapes flows root: {} not under {}",
                    canonical_path.display(),
                    canonical_root.display()
                ),
            ));
        }
        if !is_flow_path(&canonical_path) {
            return Err((-32002, "resource is not a .lumoflow.yaml file".into()));
        }
        let body = std::fs::read_to_string(&canonical_path)
            .map_err(|e| (-32001, format!("read {}: {e}", canonical_path.display())))?;
        Ok(json!({
            "contents": [{
                "uri": uri,
                "mimeType": "application/x-yaml",
                "text": body,
            }]
        }))
    }

    fn tool_list_flows(&self) -> Result<Value, (i32, String)> {
        let mut flows: Vec<Value> = Vec::new();
        let entries = std::fs::read_dir(&self.flows)
            .map_err(|e| (-32000, format!("read {}: {e}", self.flows.display())))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !is_flow_path(&path) {
                continue;
            }
            let parsed = lumo_dsl::parse_file(&path);
            let (id, name, description, step_count, valid, error) = match parsed.as_ref() {
                Ok(f) => (
                    Some(f.metadata.id.clone()),
                    f.metadata.name.clone(),
                    f.metadata.description.clone(),
                    Some(count_steps(&f.spec.steps)),
                    lumo_dsl::validate(f).is_ok(),
                    lumo_dsl::validate(f).err().map(|e| e.to_string()),
                ),
                Err(e) => (None, None, None, None, false, Some(e.to_string())),
            };
            flows.push(json!({
                "path": path.display().to_string(),
                "id": id,
                "name": name,
                "description": description,
                "step_count": step_count,
                "valid": valid,
                "error": error,
            }));
        }
        flows.sort_by(|a, b| {
            a["path"]
                .as_str()
                .unwrap_or("")
                .cmp(b["path"].as_str().unwrap_or(""))
        });
        Ok(json!({ "flows": flows }))
    }

    fn tool_validate_flow(&self, args: Value) -> Result<Value, (i32, String)> {
        let path = require_path(&args, "path")?;
        let flow = lumo_dsl::parse_file(&path).map_err(|e| (-32001, e.to_string()))?;
        lumo_dsl::validate(&flow).map_err(|e| (-32001, e.to_string()))?;
        Ok(json!({
            "id": flow.metadata.id,
            "version": flow.metadata.version,
            "name": flow.metadata.name,
            "description": flow.metadata.description,
            "step_count": count_steps(&flow.spec.steps),
            "capabilities": serde_json::to_value(&flow.spec.capabilities).unwrap_or(Value::Null),
            "trigger_kinds": flow.spec.triggers.iter().map(|t| t.kind.clone()).collect::<Vec<_>>(),
        }))
    }

    async fn tool_run_flow(&self, args: Value) -> Result<Value, (i32, String)> {
        let path = require_path(&args, "path")?;
        let inputs = args
            .get("inputs")
            .cloned()
            .unwrap_or(Value::Object(serde_json::Map::new()));
        if !(inputs.is_object() || inputs.is_null()) {
            return Err((-32602, "inputs must be a JSON object".into()));
        }
        let flow = lumo_dsl::parse_file(&path).map_err(|e| (-32001, e.to_string()))?;
        lumo_dsl::validate(&flow).map_err(|e| (-32001, e.to_string()))?;
        let registry = build_action_registry(&self.home, Some(&path));
        let repo =
            Some(Repo::open(self.home.join("lumo.db")).map_err(|e| (-32002, e.to_string()))?);
        let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), &self.home, &flow);
        let report = vm
            .run(
                &flow,
                RunOptions {
                    inputs,
                    trigger_kind: "mcp".into(),
                },
            )
            .await
            .map_err(|e| (-32003, e.to_string()))?;
        Ok(json!({
            "run_id": report.run_id,
            "success": report.success,
            "steps_total": report.steps_total,
            "steps_ok": report.steps_ok,
            "steps_failed": report.steps_failed,
            "duration_ms": report.duration_ms,
            "outputs": report.outputs,
        }))
    }

    fn tool_list_runs(&self, args: Value) -> Result<Value, (i32, String)> {
        let limit = args
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n.clamp(1, 500) as u32)
            .unwrap_or(20);
        let repo = Repo::open(self.home.join("lumo.db")).map_err(|e| (-32002, e.to_string()))?;
        let rows = repo.list_runs(limit).map_err(|e| (-32002, e.to_string()))?;
        let runs: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                json!({
                    "id": r.id,
                    "flow_id": r.flow_id,
                    "flow_version": r.flow_version,
                    "trigger_kind": r.trigger_kind,
                    "state": r.state,
                    "started_at": r.started_at.map(|t| t.to_rfc3339()),
                    "finished_at": r.finished_at.map(|t| t.to_rfc3339()),
                })
            })
            .collect();
        Ok(json!({ "runs": runs }))
    }

    fn tool_get_run(&self, args: Value) -> Result<Value, (i32, String)> {
        let run_id = args
            .get("run_id")
            .and_then(|v| v.as_str())
            .ok_or((-32602, "run_id is required".into()))?;
        let repo = Repo::open(self.home.join("lumo.db")).map_err(|e| (-32002, e.to_string()))?;
        let run = repo
            .get_run(run_id)
            .map_err(|e| (-32002, e.to_string()))?
            .ok_or((-32004, format!("run `{run_id}` not found")))?;
        let steps = repo
            .list_steps(run_id)
            .map_err(|e| (-32002, e.to_string()))?;
        let steps_json: Vec<Value> = steps
            .into_iter()
            .map(|s| {
                json!({
                    "seq": s.seq,
                    "path": s.path,
                    "step_id": s.step_id,
                    "state": s.state,
                    "attempt": s.attempt,
                    "output_json": s.output_json,
                    "error": s.error,
                    "duration_ms": match (s.started_at, s.finished_at) {
                        (Some(a), Some(b)) => Some(b.timestamp_millis() - a.timestamp_millis()),
                        _ => None,
                    },
                })
            })
            .collect();
        Ok(json!({
            "run": {
                "id": run.id,
                "flow_id": run.flow_id,
                "flow_version": run.flow_version,
                "trigger_kind": run.trigger_kind,
                "state": run.state,
                "started_at": run.started_at.map(|t| t.to_rfc3339()),
                "finished_at": run.finished_at.map(|t| t.to_rfc3339()),
                "inputs": run.inputs,
                "outputs": run.outputs,
            },
            "steps": steps_json,
        }))
    }
}

// ─── JSON-RPC framing ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcErrorPayload>,
}

impl JsonRpcResponse {
    fn result(id: Value, value: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(value),
            error: None,
        }
    }
    fn error(id: Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcErrorPayload { code, message }),
        }
    }
}

#[derive(Debug, Serialize)]
struct JsonRpcErrorPayload {
    code: i32,
    message: String,
}

fn encode(resp: JsonRpcResponse) -> String {
    serde_json::to_string(&resp).unwrap_or_else(|_| "{}".into())
}

// ─── Helpers ────────────────────────────────────────────────────────────────

fn wrap_text(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into());
    json!({
        "content": [{ "type": "text", "text": text }]
    })
}

fn require_path(args: &Value, key: &str) -> Result<PathBuf, (i32, String)> {
    let s = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or((-32602, format!("`{key}` is required and must be a string")))?;
    Ok(PathBuf::from(s))
}

fn is_flow_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.ends_with(".lumoflow.yaml") || n.ends_with(".lumoflow.yml"))
        .unwrap_or(false)
}

fn count_steps(steps: &[lumo_dsl::Step]) -> usize {
    steps
        .iter()
        .map(|step| 1 + step.children().into_iter().map(count_steps).sum::<usize>())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_flow(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(format!("{name}.lumoflow.yaml")), body.trim_start()).unwrap();
    }

    fn sample_flow(id: &str) -> String {
        format!(
            r#"
apiVersion: lumorpa.io/v1
kind: Flow
metadata: {{ id: {id} }}
spec:
  steps:
    - {{ id: hi, action: control.log, with: {{ message: "hello from {id}" }} }}
"#,
        )
    }

    fn test_server() -> (TempDir, TempDir, Server) {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        (home, flows, s)
    }

    async fn call(server: &Server, line: &str) -> Value {
        let raw = server.handle_line(line).await.expect("response expected");
        serde_json::from_str(&raw).unwrap()
    }

    fn unwrap_tool_text(resp: &Value) -> Value {
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("text payload");
        serde_json::from_str(text).expect("text is JSON")
    }

    #[tokio::test]
    async fn initialize_returns_protocol_version() {
        let (_h, _f, s) = test_server();
        let resp = call(
            &s,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .await;
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(resp["result"]["serverInfo"]["name"], "lumorpa");
    }

    #[tokio::test]
    async fn tools_list_returns_five_tools() {
        let (_h, _f, s) = test_server();
        let resp = call(&s, r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#).await;
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for required in [
            "list_flows",
            "validate_flow",
            "run_flow",
            "list_runs",
            "get_run",
        ] {
            assert!(names.contains(&required), "missing tool {required}");
        }
    }

    #[tokio::test]
    async fn unknown_method_returns_minus_32601() {
        let (_h, _f, s) = test_server();
        let resp = call(&s, r#"{"jsonrpc":"2.0","id":3,"method":"banana"}"#).await;
        assert_eq!(resp["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        let (_h, _f, s) = test_server();
        let resp = s
            .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .await;
        assert!(resp.is_none());
    }

    #[tokio::test]
    async fn list_flows_finds_yaml_files() {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "alpha", &sample_flow("alpha"));
        write_flow(flows.path(), "beta", &sample_flow("beta"));
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        let resp = call(
            &s,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_flows","arguments":{}}}"#,
        )
        .await;
        let payload = unwrap_tool_text(&resp);
        let arr = payload["flows"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert!(arr.iter().all(|f| f["valid"] == true));
    }

    #[tokio::test]
    async fn validate_flow_reports_metadata() {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "demo", &sample_flow("demo"));
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        let p = flows
            .path()
            .join("demo.lumoflow.yaml")
            .display()
            .to_string();
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{{"name":"validate_flow","arguments":{{"path":"{p}"}}}}}}"#,
        );
        let resp = call(&s, &line).await;
        let payload = unwrap_tool_text(&resp);
        assert_eq!(payload["id"], "demo");
        assert_eq!(payload["step_count"], 1);
    }

    #[tokio::test]
    async fn run_flow_returns_run_id_and_persists() {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "demo", &sample_flow("demo"));
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        let p = flows
            .path()
            .join("demo.lumoflow.yaml")
            .display()
            .to_string();
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{{"name":"run_flow","arguments":{{"path":"{p}"}}}}}}"#,
        );
        let resp = call(&s, &line).await;
        let payload = unwrap_tool_text(&resp);
        assert_eq!(payload["success"], true);
        let run_id = payload["run_id"].as_str().unwrap().to_string();
        assert!(!run_id.is_empty());

        // list_runs should now see it.
        let resp_list = call(
            &s,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_runs","arguments":{}}}"#,
        )
        .await;
        let runs = unwrap_tool_text(&resp_list);
        let arr = runs["runs"].as_array().unwrap();
        assert!(arr.iter().any(|r| r["id"] == run_id));
        assert!(arr.iter().any(|r| r["trigger_kind"] == "mcp"));
    }

    #[tokio::test]
    async fn unknown_tool_returns_error() {
        let (_h, _f, s) = test_server();
        let resp = call(
            &s,
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"banana"}}"#,
        )
        .await;
        assert_eq!(resp["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn malformed_line_emits_parse_error() {
        let (_h, _f, s) = test_server();
        let raw = s
            .handle_line("not json at all")
            .await
            .expect("error response");
        let resp: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(resp["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn initialize_advertises_resources_capability() {
        let (_h, _f, s) = test_server();
        let resp = call(&s, r#"{"jsonrpc":"2.0","id":7,"method":"initialize"}"#).await;
        assert_eq!(
            resp["result"]["capabilities"]["resources"]["listChanged"],
            false
        );
        assert_eq!(
            resp["result"]["capabilities"]["resources"]["subscribe"],
            false
        );
    }

    #[tokio::test]
    async fn resources_list_returns_flow_files() {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "alpha", &sample_flow("alpha"));
        write_flow(flows.path(), "beta", &sample_flow("beta"));
        // Drop a non-flow file to confirm it is filtered out.
        std::fs::write(flows.path().join("README.md"), "hello").unwrap();
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        let resp = call(&s, r#"{"jsonrpc":"2.0","id":8,"method":"resources/list"}"#).await;
        let resources = resp["result"]["resources"].as_array().unwrap();
        assert_eq!(resources.len(), 2);
        for r in resources {
            let uri = r["uri"].as_str().unwrap();
            assert!(uri.starts_with("file://"));
            assert!(uri.ends_with(".lumoflow.yaml"));
            assert_eq!(r["mimeType"], "application/x-yaml");
        }
    }

    #[tokio::test]
    async fn resources_read_returns_yaml_body() {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "alpha", &sample_flow("alpha"));
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        let real_path = flows
            .path()
            .canonicalize()
            .unwrap()
            .join("alpha.lumoflow.yaml");
        let uri = format!("file://{}", real_path.display());
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"resources/read","params":{{"uri":"{uri}"}}}}"#,
        );
        let resp = call(&s, &line).await;
        let contents = resp["result"]["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1);
        let text = contents[0]["text"].as_str().unwrap();
        assert!(text.contains("apiVersion"));
        assert!(text.contains("alpha"));
    }

    #[tokio::test]
    async fn resources_read_rejects_path_outside_flows_root() {
        let home = TempDir::new().unwrap();
        let flows = TempDir::new().unwrap();
        write_flow(flows.path(), "alpha", &sample_flow("alpha"));
        let outside = TempDir::new().unwrap();
        let evil = outside.path().join("secret.lumoflow.yaml");
        std::fs::write(&evil, "shhh").unwrap();
        let s = Server::new(home.path().to_path_buf(), flows.path().to_path_buf());
        let uri = format!("file://{}", evil.canonicalize().unwrap().display());
        let line = format!(
            r#"{{"jsonrpc":"2.0","id":10,"method":"resources/read","params":{{"uri":"{uri}"}}}}"#,
        );
        let resp = call(&s, &line).await;
        assert_eq!(resp["error"]["code"], -32002);
    }

    #[tokio::test]
    async fn resources_read_rejects_non_file_uri() {
        let (_h, _f, s) = test_server();
        let resp = call(
            &s,
            r#"{"jsonrpc":"2.0","id":11,"method":"resources/read","params":{"uri":"http://example.com/x"}}"#,
        )
        .await;
        assert_eq!(resp["error"]["code"], -32602);
    }
}
