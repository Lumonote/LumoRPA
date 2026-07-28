//! Reusable MCP client plus the built-in `mcp.call` and `mcp.discover` actions.

#[path = "mcp/oauth.rs"]
pub mod oauth;

use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use lumo_core::error::StepError;
use lumo_core::{Action, ActionRegistry, ActionResult, StepCtx};
use once_cell::sync::Lazy;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpTransportConfig {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    StreamableHttp {
        url: String,
        headers: BTreeMap<String, String>,
    },
    Sse {
        url: String,
        headers: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
}

#[derive(Debug, Error)]
pub enum McpError {
    #[error("failed to spawn MCP server: {message}")]
    Spawn { message: String },
    #[error("MCP transport error: {message}")]
    Transport { message: String },
    #[error("MCP protocol error: {message}")]
    Protocol { message: String },
    #[error("MCP server error {code}: {message}")]
    Server {
        code: i64,
        message: String,
        data: Option<Value>,
    },
    #[error("MCP operation timed out after {duration_ms}ms")]
    Timeout { duration_ms: u64 },
    #[error("MCP tool arguments must be an object or null")]
    InvalidArguments,
}

enum McpTransport {
    Stdio {
        child: Child,
        stdin: ChildStdin,
        reader: BufReader<ChildStdout>,
    },
    StreamableHttp {
        client: reqwest::Client,
        url: reqwest::Url,
        headers: reqwest::header::HeaderMap,
        session_id: Option<String>,
    },
    Sse {
        client: reqwest::Client,
        post_url: reqwest::Url,
        headers: reqwest::header::HeaderMap,
        stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
        buffer: String,
    },
}

pub struct McpClient {
    transport: Option<McpTransport>,
    timeout: Duration,
    next_id: u64,
}

impl McpClient {
    pub async fn connect(
        config: McpTransportConfig,
        operation_timeout: Duration,
    ) -> Result<Self, McpError> {
        let transport = match config {
            McpTransportConfig::Stdio { command, args, env } => {
                let mut child = Command::new(&command)
                    .args(args)
                    .envs(env)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()
                    .map_err(|error| McpError::Spawn {
                        message: format!("`{command}`: {error}"),
                    })?;
                let stdin = child.stdin.take().ok_or_else(|| McpError::Transport {
                    message: "spawned server has no stdin".to_string(),
                })?;
                let stdout = child.stdout.take().ok_or_else(|| McpError::Transport {
                    message: "spawned server has no stdout".to_string(),
                })?;
                McpTransport::Stdio {
                    child,
                    stdin,
                    reader: BufReader::new(stdout),
                }
            }
            McpTransportConfig::StreamableHttp { url, headers } => {
                let url = reqwest::Url::parse(&url).map_err(|_| McpError::Transport {
                    message: "invalid Streamable HTTP URL".to_string(),
                })?;
                if !url.username().is_empty() || url.password().is_some() {
                    return Err(McpError::Transport {
                        message: "Streamable HTTP URL must not contain credentials".to_string(),
                    });
                }
                let mut parsed_headers = reqwest::header::HeaderMap::new();
                for (name, value) in headers {
                    let name =
                        reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
                            McpError::Transport {
                                message: "invalid caller header name".to_string(),
                            }
                        })?;
                    let value = reqwest::header::HeaderValue::from_str(&value).map_err(|_| {
                        McpError::Transport {
                            message: "invalid caller header value".to_string(),
                        }
                    })?;
                    parsed_headers.insert(name, value);
                }
                parsed_headers.remove("mcp-session-id");
                McpTransport::StreamableHttp {
                    client: reqwest::Client::new(),
                    url,
                    headers: parsed_headers,
                    session_id: None,
                }
            }
            McpTransportConfig::Sse { url, headers } => {
                let url = parse_mcp_url(&url, "SSE")?;
                let parsed_headers = parse_caller_headers(headers)?;
                let client = reqwest::Client::new();
                let response = client
                    .get(url.clone())
                    .headers(parsed_headers.clone())
                    .header(reqwest::header::ACCEPT, "text/event-stream")
                    .send()
                    .await
                    .map_err(|error| McpError::Transport {
                        message: format!("SSE connection failed: {}", error.without_url()),
                    })?;
                if !response.status().is_success() {
                    return Err(McpError::Transport {
                        message: format!(
                            "SSE server returned status {}",
                            response.status().as_u16()
                        ),
                    });
                }
                let mut stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
                    Box::pin(response.bytes_stream());
                let mut buffer = String::new();
                let post_url = timeout(
                    operation_timeout,
                    read_legacy_sse_endpoint(&mut stream, &mut buffer, &url),
                )
                .await
                .map_err(|_| McpError::Timeout {
                    duration_ms: duration_millis(operation_timeout),
                })??;
                McpTransport::Sse {
                    client,
                    post_url,
                    headers: parsed_headers,
                    stream,
                    buffer,
                }
            }
        };

        let mut client = Self {
            transport: Some(transport),
            timeout: operation_timeout,
            next_id: 1,
        };
        let initialized = async {
            let initialize_result = client
                .request(
                    "initialize",
                    Some(json!({
                        "protocolVersion": MCP_PROTOCOL_VERSION,
                        "capabilities": {},
                        "clientInfo": {
                            "name": "lumo-rpa",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    })),
                )
                .await?;
            validate_initialize_result(&initialize_result)?;
            client.notification("notifications/initialized", None).await
        };
        match timeout(operation_timeout, initialized).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                client.abort_stdio().await;
                return Err(error);
            }
            Err(_) => {
                client.abort_stdio().await;
                return Err(McpError::Timeout {
                    duration_ms: duration_millis(operation_timeout),
                });
            }
        }

        Ok(client)
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpTool>, McpError> {
        let response = self.request_with_timeout("tools/list", None).await?;
        let parsed: Result<Vec<McpTool>, McpError> = (|| {
            let tools = response
                .as_object()
                .and_then(|result| result.get("tools"))
                .and_then(Value::as_array)
                .ok_or_else(|| McpError::Protocol {
                    message: "tools/list result must contain a tools array".to_string(),
                })?;

            tools
                .iter()
                .map(|tool| {
                    let tool = tool.as_object().ok_or_else(|| McpError::Protocol {
                        message: "tool descriptor must be an object".to_string(),
                    })?;
                    let name = tool.get("name").and_then(Value::as_str).ok_or_else(|| {
                        McpError::Protocol {
                            message: "tool descriptor name must be a string".to_string(),
                        }
                    })?;
                    let description = match tool.get("description") {
                        None => "",
                        Some(Value::String(description)) => description,
                        Some(_) => {
                            return Err(McpError::Protocol {
                                message: "tool descriptor description must be a string".to_string(),
                            });
                        }
                    };
                    let input_schema = match tool.get("inputSchema") {
                        None => json!({ "type": "object" }),
                        Some(Value::Object(schema)) => Value::Object(schema.clone()),
                        Some(_) => {
                            return Err(McpError::Protocol {
                                message: format!(
                                    "tool descriptor `{name}` inputSchema must be an object"
                                ),
                            });
                        }
                    };
                    let output_schema = match tool.get("outputSchema") {
                        None => None,
                        Some(Value::Object(schema)) => Some(Value::Object(schema.clone())),
                        Some(_) => {
                            return Err(McpError::Protocol {
                                message: format!(
                                    "tool descriptor `{name}` outputSchema must be an object"
                                ),
                            });
                        }
                    };
                    Ok(McpTool {
                        name: name.to_string(),
                        description: description.to_string(),
                        input_schema,
                        output_schema,
                    })
                })
                .collect()
        })();
        if parsed.is_err() {
            self.abort_stdio().await;
        }
        parsed
    }

    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, McpError> {
        let arguments = match arguments {
            Value::Null => json!({}),
            Value::Object(_) => arguments,
            _ => return Err(McpError::InvalidArguments),
        };
        self.request_with_timeout(
            "tools/call",
            Some(json!({ "name": name, "arguments": arguments })),
        )
        .await
    }

    pub async fn close(&mut self) {
        self.abort_stdio().await;
        self.transport = None;
    }

    async fn request_with_timeout(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError> {
        match timeout(self.timeout, self.request(method, params)).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(error)) => {
                self.abort_stdio().await;
                Err(error)
            }
            Err(_) => {
                self.abort_stdio().await;
                Err(McpError::Timeout {
                    duration_ms: duration_millis(self.timeout),
                })
            }
        }
    }

    async fn request(&mut self, method: &str, params: Option<Value>) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| McpError::Protocol {
                message: "JSON-RPC request id overflow".to_string(),
            })?;
        let mut request = json!({ "jsonrpc": "2.0", "id": id, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        let response =
            self.exchange(request, Some(id))
                .await?
                .ok_or_else(|| McpError::Protocol {
                    message: "request returned no JSON-RPC response".to_string(),
                })?;
        parse_response(response)
    }

    async fn notification(&mut self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let mut notification = json!({ "jsonrpc": "2.0", "method": method });
        if let Some(params) = params {
            notification["params"] = params;
        }
        self.exchange(notification, None).await?;
        Ok(())
    }

    async fn exchange(
        &mut self,
        message: Value,
        response_id: Option<u64>,
    ) -> Result<Option<Value>, McpError> {
        match self.transport.as_mut().ok_or_else(|| McpError::Transport {
            message: "client is closed".to_string(),
        })? {
            McpTransport::Stdio { stdin, reader, .. } => {
                write_stdio_message(stdin, &message).await?;
                match response_id {
                    Some(id) => read_stdio_response(reader, id).await.map(Some),
                    None => Ok(None),
                }
            }
            McpTransport::StreamableHttp {
                client,
                url,
                headers,
                session_id,
            } => {
                let mut request = client
                    .post(url.clone())
                    .headers(headers.clone())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .header(
                        reqwest::header::ACCEPT,
                        "application/json, text/event-stream",
                    );
                if let Some(session_id) = session_id.as_deref() {
                    request = request.header("Mcp-Session-Id", session_id);
                }
                let response =
                    request
                        .json(&message)
                        .send()
                        .await
                        .map_err(|error| McpError::Transport {
                            message: format!("HTTP request failed: {}", error.without_url()),
                        })?;
                let status = response.status();
                if !status.is_success() {
                    return Err(McpError::Transport {
                        message: format!("HTTP server returned status {}", status.as_u16()),
                    });
                }
                if session_id.is_none() {
                    if let Some(value) = response.headers().get("Mcp-Session-Id") {
                        let value = value.to_str().map_err(|_| McpError::Protocol {
                            message: "Mcp-Session-Id is not valid text".to_string(),
                        })?;
                        *session_id = Some(value.to_string());
                    }
                }
                if response_id.is_none() || status == reqwest::StatusCode::NO_CONTENT {
                    return Ok(None);
                }
                let content_type = response
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let body = response.text().await.map_err(|error| McpError::Transport {
                    message: format!("failed to read HTTP response: {}", error.without_url()),
                })?;
                let id = response_id.expect("checked above");
                if content_type.starts_with("text/event-stream") {
                    parse_sse_response(&body, id).map(Some)
                } else {
                    let value =
                        serde_json::from_str(&body).map_err(|error| McpError::Protocol {
                            message: format!("invalid JSON HTTP response: {error}"),
                        })?;
                    if response_matches_id(&value, id) {
                        Ok(Some(value))
                    } else {
                        Err(McpError::Protocol {
                            message: format!("HTTP response did not contain request id {id}"),
                        })
                    }
                }
            }
            McpTransport::Sse {
                client,
                post_url,
                headers,
                stream,
                buffer,
            } => {
                let response = client
                    .post(post_url.clone())
                    .headers(headers.clone())
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .json(&message)
                    .send()
                    .await
                    .map_err(|error| McpError::Transport {
                        message: format!("SSE message POST failed: {}", error.without_url()),
                    })?;
                if !response.status().is_success() {
                    return Err(McpError::Transport {
                        message: format!(
                            "SSE message endpoint returned status {}",
                            response.status().as_u16()
                        ),
                    });
                }
                match response_id {
                    Some(id) => read_legacy_sse_response(stream, buffer, id).await.map(Some),
                    None => Ok(None),
                }
            }
        }
    }

    async fn abort_stdio(&mut self) {
        if let Some(McpTransport::Stdio { child, .. }) = self.transport.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Some(McpTransport::Stdio { mut child, .. }) = self.transport.take() {
            let _ = child.start_kill();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = child.wait().await;
                });
            }
        }
    }
}

