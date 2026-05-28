//! `mcp.call` — invoke a tool on an external MCP (Model Context Protocol) server.
//!
//! Spawns the server as a child process, performs the JSON-RPC 2.0 handshake
//! over stdio (`initialize` → `notifications/initialized` → `tools/call`), and
//! returns the tool's content array.

use async_trait::async_trait;
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

pub fn register(r: &mut ActionRegistry) {
    r.register(McpCallAction);
    r.register(McpDiscoverAction);
}

pub struct McpCallAction;
pub struct McpDiscoverAction;

#[derive(Deserialize)]
struct CallIn {
    server: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    tool: String,
    #[serde(default)]
    arguments: Value,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}
fn default_timeout_ms() -> u64 {
    30_000
}

#[async_trait]
impl Action for McpCallAction {
    fn id(&self) -> &'static str {
        "mcp.call"
    }
    fn summary(&self) -> &'static str {
        "Invoke a tool on an external MCP server (JSON-RPC over stdio)"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            json!({
                "type": "object",
                "required": ["server", "command", "tool"],
                "properties": {
                    "server": { "type": "string", "description": "Capability-gated server name." },
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "env": { "type": "object", "additionalProperties": { "type": "string" } },
                    "tool": { "type": "string" },
                    "arguments": { "type": "object" },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let cfg: CallIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("mcp.call input invalid: {e}")))?;
        ctx.ensure_mcp_tool(&cfg.server, &cfg.tool)?;

        let arguments = if cfg.arguments.is_null() {
            json!({})
        } else if cfg.arguments.is_object() {
            cfg.arguments.clone()
        } else {
            return Err(StepError::msg(
                "mcp.call `arguments` must be an object or omitted",
            ));
        };

        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| StepError::msg(format!("mcp.call spawn `{}`: {e}", cfg.command)))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| StepError::msg("mcp.call: child stdin missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| StepError::msg("mcp.call: child stdout missing"))?;
        let mut reader = BufReader::new(stdout);

        let deadline = Duration::from_millis(cfg.timeout_ms);
        let result = timeout(deadline, async {
            // initialize
            let init = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "lumo-rpa", "version": env!("CARGO_PKG_VERSION") }
                }
            });
            write_line(&mut stdin, &init).await?;
            let _ = read_response_for(&mut reader, 1).await?;

            // initialized notification (no response expected)
            let notif = json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            });
            write_line(&mut stdin, &notif).await?;

            // tools/call
            let call = json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": { "name": cfg.tool, "arguments": arguments }
            });
            write_line(&mut stdin, &call).await?;
            read_response_for(&mut reader, 2).await
        })
        .await;

        let _ = child.kill().await;

        let resp = result.map_err(|_| {
            StepError::msg(format!("mcp.call timed out after {}ms", cfg.timeout_ms))
        })??;

        if let Some(err) = resp.get("error") {
            return Err(StepError::msg(format!("mcp.call server error: {err}")));
        }
        let result = resp
            .get("result")
            .cloned()
            .unwrap_or_else(|| json!({ "content": [] }));
        Ok(ActionResult::from(result))
    }
}

#[derive(Deserialize)]
struct DiscoverIn {
    server: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: HashMap<String, String>,
    #[serde(default = "default_timeout_ms")]
    timeout_ms: u64,
}

