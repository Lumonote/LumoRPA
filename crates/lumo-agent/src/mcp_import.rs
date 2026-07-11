use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::path::Path;

use serde_json::{Map, Value};
use thiserror::Error;

use crate::mcp_profile::{
    ConfigValue, DiscoveredConfig, ImportWarning, McpConfigSource, McpImportBatch, McpServerDraft,
    McpTransportDraft, SecretCandidate,
};

const RECOGNIZED_KEYS: &[&str] = &[
    "name",
    "command",
    "args",
    "env",
    "url",
    "headers",
    "http_headers",
    "httpHeaders",
    "type",
    "transport",
    "enabled",
];

const SECRET_MARKERS: &[&str] = &[
    "TOKEN",
    "KEY",
    "SECRET",
    "PASSWORD",
    "AUTHORIZATION",
    "COOKIE",
];

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ImportError {
    #[error("failed to parse MCP configuration {source_name} as {format}")]
    Malformed {
        source_name: String,
        format: &'static str,
    },
    #[error(
        "MCP configuration {source_name} must contain an mcpServers wrapper or a server object"
    )]
    UnsupportedShape { source_name: String },
}

pub fn import_bytes(source_name: &str, bytes: &[u8]) -> Result<McpImportBatch, ImportError> {
    let root = parse_document(source_name, bytes)?;
    let entries = server_entries(source_name, root)?;
    let mut batch = McpImportBatch::default();

    for entry in entries {
        match normalize_server(&entry.name, entry.value) {
            Ok((server, mut secrets)) => {
                batch.servers.push(server);
                batch.secrets.append(&mut secrets);
            }
            Err(message) => batch.warnings.push(ImportWarning {
                server_id: entry.id,
                message,
            }),
        }
    }

    batch.servers.sort_by(|left, right| left.id.cmp(&right.id));
    batch.secrets.sort_by(|left, right| {
        (&left.server_id, &left.field_path).cmp(&(&right.server_id, &right.field_path))
    });
    batch
        .warnings
        .sort_by(|left, right| left.server_id.cmp(&right.server_id));
    Ok(batch)
}

pub fn discover_macos_configs(home: &Path) -> Vec<DiscoveredConfig> {
    [
        (
            McpConfigSource::ClaudeDesktop,
            "Claude Desktop",
            "Library/Application Support/Claude/claude_desktop_config.json",
        ),
        (McpConfigSource::Cursor, "Cursor", ".cursor/mcp.json"),
        (McpConfigSource::Codex, "Codex", ".codex/config.toml"),
    ]
    .into_iter()
    .filter_map(|(source, source_name, relative)| {
        let path = home.join(relative);
        if path.is_file() && File::open(&path).is_ok() {
            Some(DiscoveredConfig {
                source,
                source_name,
                path,
            })
        } else {
            None
        }
    })
    .collect()
}

fn parse_document(source_name: &str, bytes: &[u8]) -> Result<Value, ImportError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ImportError::Malformed {
        source_name: source_name.to_owned(),
        format: format_name(source_name),
    })?;
    let extension = Path::new(source_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    match extension.as_str() {
        "json" | "jsonc" => parse_json5(source_name, text),
        "toml" => parse_toml(source_name, text),
        "yaml" | "yml" => parse_yaml(source_name, bytes),
        _ => parse_json5(source_name, text)
            .or_else(|_| parse_toml(source_name, text))
            .or_else(|_| parse_yaml(source_name, bytes)),
    }
}

fn parse_json5(source_name: &str, text: &str) -> Result<Value, ImportError> {
    json5::from_str(text).map_err(|_| ImportError::Malformed {
        source_name: source_name.to_owned(),
        format: "JSON/JSONC",
    })
}

fn parse_toml(source_name: &str, text: &str) -> Result<Value, ImportError> {
    let value = toml::from_str::<toml::Value>(text).map_err(|_| ImportError::Malformed {
        source_name: source_name.to_owned(),
        format: "TOML",
    })?;
    serde_json::to_value(value).map_err(|_| ImportError::Malformed {
        source_name: source_name.to_owned(),
        format: "TOML",
    })
}

fn parse_yaml(source_name: &str, bytes: &[u8]) -> Result<Value, ImportError> {
    serde_yaml::from_slice(bytes).map_err(|_| ImportError::Malformed {
        source_name: source_name.to_owned(),
        format: "YAML",
    })
}

fn format_name(source_name: &str) -> &'static str {
    match Path::new(source_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "json" | "jsonc" => "JSON/JSONC",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        _ => "supported format",
    }
}

struct ServerEntry {
    id: String,
    name: String,
    value: Value,
}

fn server_entries(source_name: &str, root: Value) -> Result<Vec<ServerEntry>, ImportError> {
    let Value::Object(mut root) = root else {
        return Err(ImportError::UnsupportedShape {
            source_name: source_name.to_owned(),
        });
    };

    if let Some(wrapper) = root
        .remove("mcpServers")
        .or_else(|| root.remove("mcp_servers"))
    {
        let Value::Object(servers) = wrapper else {
            return Err(ImportError::UnsupportedShape {
                source_name: source_name.to_owned(),
            });
        };
        let mut entries = servers
            .into_iter()
            .map(|(name, value)| ServerEntry {
                id: sanitize_identifier(&name),
                name,
                value,
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.id.cmp(&right.id));
        return Ok(entries);
    }

    if root.contains_key("command") || root.contains_key("url") {
        let fallback = source_stem(source_name);
        let name = root
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&fallback)
            .to_owned();
        return Ok(vec![ServerEntry {
            id: sanitize_identifier(&name),
            name,
            value: Value::Object(root),
        }]);
    }

    Err(ImportError::UnsupportedShape {
        source_name: source_name.to_owned(),
    })
}