fn duration_millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn parse_mcp_url(value: &str, transport: &str) -> Result<reqwest::Url, McpError> {
    let url = reqwest::Url::parse(value).map_err(|_| McpError::Transport {
        message: format!("invalid {transport} URL"),
    })?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(McpError::Transport {
            message: format!("{transport} URL must be HTTP(S) and must not contain credentials"),
        });
    }
    Ok(url)
}

fn parse_caller_headers(
    headers: BTreeMap<String, String>,
) -> Result<reqwest::header::HeaderMap, McpError> {
    let mut parsed = reqwest::header::HeaderMap::new();
    for (name, value) in headers {
        let name = reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|_| {
            McpError::Transport {
                message: "invalid caller header name".to_string(),
            }
        })?;
        let value =
            reqwest::header::HeaderValue::from_str(&value).map_err(|_| McpError::Transport {
                message: "invalid caller header value".to_string(),
            })?;
        parsed.insert(name, value);
    }
    parsed.remove(reqwest::header::CONTENT_TYPE);
    parsed.remove(reqwest::header::ACCEPT);
    Ok(parsed)
}

async fn read_legacy_sse_endpoint(
    stream: &mut Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: &mut String,
    source_url: &reqwest::Url,
) -> Result<reqwest::Url, McpError> {
    loop {
        let (event, data) = read_sse_event(stream, buffer).await?;
        if event.as_deref() != Some("endpoint") {
            continue;
        }
        let endpoint = source_url
            .join(data.trim())
            .map_err(|_| McpError::Protocol {
                message: "legacy SSE endpoint event contained an invalid URL".to_string(),
            })?;
        if endpoint.scheme() != source_url.scheme()
            || endpoint.host_str() != source_url.host_str()
            || endpoint.port_or_known_default() != source_url.port_or_known_default()
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
        {
            return Err(McpError::Transport {
                message: "legacy SSE message endpoint must be same-origin".to_string(),
            });
        }
        return Ok(endpoint);
    }
}