#[async_trait]
impl Action for McpDiscoverAction {
    fn id(&self) -> &'static str {
        "mcp.discover"
    }
    fn summary(&self) -> &'static str {
        "Connect to an MCP server and return its `tools/list` descriptor array"
    }
    fn schema(&self) -> &'static Value {
        static SCHEMA: Lazy<Value> = Lazy::new(|| {
            json!({
                "type": "object",
                "required": ["server", "command"],
                "properties": {
                    "server": { "type": "string", "description": "Capability-gated server name." },
                    "command": { "type": "string" },
                    "args": { "type": "array", "items": { "type": "string" } },
                    "env": { "type": "object", "additionalProperties": { "type": "string" } },
                    "timeout_ms": { "type": "integer", "minimum": 1 }
                },
                "additionalProperties": false
            })
        });
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let cfg: DiscoverIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("mcp.discover input invalid: {e}")))?;
        ctx.ensure_mcp_server(&cfg.server)?;

        let mut child = Command::new(&cfg.command)
            .args(&cfg.args)
            .envs(&cfg.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| StepError::msg(format!("mcp.discover spawn `{}`: {e}", cfg.command)))?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| StepError::msg("mcp.discover: child stdin missing"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| StepError::msg("mcp.discover: child stdout missing"))?;
        let mut reader = BufReader::new(stdout);

        let deadline = Duration::from_millis(cfg.timeout_ms);
        let result = timeout(deadline, async {
            let init = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": { "name": "lumo-rpa", "version": env!("CARGO_PKG_VERSION") }
                }
            });
            write_line(&mut stdin, &init).await?;
            let _ = read_response_for(&mut reader, 1).await?;

            let notif = json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            });
            write_line(&mut stdin, &notif).await?;

            let list = json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list"
            });
            write_line(&mut stdin, &list).await?;
            read_response_for(&mut reader, 2).await
        })
        .await;

        let _ = child.kill().await;

        let resp = result.map_err(|_| {
            StepError::msg(format!("mcp.discover timed out after {}ms", cfg.timeout_ms))
        })??;

        if let Some(err) = resp.get("error") {
            return Err(StepError::msg(format!("mcp.discover server error: {err}")));
        }
        let tools = resp
            .get("result")
            .and_then(|r| r.get("tools"))
            .cloned()
            .unwrap_or_else(|| json!([]));
        let descriptors = tools
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|t| {
                        let name = t.get("name").and_then(|n| n.as_str())?.to_string();
                        let description = t
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();
                        let proposed_grant = format!("{}:{name}", cfg.server);
                        let already_allowed = ctx.ensure_mcp_tool(&cfg.server, &name).is_ok();
                        Some(json!({
                            "name": name,
                            "description": description,
                            "input_schema": t.get("inputSchema").cloned().unwrap_or(Value::Null),
                            "proposed_grant": proposed_grant,
                            "already_allowed": already_allowed
                        }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(ActionResult::from(json!({
            "server": cfg.server,
            "count": descriptors.len(),
            "tools": descriptors
        })))
    }
}

async fn write_line<W: AsyncWriteExt + Unpin>(w: &mut W, value: &Value) -> Result<(), StepError> {
    let mut line = serde_json::to_string(value)
        .map_err(|e| StepError::msg(format!("mcp.call encode: {e}")))?;
    line.push('\n');
    w.write_all(line.as_bytes())
        .await
        .map_err(|e| StepError::msg(format!("mcp.call write: {e}")))?;
    w.flush()
        .await
        .map_err(|e| StepError::msg(format!("mcp.call flush: {e}")))?;
    Ok(())
}

async fn read_response_for<R: AsyncBufReadExt + Unpin>(
    r: &mut R,
    want_id: u64,
) -> Result<Value, StepError> {
    loop {
        let mut buf = String::new();
        let n = r
            .read_line(&mut buf)
            .await
            .map_err(|e| StepError::msg(format!("mcp.call read: {e}")))?;
        if n == 0 {
            return Err(StepError::msg("mcp.call: server closed stdout"));
        }
        let line = buf.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|e| StepError::msg(format!("mcp.call decode `{line}`: {e}")))?;
        // Ignore notifications and unrelated ids; only match the request id.
        if value.get("id").and_then(|v| v.as_u64()) == Some(want_id) {
            return Ok(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumo_core::ActionRegistry;
    use lumo_dsl::Capabilities;

    fn make_ctx(mcp_allow: Vec<String>) -> StepCtx {
        let caps = Capabilities {
            mcp: mcp_allow,
            ..Default::default()
        };
        StepCtx::new(
            "run-1".into(),
            "flow-1".into(),
            ActionRegistry::new(),
            None,
            json!({}),
            caps,
            vec![],
        )
    }

    fn locate_lumo_bin() -> Option<std::path::PathBuf> {
        let exe = std::env::current_exe().ok()?;
        // target/debug/deps/<test-bin>  →  target/debug/lumo
        let mut path = exe.parent()?.to_path_buf();
        if path.ends_with("deps") {
            path.pop();
        }
        let candidate = path.join(if cfg!(windows) { "lumo.exe" } else { "lumo" });
        candidate.exists().then_some(candidate)
    }

    #[tokio::test]
    async fn capability_gate_blocks_undeclared_server() {
        let mut ctx = make_ctx(vec![]);
        let res = McpCallAction
            .execute(
                &mut ctx,
                json!({
                    "server": "github",
                    "command": "/bin/true",
                    "tool": "noop"
                }),
            )
            .await;
        match res {
            Err(StepError::CapabilityDenied { target, .. }) => assert_eq!(target, "github:noop"),
            other => panic!("expected CapabilityDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn invalid_input_returns_message_error() {
        let mut ctx = make_ctx(vec!["*".into()]);
        let res = McpCallAction
            .execute(&mut ctx, json!({ "server": "x" }))
            .await;
        assert!(matches!(res, Err(StepError::Message(_))));
    }

    #[tokio::test]
    async fn arguments_must_be_object() {
        let mut ctx = make_ctx(vec!["lumo".into()]);
        let res = McpCallAction
            .execute(
                &mut ctx,
                json!({
                    "server": "lumo",
                    "command": "/bin/true",
                    "tool": "noop",
                    "arguments": [1, 2, 3]
                }),
            )
            .await;
        match res {
            Err(StepError::Message(m)) => assert!(m.contains("arguments"), "msg: {m}"),
            other => panic!("expected Message error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn spawn_failure_propagates() {
        let mut ctx = make_ctx(vec!["lumo".into()]);
        let res = McpCallAction
            .execute(
                &mut ctx,
                json!({
                    "server": "lumo",
                    "command": "/no/such/binary/lumo-x-x-x",
                    "tool": "noop"
                }),
            )
            .await;
        match res {
            Err(StepError::Message(m)) => assert!(m.contains("spawn"), "msg: {m}"),
            other => panic!("expected spawn error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn timeout_kills_unresponsive_server() {
        // `sleep` does not read stdin or write to stdout → handshake times out.
        let mut ctx = make_ctx(vec!["sleep".into()]);
        let res = McpCallAction
            .execute(
                &mut ctx,
                json!({
                    "server": "sleep",
                    "command": "/bin/sleep",
                    "args": ["10"],
                    "tool": "noop",
                    "timeout_ms": 150
                }),
            )
            .await;
        match res {
            Err(StepError::Message(m)) => assert!(m.contains("timed out"), "msg: {m}"),
            other => panic!("expected timeout, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn round_trip_against_lumo_mcp_server() {
        let Some(bin) = locate_lumo_bin() else {
            eprintln!("[skip] lumo binary not built; run `cargo build -p lumo-cli` first");
            return;
        };
        let flows = tempfile::TempDir::new().unwrap();
        std::fs::write(
            flows.path().join("ping.lumoflow.yaml"),
            "apiVersion: lumo/v1\nkind: Flow\nmetadata:\n  name: ping\nspec:\n  steps:\n    - id: a\n      action: data.set\n      with:\n        x: 1\n",
        )
        .unwrap();
        let mut ctx = make_ctx(vec!["lumo".into()]);
        let res = McpCallAction
            .execute(
                &mut ctx,
                json!({
                    "server": "lumo",
                    "command": bin.to_string_lossy(),
                    "args": ["mcp", "--flows", flows.path().to_string_lossy()],
                    "tool": "list_flows",
                    "timeout_ms": 8_000
                }),
            )
            .await
            .expect("round-trip");
        let content = res.output.get("content").and_then(|c| c.as_array());
        assert!(content.is_some(), "result missing content array: {:?}", res);
        let text = content.unwrap()[0]
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("");
        assert!(
            text.contains("ping"),
            "expected `ping` in tool response, got: {text}"
        );
    }

    #[tokio::test]
    async fn tool_grant_allows_exact_match_only() {
        let mut ctx = make_ctx(vec!["lumo:list_flows".into()]);
        // allowed
        assert!(ctx.ensure_mcp_tool("lumo", "list_flows").is_ok());
        // server-only call without tool gate is still server-level allowed for discover
        assert!(ctx.ensure_mcp_server("lumo").is_ok());
        // different tool blocked
        let res = McpCallAction
            .execute(
                &mut ctx,
                json!({
                    "server": "lumo",
                    "command": "/bin/true",
                    "tool": "run_flow"
                }),
            )
            .await;
        match res {
            Err(StepError::CapabilityDenied { target, .. }) => {
                assert_eq!(target, "lumo:run_flow")
            }
            other => panic!("expected per-tool CapabilityDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_grant_supports_wildcard() {
        let ctx = make_ctx(vec!["lumo:list_*".into()]);
        assert!(ctx.ensure_mcp_tool("lumo", "list_flows").is_ok());
        assert!(ctx.ensure_mcp_tool("lumo", "list_runs").is_ok());
        assert!(ctx.ensure_mcp_tool("lumo", "run_flow").is_err());
    }

    #[tokio::test]
    async fn discover_capability_gate_blocks_undeclared_server() {
        let mut ctx = make_ctx(vec![]);
        let res = McpDiscoverAction
            .execute(
                &mut ctx,
                json!({
                    "server": "github",
                    "command": "/bin/true"
                }),
            )
            .await;
        match res {
            Err(StepError::CapabilityDenied { target, .. }) => assert_eq!(target, "github"),
            other => panic!("expected CapabilityDenied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discover_against_lumo_mcp_returns_five_tools() {
        let Some(bin) = locate_lumo_bin() else {
            eprintln!("[skip] lumo binary not built; run `cargo build -p lumo-cli` first");
            return;
        };
        let flows = tempfile::TempDir::new().unwrap();
        // grant only one specific tool so already_allowed has variance
        let mut ctx = make_ctx(vec!["lumo:list_flows".into()]);
        let res = McpDiscoverAction
            .execute(
                &mut ctx,
                json!({
                    "server": "lumo",
                    "command": bin.to_string_lossy(),
                    "args": ["mcp", "--flows", flows.path().to_string_lossy()],
                    "timeout_ms": 8_000
                }),
            )
            .await
            .expect("discover");
        assert_eq!(res.output.get("count").and_then(|v| v.as_u64()), Some(5));
        let tools = res.output.get("tools").and_then(|v| v.as_array()).unwrap();
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"list_flows"));
        assert!(names.contains(&"run_flow"));
        // already_allowed reflects per-tool grant state
        let list_flows = tools
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("list_flows"))
            .unwrap();
        let run_flow = tools
            .iter()
            .find(|t| t.get("name").and_then(|n| n.as_str()) == Some("run_flow"))
            .unwrap();
        assert_eq!(
            list_flows.get("already_allowed").and_then(|v| v.as_bool()),
            Some(true)
        );
        assert_eq!(
            run_flow.get("already_allowed").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            run_flow.get("proposed_grant").and_then(|v| v.as_str()),
            Some("lumo:run_flow")
        );
    }
}
