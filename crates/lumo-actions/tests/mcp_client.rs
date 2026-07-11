#[cfg(unix)]
use lumo_actions::mcp::McpTool;
use lumo_actions::mcp::{McpClient, McpError, McpTransportConfig};
use serde_json::{json, Value};
use std::collections::BTreeMap;
#[cfg(unix)]
use std::process::{Command, Stdio};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

#[cfg(unix)]
const STDIO_FIXTURE: &str = r#"
mode="${MCP_FIXTURE_MODE:-roundtrip}"

if [ -n "${MCP_PID_FILE:-}" ]; then
  printf '%s\n' "$$" > "$MCP_PID_FILE"
fi

if [ "$mode" = "timeout" ]; then
  exec sleep 30
fi

while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')

  case "$line" in
    *'"method":"initialize"'*)
      if [ "$mode" = "malformed" ]; then
        printf 'this is not json\n'
        exit 0
      fi
      if [ "$mode" = "unrelated" ]; then
        printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":1}}'
        printf '%s\n' '{"jsonrpc":"2.0","id":999,"result":{"ignored":true}}'
      fi
      case "$mode" in
        init_null)
          printf '{"jsonrpc":"2.0","id":%s,"result":null}\n' "$id"
          ;;
        init_version_mismatch)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-01-01","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$id"
          ;;
        init_missing_capabilities)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"fixture","version":"1"}}}\n' "$id"
          ;;
        init_bad_server_info)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"fixture","version":1}}}\n' "$id"
          ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}}\n' "$id"
          ;;
      esac
      ;;
    *'"method":"notifications/initialized"'*)
      ;;
    *'"method":"tools/list"'*)
      if [ "$mode" = "unrelated" ]; then
        printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/tools/list_changed"}'
        printf '%s\n' '{"jsonrpc":"2.0","id":999,"result":{"tools":[]}}'
      fi
      case "$mode" in
        input_schema_non_object)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":[]}]}}\n' "$id"
          ;;
        output_schema_non_object)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"},"outputSchema":"bad"}]}}\n' "$id"
          ;;
        missing_input_schema)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo"}]}}\n' "$id"
          ;;
        *)
          printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"echo","description":"Echo input","inputSchema":{"type":"object"},"outputSchema":{"type":"object"}}]}}\n' "$id"
          ;;
      esac
      ;;
    *'"method":"tools/call"'*)
      if [ -n "${MCP_CALL_LOG:-}" ]; then
        printf '%s\n' "$line" >> "$MCP_CALL_LOG"
      fi
      if [ "$mode" = "server_error" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32001,"message":"fixture boom","data":{"kind":"fixture"}}}\n' "$id"
      else
        case "$line" in
          *'"arguments":{}'*) normalized=true ;;
          *) normalized=false ;;
        esac
        if [ "$mode" = "unrelated" ]; then
          printf '%s\n' '{"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":2}}'
          printf '%s\n' '{"jsonrpc":"2.0","id":999,"result":{"ignored":true}}'
        fi
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"echoed"}],"normalized":%s}}\n' "$id" "$normalized"
      fi
      ;;
  esac
done
"#;

#[cfg(unix)]
fn stdio_config(
    mode: &str,
    extra_env: impl IntoIterator<Item = (String, String)>,
) -> McpTransportConfig {
    let mut env = BTreeMap::from([("MCP_FIXTURE_MODE".to_string(), mode.to_string())]);
    env.extend(extra_env);
    McpTransportConfig::Stdio {
        command: "/bin/sh".to_string(),
        args: vec!["-c".to_string(), STDIO_FIXTURE.to_string()],
        env,
    }
}

#[cfg(unix)]
async fn connect_stdio(mode: &str) -> McpClient {
    McpClient::connect(
        stdio_config(mode, std::iter::empty()),
        Duration::from_secs(2),
    )
    .await
    .expect("stdio fixture should initialize")
}

#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
async fn wait_for_process_exit(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("fixture process {pid} is still alive");
}