async fn read_legacy_sse_response(
    stream: &mut Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: &mut String,
    response_id: u64,
) -> Result<Value, McpError> {
    loop {
        let (_, data) = read_sse_event(stream, buffer).await?;
        let Ok(value) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        if response_matches_id(&value, response_id) {
            return Ok(value);
        }
    }
}

async fn read_sse_event(
    stream: &mut Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    buffer: &mut String,
) -> Result<(Option<String>, String), McpError> {
    loop {
        if let Some((frame, consumed)) = take_sse_frame(buffer) {
            let frame = frame.to_string();
            buffer.drain(..consumed);
            let mut event = None;
            let mut data = Vec::new();
            for line in frame.lines() {
                if let Some(value) = line.strip_prefix("event:") {
                    event = Some(value.trim().to_string());
                } else if let Some(value) = line.strip_prefix("data:") {
                    data.push(value.trim_start().to_string());
                }
            }
            if !data.is_empty() {
                return Ok((event, data.join("\n")));
            }
            continue;
        }
        let chunk = stream
            .next()
            .await
            .ok_or_else(|| McpError::Transport {
                message: "legacy SSE stream closed".to_string(),
            })?
            .map_err(|error| McpError::Transport {
                message: format!("legacy SSE stream failed: {}", error.without_url()),
            })?;
        let text = std::str::from_utf8(&chunk).map_err(|_| McpError::Protocol {
            message: "legacy SSE stream was not UTF-8".to_string(),
        })?;
        buffer.push_str(text);
    }
}

