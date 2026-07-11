use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use lumo_agent::{
    discover_macos_configs, import_bytes, ConfigValue, McpConfigSource, McpTransportDraft,
};

const CLAUDE_SECRET: &str = "ghp_claude_fixture_secret";
const CODEX_SECRET: &str = "codex_fixture_secret";
const YAML_SECRET: &str = "Bearer yaml_fixture_secret";

fn fixture(name: &str) -> &'static [u8] {
    match name {
        "claude.json" => include_bytes!("fixtures/mcp/claude.json"),
        "cursor.jsonc" => include_bytes!("fixtures/mcp/cursor.jsonc"),
        "codex.toml" => include_bytes!("fixtures/mcp/codex.toml"),
        "servers.yaml" => include_bytes!("fixtures/mcp/servers.yaml"),
        _ => panic!("unknown fixture: {name}"),
    }
}

#[test]
fn imports_claude_json_stdio_and_extracts_secrets() {
    let batch = import_bytes("claude.json", fixture("claude.json")).unwrap();

    assert!(batch.warnings.is_empty());
    assert_eq!(batch.servers.len(), 1);
    let server = &batch.servers[0];
    assert_eq!(server.id, "github-mcp");
    assert_eq!(server.name, "GitHub MCP");
    assert!(server.enabled);
    assert_eq!(server.extensions["timeout"], 30);
    assert_eq!(batch.secrets.len(), 1);
    assert_eq!(batch.secrets[0].server_id, "github-mcp");
    assert_eq!(batch.secrets[0].field_path, "env.GITHUB_TOKEN");
    assert_eq!(
        batch.secrets[0].suggested_vault_key,
        "mcp.github-mcp.github-token"
    );
    assert_eq!(batch.secrets[0].value, CLAUDE_SECRET);

    let McpTransportDraft::Stdio { command, args, env } = &server.transport else {
        panic!("expected stdio transport");
    };
    assert_eq!(command, "npx");
    assert_eq!(args, &["-y", "@modelcontextprotocol/server-github"]);
    assert_eq!(env["LOG_LEVEL"], ConfigValue::Plain("info".into()));
    assert_eq!(
        env["GITHUB_TOKEN"],
        ConfigValue::VaultRef("mcp.github-mcp.github-token".into())
    );
}

#[test]
fn imports_cursor_jsonc_comments_and_trailing_commas() {
    let batch = import_bytes("cursor.jsonc", fixture("cursor.jsonc")).unwrap();

    assert!(batch.warnings.is_empty());
    assert_eq!(batch.servers.len(), 1);
    assert_eq!(batch.servers[0].id, "browser-tools");
    assert_eq!(
        batch.servers[0].transport,
        McpTransportDraft::Stdio {
            command: "node".into(),
            args: vec!["server.js".into()],
            env: BTreeMap::new(),
        }
    );
}

#[test]
fn imports_codex_toml_stdio_env_and_orders_servers_by_id() {
    let batch = import_bytes("config.toml", fixture("codex.toml")).unwrap();

    assert!(batch.warnings.is_empty());
    assert_eq!(
        batch
            .servers
            .iter()
            .map(|server| server.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "zeta"]
    );
    assert!(!batch.servers[1].enabled);
    let McpTransportDraft::Stdio { env, .. } = &batch.servers[1].transport else {
        panic!("expected stdio transport");
    };
    assert_eq!(env["PORT"], ConfigValue::Plain("4312".into()));
    assert_eq!(
        env["API_KEY"],
        ConfigValue::VaultRef("mcp.zeta.api-key".into())
    );
    assert_eq!(batch.secrets[0].value, CODEX_SECRET);
}

#[test]
fn imports_yaml_streamable_http_and_legacy_sse() {
    let batch = import_bytes("servers.yaml", fixture("servers.yaml")).unwrap();

    assert!(batch.warnings.is_empty());
    assert_eq!(batch.servers.len(), 2);
    assert!(matches!(
        batch.servers[0].transport,
        McpTransportDraft::Sse { .. }
    ));
    let McpTransportDraft::StreamableHttp { url, headers } = &batch.servers[1].transport else {
        panic!("expected streamable HTTP transport");
    };
    assert_eq!(url, "https://mcp.example.test/v1");
    assert_eq!(
        headers["Authorization"],
        ConfigValue::VaultRef("mcp.modern-http.authorization".into())
    );
    assert_eq!(headers["X-Retry-Count"], ConfigValue::Plain("3".into()));
    assert_eq!(batch.secrets[0].value, YAML_SECRET);
}