#[cfg(unix)]
async fn wait_for_runtime_reap(pid: u32) {
    unsafe extern "C" {
        fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
    }

    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let mut status = 0;
        // WNOHANG is 1 on the Unix targets supported by this crate.
        let result = unsafe { waitpid(pid as i32, &mut status, 1) };
        if result == -1 {
            return;
        }
        if result == pid as i32 {
            panic!("fixture process {pid} was left for the caller to reap");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("fixture process {pid} was not reaped by the client drop task");
}

#[cfg(unix)]
fn read_pid(path: &std::path::Path) -> u32 {
    std::fs::read_to_string(path)
        .expect("fixture should write its pid")
        .trim()
        .parse()
        .expect("fixture pid should be numeric")
}

#[cfg(unix)]
#[tokio::test]
async fn reusable_stdio_client_lists_and_calls_echo_tool() {
    let mut client = connect_stdio("roundtrip").await;

    let tools = client.list_tools().await.expect("list tools");
    assert_eq!(
        tools,
        vec![McpTool {
            name: "echo".to_string(),
            description: "Echo input".to_string(),
            input_schema: json!({"type": "object"}),
            output_schema: Some(json!({"type": "object"})),
        }]
    );

    let result = client
        .call_tool("echo", json!({"message": "hello"}))
        .await
        .expect("call echo");
    assert_eq!(result["content"][0]["text"], json!("echoed"));

    client.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_ignores_notifications_and_unrelated_response_ids() {
    let mut client = connect_stdio("unrelated").await;

    let tools = client.list_tools().await.expect("list tools");
    assert_eq!(tools[0].name, "echo");
    let result = client
        .call_tool("echo", json!({}))
        .await
        .expect("call echo");
    assert_eq!(result["content"][0]["text"], json!("echoed"));

    client.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn list_tools_rejects_non_object_input_schema() {
    let mut client = connect_stdio("input_schema_non_object").await;

    let err = client
        .list_tools()
        .await
        .expect_err("array inputSchema must be rejected");
    assert!(matches!(err, McpError::Protocol { .. }), "got: {err:?}");
    assert!(err.to_string().contains("echo"));
    assert!(err.to_string().contains("inputSchema"));
}

#[cfg(unix)]
#[tokio::test]
async fn list_tools_rejects_non_object_output_schema() {
    let mut client = connect_stdio("output_schema_non_object").await;

    let err = client
        .list_tools()
        .await
        .expect_err("string outputSchema must be rejected");
    assert!(matches!(err, McpError::Protocol { .. }), "got: {err:?}");
    assert!(err.to_string().contains("echo"));
    assert!(err.to_string().contains("outputSchema"));
}

#[cfg(unix)]
#[tokio::test]
async fn list_tools_defaults_missing_input_schema_to_object() {
    let mut client = connect_stdio("missing_input_schema").await;

    let tools = client.list_tools().await.expect("missing schema defaults");
    assert_eq!(tools[0].input_schema, json!({"type": "object"}));
    assert_eq!(tools[0].output_schema, None);

    client.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn malformed_stdio_json_is_a_protocol_error() {
    let err = match McpClient::connect(
        stdio_config("malformed", std::iter::empty()),
        Duration::from_secs(2),
    )
    .await
    {
        Ok(mut client) => {
            client.close().await;
            panic!("malformed response unexpectedly initialized")
        }
        Err(err) => err,
    };

    assert!(matches!(err, McpError::Protocol { .. }), "got: {err:?}");
}

#[cfg(unix)]
#[tokio::test]
async fn json_rpc_server_error_is_typed() {
    let mut client = connect_stdio("server_error").await;

    let err = client
        .call_tool("echo", json!({}))
        .await
        .expect_err("fixture should return a JSON-RPC error");
    match err {
        McpError::Server {
            code,
            message,
            data,
        } => {
            assert_eq!(code, -32001);
            assert_eq!(message, "fixture boom");
            assert_eq!(data, Some(json!({"kind": "fixture"})));
        }
        other => panic!("expected server error, got {other:?}"),
    }

    client.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_unblocks_and_kills_unresponsive_stdio_server() {
    let temp = tempfile::TempDir::new().unwrap();
    let pid_file = temp.path().join("pid");
    let started = Instant::now();

    let err = match McpClient::connect(
        stdio_config(
            "timeout",
            [("MCP_PID_FILE".to_string(), pid_file.display().to_string())],
        ),
        Duration::from_millis(250),
    )
    .await
    {
        Ok(mut client) => {
            client.close().await;
            panic!("unresponsive fixture unexpectedly initialized")
        }
        Err(err) => err,
    };

    assert!(matches!(err, McpError::Timeout { .. }), "got: {err:?}");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "timeout did not unblock promptly"
    );
    let pid = read_pid(&pid_file);
    wait_for_runtime_reap(pid).await;
}

#[cfg(unix)]
#[tokio::test]
async fn call_tool_normalizes_null_and_rejects_array_before_request() {
    let temp = tempfile::TempDir::new().unwrap();
    let call_log = temp.path().join("calls.jsonl");
    let mut client = McpClient::connect(
        stdio_config(
            "roundtrip",
            [("MCP_CALL_LOG".to_string(), call_log.display().to_string())],
        ),
        Duration::from_secs(2),
    )
    .await
    .expect("stdio fixture should initialize");

    let err = client
        .call_tool("echo", json!([1, 2, 3]))
        .await
        .expect_err("array arguments must be rejected");
    assert!(matches!(err, McpError::InvalidArguments));
    assert!(!call_log.exists(), "invalid arguments reached the server");

    let result = client
        .call_tool("echo", Value::Null)
        .await
        .expect("null arguments should normalize");
    assert_eq!(result["normalized"], json!(true));

    let calls = std::fs::read_to_string(&call_log).unwrap();
    let request: Value = serde_json::from_str(calls.trim()).unwrap();
    assert_eq!(
        request["id"],
        json!(2),
        "rejected calls must not consume IDs"
    );
    assert_eq!(request["params"]["arguments"], json!({}));

    client.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn initialize_response_is_strictly_validated() {
    for (mode, expected) in [
        ("init_null", "initialize result"),
        ("init_version_mismatch", "protocolVersion"),
        ("init_missing_capabilities", "capabilities"),
        ("init_bad_server_info", "serverInfo.version"),
    ] {
        let err = match McpClient::connect(
            stdio_config(mode, std::iter::empty()),
            Duration::from_secs(2),
        )
        .await
        {
            Ok(mut client) => {
                client.close().await;
                panic!("invalid initialize response unexpectedly accepted for {mode}")
            }
            Err(err) => err,
        };
        assert!(matches!(err, McpError::Protocol { .. }), "got: {err:?}");
        let rendered = err.to_string();
        assert!(rendered.contains("initialize"), "got: {rendered}");
        assert!(rendered.contains(expected), "got: {rendered}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_client_kills_and_reaps_stdio_child() {
    let temp = tempfile::TempDir::new().unwrap();
    let pid_file = temp.path().join("pid");
    let client = McpClient::connect(
        stdio_config(
            "roundtrip",
            [("MCP_PID_FILE".to_string(), pid_file.display().to_string())],
        ),
        Duration::from_secs(2),
    )
    .await
    .expect("stdio fixture should initialize");
    let pid = read_pid(&pid_file);
    assert!(process_is_alive(pid));

    drop(client);

    wait_for_process_exit(pid).await;
}

fn json_rpc_http_fixture(request: &Request) -> ResponseTemplate {
    let body: Value = request.body_json().expect("valid JSON-RPC request");
    let id = body.get("id").cloned().unwrap_or(Value::Null);
    match body.get("method").and_then(Value::as_str) {
        Some("initialize") => ResponseTemplate::new(200)
            .insert_header("Mcp-Session-Id", "session-123")
            .set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "serverInfo": {"name": "http-fixture", "version": "1"}
                }
            })),
        Some("notifications/initialized") => ResponseTemplate::new(204),
        Some("tools/list") => ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "tools": [{
                    "name": "echo",
                    "description": "HTTP echo",
                    "inputSchema": {"type": "object"}
                }]
            }
        })),
        Some("tools/call") => ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{
                    "type": "text",
                    "text": body["params"]["arguments"]["message"]
                }]
            }
        })),
        other => panic!("unexpected method: {other:?}"),
    }
}