fn take_sse_frame(buffer: &str) -> Option<(&str, usize)> {
    if let Some(index) = buffer.find("\r\n\r\n") {
        Some((&buffer[..index], index + 4))
    } else {
        buffer
            .find("\n\n")
            .map(|index| (&buffer[..index], index + 2))
    }
}

async fn write_stdio_message(stdin: &mut ChildStdin, value: &Value) -> Result<(), McpError> {
    let mut line = serde_json::to_vec(value).map_err(|error| McpError::Protocol {
        message: format!("failed to encode JSON-RPC message: {error}"),
    })?;
    line.push(b'\n');
    stdin
        .write_all(&line)
        .await
        .map_err(|error| McpError::Transport {
            message: format!("failed to write to MCP server: {error}"),
        })?;
    stdin.flush().await.map_err(|error| McpError::Transport {
        message: format!("failed to flush MCP server input: {error}"),
    })
}

async fn read_stdio_response(
    reader: &mut BufReader<ChildStdout>,
    wanted_id: u64,
) -> Result<Value, McpError> {
    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .await
            .map_err(|error| McpError::Transport {
                message: format!("failed to read MCP server response: {error}"),
            })?;
        if bytes == 0 {
            return Err(McpError::Transport {
                message: "MCP server closed stdout".to_string(),
            });
        }
        if line.trim().is_empty() {
            continue;
        }
        let value = serde_json::from_str(line.trim()).map_err(|error| McpError::Protocol {
            message: format!("invalid JSON-RPC response: {error}"),
        })?;
        if response_matches_id(&value, wanted_id) {
            return Ok(value);
        }
    }
}