#[test]
fn accepts_wrapper_and_single_server_objects_with_stable_names() {
    let wrapper = import_bytes(
        "wrapper.json",
        br#"{"mcpServers":{"Zulu":{"command":"z"},"Alpha":{"command":"a"}}}"#,
    )
    .unwrap();
    assert_eq!(
        wrapper
            .servers
            .iter()
            .map(|server| (&*server.id, &*server.name))
            .collect::<Vec<_>>(),
        [("alpha", "Alpha"), ("zulu", "Zulu")]
    );

    let named = import_bytes(
        "ignored.json",
        br#"{"name":"Named Server","command":"named"}"#,
    )
    .unwrap();
    assert_eq!(named.servers[0].id, "named-server");
    assert_eq!(named.servers[0].name, "Named Server");

    let fallback = import_bytes("Fallback Config.json", br#"{"command":"fallback"}"#).unwrap();
    assert_eq!(fallback.servers[0].id, "fallback-config");
    assert_eq!(fallback.servers[0].name, "Fallback Config");
}

#[test]
fn redacted_and_serialized_drafts_never_contain_raw_secrets() {
    for (source, bytes, secret) in [
        ("claude.json", fixture("claude.json"), CLAUDE_SECRET),
        ("config.toml", fixture("codex.toml"), CODEX_SECRET),
        ("servers.yaml", fixture("servers.yaml"), YAML_SECRET),
    ] {
        let batch = import_bytes(source, bytes).unwrap();
        let batch_debug = format!("{batch:?}");
        assert!(!batch_debug.contains(secret));
        for server in &batch.servers {
            let redacted = server.redacted_json().to_string();
            let serialized = serde_json::to_string(server).unwrap();
            assert!(!redacted.contains(secret));
            assert!(!serialized.contains(secret));
            assert!(!serde_json::to_string(&server.extensions)
                .unwrap()
                .contains(secret));
        }
    }
}

#[test]
fn normalizes_string_like_scalars_and_rejects_complex_values() {
    let valid = import_bytes(
        "scalars.yaml",
        br#"command: tool
env:
  PORT: 42
  ENABLED: true
  EMPTY: null
"#,
    )
    .unwrap();
    let McpTransportDraft::Stdio { env, .. } = &valid.servers[0].transport else {
        panic!("expected stdio transport");
    };
    assert_eq!(env["PORT"], ConfigValue::Plain("42".into()));
    assert_eq!(env["ENABLED"], ConfigValue::Plain("true".into()));
    assert_eq!(env["EMPTY"], ConfigValue::Plain("null".into()));

    for invalid in [
        br#"{"command":"tool","env":{"BAD":["array"]}}"#.as_slice(),
        br#"{"url":"https://example.test","headers":{"BAD":{"nested":true}}}"#.as_slice(),
    ] {
        let batch = import_bytes("invalid.json", invalid).unwrap();
        assert!(batch.servers.is_empty());
        assert_eq!(batch.warnings.len(), 1);
        assert!(batch.warnings[0].message.contains("scalar"));
    }
}

#[test]
fn ambiguous_and_missing_transports_are_explicitly_skipped() {
    let batch = import_bytes(
        "invalid.json",
        br#"{"mcpServers":{"ambiguous":{"command":"x","url":"https://example.test"},"missing":{"args":[]},"valid":{"command":"ok"}}}"#,
    )
    .unwrap();

    assert_eq!(batch.servers.len(), 1);
    assert_eq!(batch.servers[0].id, "valid");
    assert_eq!(batch.warnings.len(), 2);
    assert_eq!(batch.warnings[0].server_id, "ambiguous");
    assert!(batch.warnings[0].message.contains("both command and url"));
    assert_eq!(batch.warnings[1].server_id, "missing");
    assert!(batch.warnings[1].message.contains("command or url"));
}

#[test]
fn malformed_input_error_never_echoes_file_contents() {
    let secret = "malformed_do_not_echo_secret";
    let bytes = format!(r#"{{"mcpServers": {{"x": {{"command": "{secret}"}}"#);
    let error = import_bytes("broken.json", bytes.as_bytes()).unwrap_err();
    let rendered = format!("{error:?} {error}");

    assert!(rendered.contains("broken.json"));
    assert!(!rendered.contains(secret));
    assert!(!rendered.contains("mcpServers"));
}

#[test]
fn secret_detection_is_case_insensitive_and_sanitizes_vault_keys() {
    let batch = import_bytes(
        "secret.json",
        br#"{"name":"My Server","url":"https://example.test","httpHeaders":{"x-Api-Key":"value-one","Cookie":"value-two"}}"#,
    )
    .unwrap();

    assert_eq!(batch.secrets.len(), 2);
    assert_eq!(batch.secrets[0].field_path, "headers.Cookie");
    assert_eq!(batch.secrets[0].suggested_vault_key, "mcp.my-server.cookie");
    assert_eq!(batch.secrets[1].field_path, "headers.x-Api-Key");
    assert_eq!(
        batch.secrets[1].suggested_vault_key,
        "mcp.my-server.x-api-key"
    );
}

#[test]
fn discovery_returns_only_existing_readable_macos_configs_in_fixed_order() {
    let home = unique_temp_home();
    let claude = home.join("Library/Application Support/Claude/claude_desktop_config.json");
    let cursor = home.join(".cursor/mcp.json");
    let codex = home.join(".codex/config.toml");
    fs::create_dir_all(claude.parent().unwrap()).unwrap();
    fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    fs::write(&claude, b"not parsed during discovery").unwrap();
    fs::write(&codex, b"also not parsed").unwrap();

    let discovered = discover_macos_configs(&home);
    assert_eq!(discovered.len(), 2);
    assert_eq!(discovered[0].source, McpConfigSource::ClaudeDesktop);
    assert_eq!(discovered[0].source_name, "Claude Desktop");
    assert_eq!(discovered[0].path, claude);
    assert_eq!(discovered[1].source, McpConfigSource::Codex);
    assert_eq!(discovered[1].source_name, "Codex");
    assert_eq!(discovered[1].path, codex);

    fs::remove_dir_all(home).unwrap();
}

fn unique_temp_home() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "lumo-agent-mcp-import-{}-{nonce}",
        std::process::id()
    ))
}