#[tokio::test]
async fn streamable_http_uses_session_and_reuses_client_for_list_and_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(json_rpc_http_fixture)
        .expect(4)
        .mount(&server)
        .await;
    let headers = BTreeMap::from([
        ("X-Api-Key".to_string(), "caller-key".to_string()),
        ("mCp-SeSsIoN-iD".to_string(), "stale-session".to_string()),
    ]);
    let mut client = McpClient::connect(
        McpTransportConfig::StreamableHttp {
            url: format!("{}/mcp", server.uri()),
            headers,
        },
        Duration::from_secs(2),
    )
    .await
    .expect("HTTP initialize");

    let tools = client.list_tools().await.expect("HTTP list");
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "HTTP echo");
    assert_eq!(tools[0].input_schema, json!({"type": "object"}));
    assert_eq!(tools[0].output_schema, None);

    let result = client
        .call_tool("echo", json!({"message": "over-http"}))
        .await
        .expect("HTTP call");
    assert_eq!(result["content"][0]["text"], json!("over-http"));

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 4);
    let bodies: Vec<Value> = requests
        .iter()
        .map(|request| request.body_json().unwrap())
        .collect();
    assert_eq!(bodies[0]["id"], json!(1));
    assert_eq!(bodies[0]["params"]["protocolVersion"], json!("2024-11-05"));
    assert_eq!(bodies[0]["params"]["clientInfo"]["name"], json!("lumo-rpa"));
    assert_eq!(bodies[1]["method"], json!("notifications/initialized"));
    assert!(bodies[1].get("id").is_none());
    assert_eq!(bodies[2]["id"], json!(2));
    assert_eq!(bodies[3]["id"], json!(3));
    for request in &requests {
        assert_eq!(
            request.headers["accept"],
            "application/json, text/event-stream"
        );
        assert_eq!(request.headers["content-type"], "application/json");
        assert_eq!(request.headers["x-api-key"], "caller-key");
    }
    assert!(requests[0].headers.get("mcp-session-id").is_none());
    for request in requests.iter().skip(1) {
        assert_eq!(request.headers["mcp-session-id"], "session-123");
    }

    client.close().await;
}