fn response_matches_id(value: &Value, wanted_id: u64) -> bool {
    value.get("id").and_then(Value::as_u64) == Some(wanted_id)
}

fn validate_initialize_result(result: &Value) -> Result<(), McpError> {
    let result = result.as_object().ok_or_else(|| McpError::Protocol {
        message: "initialize result must be an object".to_string(),
    })?;
    let protocol_version = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Protocol {
            message: "initialize result protocolVersion must be a string".to_string(),
        })?;
    if protocol_version != MCP_PROTOCOL_VERSION {
        return Err(McpError::Protocol {
            message: format!("initialize result protocolVersion must equal {MCP_PROTOCOL_VERSION}"),
        });
    }
    if !result.get("capabilities").is_some_and(Value::is_object) {
        return Err(McpError::Protocol {
            message: "initialize result capabilities must be an object".to_string(),
        });
    }
    let server_info = result
        .get("serverInfo")
        .and_then(Value::as_object)
        .ok_or_else(|| McpError::Protocol {
            message: "initialize result serverInfo must be an object".to_string(),
        })?;
    if !server_info.get("name").is_some_and(Value::is_string) {
        return Err(McpError::Protocol {
            message: "initialize result serverInfo.name must be a string".to_string(),
        });
    }
    if !server_info.get("version").is_some_and(Value::is_string) {
        return Err(McpError::Protocol {
            message: "initialize result serverInfo.version must be a string".to_string(),
        });
    }
    Ok(())
}

fn parse_response(response: Value) -> Result<Value, McpError> {
    let object = response.as_object().ok_or_else(|| McpError::Protocol {
        message: "JSON-RPC response must be an object".to_string(),
    })?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(McpError::Protocol {
            message: "JSON-RPC response version must be 2.0".to_string(),
        });
    }
    if let Some(error) = object.get("error") {
        let error = error.as_object().ok_or_else(|| McpError::Protocol {
            message: "JSON-RPC error must be an object".to_string(),
        })?;
        let code = error
            .get("code")
            .and_then(Value::as_i64)
            .ok_or_else(|| McpError::Protocol {
                message: "JSON-RPC error code must be an integer".to_string(),
            })?;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::Protocol {
                message: "JSON-RPC error message must be a string".to_string(),
            })?;
        return Err(McpError::Server {
            code,
            message: message.to_string(),
            data: error.get("data").cloned(),
        });
    }
    object
        .get("result")
        .cloned()
        .ok_or_else(|| McpError::Protocol {
            message: "JSON-RPC response must contain result or error".to_string(),
        })
}