fn source_stem(source_name: &str) -> String {
    Path::new(source_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::trim)
        .filter(|stem| !stem.is_empty())
        .unwrap_or("MCP Server")
        .to_owned()
}

fn normalize_server(
    name: &str,
    value: Value,
) -> Result<(McpServerDraft, Vec<SecretCandidate>), String> {
    let id = sanitize_identifier(name);
    let Value::Object(fields) = value else {
        return Err("server configuration must be an object".into());
    };
    let command = optional_string(&fields, "command")?;
    let url = optional_string(&fields, "url")?;
    let command = command.filter(|value| !value.trim().is_empty());
    let url = url.filter(|value| !value.trim().is_empty());

    let enabled = match fields.get("enabled") {
        Some(Value::Bool(enabled)) => *enabled,
        Some(_) => return Err("enabled must be a boolean".into()),
        None => true,
    };

    let mut secrets = Vec::new();
    let transport = match (command, url) {
        (Some(_), Some(_)) => return Err("server defines both command and url".into()),
        (None, None) => return Err("server must define a non-empty command or url".into()),
        (Some(command), None) => {
            let args = parse_args(fields.get("args"))?;
            let env = parse_config_map(fields.get("env"), &id, "env", &mut secrets)?;
            McpTransportDraft::Stdio { command, args, env }
        }
        (None, Some(url)) => {
            let headers = parse_headers(&fields, &id, &mut secrets)?;
            if transport_name(&fields)?.is_some_and(|name| name.eq_ignore_ascii_case("sse")) {
                McpTransportDraft::Sse { url, headers }
            } else {
                McpTransportDraft::StreamableHttp { url, headers }
            }
        }
    };

    let recognized = RECOGNIZED_KEYS.iter().copied().collect::<BTreeSet<_>>();
    let extensions = fields
        .into_iter()
        .filter(|(key, _)| !recognized.contains(key.as_str()))
        .collect::<Map<_, _>>();

    Ok((
        McpServerDraft {
            id,
            name: name.to_owned(),
            transport,
            enabled,
            extensions,
        },
        secrets,
    ))
}

fn optional_string(fields: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match fields.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key} must be a string")),
        None => Ok(None),
    }
}

fn transport_name(fields: &Map<String, Value>) -> Result<Option<String>, String> {
    optional_string(fields, "type")?.map_or_else(
        || optional_string(fields, "transport"),
        |value| Ok(Some(value)),
    )
}

fn parse_args(value: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let Value::Array(args) = value else {
        return Err("args must be an array of strings".into());
    };
    args.iter()
        .map(|arg| match arg {
            Value::String(arg) => Ok(arg.clone()),
            _ => Err("args must contain only strings".into()),
        })
        .collect()
}

fn parse_headers(
    fields: &Map<String, Value>,
    server_id: &str,
    secrets: &mut Vec<SecretCandidate>,
) -> Result<BTreeMap<String, ConfigValue>, String> {
    let mut headers = BTreeMap::new();
    for key in ["headers", "http_headers", "httpHeaders"] {
        let values = parse_config_map(fields.get(key), server_id, "headers", secrets)?;
        headers.extend(values);
    }
    Ok(headers)
}

fn parse_config_map(
    value: Option<&Value>,
    server_id: &str,
    field_prefix: &str,
    secrets: &mut Vec<SecretCandidate>,
) -> Result<BTreeMap<String, ConfigValue>, String> {
    let Some(value) = value else {
        return Ok(BTreeMap::new());
    };
    let Value::Object(values) = value else {
        return Err(format!("{field_prefix} must be an object of scalar values"));
    };
    let mut normalized = BTreeMap::new();
    for (name, value) in values {
        let value = scalar_string(value)
            .ok_or_else(|| format!("{field_prefix}.{name} must be a scalar value"))?;
        if is_secret_name(name) {
            let suggested_vault_key = format!(
                "mcp.{}.{}",
                sanitize_identifier(server_id),
                sanitize_identifier(name)
            );
            secrets.push(SecretCandidate {
                server_id: server_id.to_owned(),
                field_path: format!("{field_prefix}.{name}"),
                suggested_vault_key: suggested_vault_key.clone(),
                value,
            });
            normalized.insert(name.clone(), ConfigValue::VaultRef(suggested_vault_key));
        } else {
            normalized.insert(name.clone(), ConfigValue::Plain(value));
        }
    }
    Ok(normalized)
}

fn scalar_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null => Some("null".into()),
        Value::Array(_) | Value::Object(_) => None,
    }
}

fn is_secret_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_MARKERS.iter().any(|marker| upper.contains(marker))
}

fn sanitize_identifier(value: &str) -> String {
    let mut sanitized = String::new();
    let mut needs_separator = false;
    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if needs_separator && !sanitized.is_empty() {
                sanitized.push('-');
            }
            sanitized.push(character);
            needs_separator = false;
        } else {
            needs_separator = true;
        }
    }
    if sanitized.is_empty() {
        "mcp-server".into()
    } else {
        sanitized
    }
}