fn sse_http_fixture(request: &Request) -> ResponseTemplate {
    let body: Value = request.body_json().expect("valid JSON-RPC request");
    let id = body.get("id").and_then(Value::as_u64).unwrap_or_default();
    match body.get("method").and_then(Value::as_str) {
        Some("initialize") => ResponseTemplate::new(200).set_body_json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "serverInfo": {"name": "sse-fixture", "version": "1"}
            }
        })),
        Some("notifications/initialized") => ResponseTemplate::new(204),
        Some("tools/list") => ResponseTemplate::new(200).set_body_raw(
            format!(
                ": keepalive\nunknown: ignored\ndata: not-json\n \t\n\
                 event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":999,\n\
                 data: \"result\":{{\"tools\":[]}}}}\n\n\
                 data: {{\"jsonrpc\":\"2.0\",\"id\":{id},\n\
                 data: \"result\":{{\"tools\":[{{\"name\":\"sse_echo\",\"inputSchema\":{{\"type\":\"object\"}}}}]}}}}\n \t\n"
            ),
            "text/event-stream",
        ),
        other => panic!("unexpected method: {other:?}"),
    }
}

#[tokio::test]
async fn streamable_http_parses_sse_and_selects_matching_response_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(sse_http_fixture)
        .expect(3)
        .mount(&server)
        .await;
    let mut client = McpClient::connect(
        McpTransportConfig::StreamableHttp {
            url: format!("{}/mcp", server.uri()),
            headers: BTreeMap::new(),
        },
        Duration::from_secs(2),
    )
    .await
    .expect("HTTP initialize");

    let tools = client.list_tools().await.expect("SSE tools/list");
    assert_eq!(tools[0].name, "sse_echo");
    assert_eq!(tools[0].description, "");

    client.close().await;
}

#[tokio::test]
async fn non_success_http_status_does_not_leak_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .expect(1)
        .mount(&server)
        .await;
    let secret = "Bearer do-not-leak-this";
    let err = match McpClient::connect(
        McpTransportConfig::StreamableHttp {
            url: format!("{}/mcp", server.uri()),
            headers: BTreeMap::from([("Authorization".to_string(), secret.to_string())]),
        },
        Duration::from_secs(2),
    )
    .await
    {
        Ok(mut client) => {
            client.close().await;
            panic!("401 response unexpectedly initialized")
        }
        Err(err) => err,
    };

    assert!(matches!(err, McpError::Transport { .. }), "got: {err:?}");
    let rendered = err.to_string();
    assert!(rendered.contains("401"), "got: {rendered}");
    assert!(!rendered.contains(secret), "secret leaked: {rendered}");
}

#[tokio::test]
async fn streamable_http_rejects_url_credentials_without_leaking_them() {
    let username = "credential-user";
    let password = "credential-password";
    let token = "path-token-secret";
    let err = match McpClient::connect(
        McpTransportConfig::StreamableHttp {
            url: format!("http://{username}:{password}@127.0.0.1:9/{token}"),
            headers: BTreeMap::new(),
        },
        Duration::from_millis(250),
    )
    .await
    {
        Ok(mut client) => {
            client.close().await;
            panic!("URL credentials unexpectedly accepted")
        }
        Err(err) => err,
    };

    assert!(
        matches!(err, McpError::Protocol { .. } | McpError::Transport { .. }),
        "got: {err:?}"
    );
    let rendered = err.to_string();
    assert!(!rendered.contains(username), "username leaked: {rendered}");
    assert!(!rendered.contains(password), "password leaked: {rendered}");
    assert!(!rendered.contains(token), "URL path leaked: {rendered}");
}

#[cfg(unix)]
#[tokio::test]
async fn close_is_idempotent_and_stops_stdio_child() {
    let temp = tempfile::TempDir::new().unwrap();
    let pid_file = temp.path().join("pid");
    let mut client = McpClient::connect(
        stdio_config(
            "roundtrip",
            [("MCP_PID_FILE".to_string(), pid_file.display().to_string())],
        ),
        Duration::from_secs(2),
    )
    .await
    .expect("stdio fixture should initialize");
    let pid = read_pid(&pid_file);
    assert!(process_is_alive(pid));

    client.close().await;
    client.close().await;

    wait_for_process_exit(pid).await;
}