fn parse_sse_response(body: &str, wanted_id: u64) -> Result<Value, McpError> {
    let mut data = Vec::new();
    for line in body.lines().chain(std::iter::once("")) {
        if let Some(value) = line.strip_prefix("data:") {
            data.push(value.strip_prefix(' ').unwrap_or(value));
        } else if line.trim().is_empty() && !data.is_empty() {
            let payload = data.join("\n");
            data.clear();
            if let Ok(value) = serde_json::from_str(&payload) {
                if response_matches_id(&value, wanted_id) {
                    return Ok(value);
                }
            }
        }
    }
    Err(McpError::Protocol {
        message: format!("SSE response did not contain request id {wanted_id}"),
    })
}

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
        if !cfg.arguments.is_null() && !cfg.arguments.is_object() {
            return Err(action_error("mcp.call", McpError::InvalidArguments));
        }

        let mut client = McpClient::connect(
            McpTransportConfig::Stdio {
                command: cfg.command,
                args: cfg.args,
                env: cfg.env.into_iter().collect(),
            },
            Duration::from_millis(cfg.timeout_ms),
        )
        .await
        .map_err(|error| action_error("mcp.call", error))?;
        let result = client
            .call_tool(&cfg.tool, cfg.arguments)
            .await
            .map_err(|error| action_error("mcp.call", error));
        client.close().await;
        let result = result?;
        Ok(ActionResult::from(result))
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
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
        static SCHEMA: Lazy<Value> = Lazy::new(crate::schema::derive::<DiscoverIn>);
        &SCHEMA
    }
    async fn execute(&self, ctx: &mut StepCtx, input: Value) -> Result<ActionResult, StepError> {
        let cfg: DiscoverIn = serde_json::from_value(input)
            .map_err(|e| StepError::msg(format!("mcp.discover input invalid: {e}")))?;
        ctx.ensure_mcp_server(&cfg.server)?;

        let mut client = McpClient::connect(
            McpTransportConfig::Stdio {
                command: cfg.command,
                args: cfg.args,
                env: cfg.env.into_iter().collect(),
            },
            Duration::from_millis(cfg.timeout_ms),
        )
        .await
        .map_err(|error| action_error("mcp.discover", error))?;
        let tools = client
            .list_tools()
            .await
            .map_err(|error| action_error("mcp.discover", error));
        client.close().await;
        let descriptors = tools?
            .into_iter()
            .map(|tool| {
                let proposed_grant = format!("{}:{}", cfg.server, tool.name);
                let already_allowed = ctx.ensure_mcp_tool(&cfg.server, &tool.name).is_ok();
                json!({
                    "name": tool.name,
                    "description": tool.description,
                    "input_schema": tool.input_schema,
                    "proposed_grant": proposed_grant,
                    "already_allowed": already_allowed
                })
            })
            .collect::<Vec<_>>();

        Ok(ActionResult::from(json!({
            "server": cfg.server,
            "count": descriptors.len(),
            "tools": descriptors
        })))
    }
}

fn action_error(action: &str, error: McpError) -> StepError {
    let message = match error {
        McpError::Spawn { message } => format!("{action} spawn: {message}"),
        McpError::Timeout { duration_ms } => {
            format!("{action} timed out after {duration_ms}ms")
        }
        McpError::Server {
            code,
            message,
            data,
        } => format!("{action} server error {code}: {message}; data={data:?}"),
        McpError::InvalidArguments => {
            format!("{action} `arguments` must be an object or omitted")
        }
        McpError::Transport { message } => format!("{action} transport: {message}"),
        McpError::Protocol { message } => format!("{action} protocol: {message}"),
    };
    StepError::msg(message)
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
    async fn legacy_sse_reads_same_origin_endpoint_and_json_rpc_response() {
        let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![Ok(Bytes::from_static(
            b"event: endpoint\ndata: /messages?id=7\n\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n",
        ))];
        let mut stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
            Box::pin(futures_util::stream::iter(chunks));
        let mut buffer = String::new();
        let source = reqwest::Url::parse("https://example.test/sse").unwrap();
        let endpoint = read_legacy_sse_endpoint(&mut stream, &mut buffer, &source)
            .await
            .unwrap();
        assert_eq!(endpoint.as_str(), "https://example.test/messages?id=7");
        let response = read_legacy_sse_response(&mut stream, &mut buffer, 1)
            .await
            .unwrap();
        assert_eq!(response["result"]["ok"], true);
    }

    #[tokio::test]
    async fn legacy_sse_rejects_cross_origin_message_endpoint() {
        let chunks: Vec<Result<Bytes, reqwest::Error>> = vec![Ok(Bytes::from_static(
            b"event: endpoint\ndata: https://evil.test/messages\n\n",
        ))];
        let mut stream: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
            Box::pin(futures_util::stream::iter(chunks));
        let mut buffer = String::new();
        let source = reqwest::Url::parse("https://example.test/sse").unwrap();
        assert!(read_legacy_sse_endpoint(&mut stream, &mut buffer, &source)
            .await
            .is_err());
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
