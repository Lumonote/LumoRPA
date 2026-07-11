use super::mcp_supervisor::{
    complete_oauth, refresh_oauth, McpOAuthStart, OAuthBrowser, OAuthTokenVault,
    ReqwestOAuthTransport,
};
use super::{app_home, open_repo, DesktopState};
use chrono::{DateTime, Utc};
use lumo_actions::mcp::oauth::{
    OAuthClientMetadata, StoredOAuthTokenRefs,
};
use lumo_actions::mcp::{McpClient, McpTool, McpTransportConfig};
use lumo_agent::{
    import_bytes, ConfigValue, ImportWarning, McpImportBatch, McpPublisherMetadata, McpRelease,
    McpServerDraft, McpToolDefinition, McpTransportDraft, SecretCandidate,
};
use lumo_storage::{McpServerRow, McpToolRow, Repo, Vault, VaultIdentity};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tauri::{Emitter, State, Wry};

type AppHandle = tauri::AppHandle<Wry>;

const MCP_OPERATION_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpServerDto {
    id: String,
    name: String,
    transport: String,
    config: Value,
    enabled: bool,
    health: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    tools: Vec<McpToolRow>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpImportPreviewDto {
    token: String,
    servers: Vec<Value>,
    secrets: Vec<SecretPreviewDto>,
    warnings: Vec<WarningDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretPreviewDto {
    server_id: String,
    field_path: String,
    suggested_vault_key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WarningDto {
    server_id: String,
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretOverrideDto {
    server_id: String,
    field_path: String,
    #[serde(default)]
    vault_key: Option<String>,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct McpTestResultDto {
    id: String,
    healthy: bool,
    tool_count: Option<usize>,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredMcpConfig {
    transport: McpTransportDraft,
    #[serde(default)]
    extensions: Map<String, Value>,
}

enum SecretChoice {
    Store { vault_ref: String, value: String },
    Existing { vault_ref: String },
}

struct TauriOAuthBrowser<'a> {
    app: &'a AppHandle,
}

impl OAuthBrowser for TauriOAuthBrowser<'_> {
    fn open(&self, url: &str) -> Result<(), String> {
        self.app
            .emit("lumo://mcp-oauth-open", serde_json::json!({ "url": url }))
            .map_err(|error| error.to_string())
    }
}

struct RepoOAuthVault<'a> {
    repo: &'a Repo,
    identity: &'a VaultIdentity,
}

impl OAuthTokenVault for RepoOAuthVault<'_> {
    fn put(&self, reference: &str, value: &str) -> Result<(), String> {
        let (namespace, key) = split_vault_ref(reference)?;
        let vault = Vault::new(self.repo, self.identity);
        let mut fields = vault
            .get(&namespace)
            .map_err(|error| error.to_string())?
            .unwrap_or_default();
        fields.insert(key, value.into());
        vault
            .put(&namespace, &fields)
            .map_err(|error| error.to_string())
    }

    fn get(&self, reference: &str) -> Result<String, String> {
        resolve_vault_ref(self.repo, Some(self.identity), reference)
    }
}

#[tauri::command]
pub(crate) fn list_mcp_servers(app: AppHandle) -> Result<Vec<McpServerDto>, String> {
    let repo = open_repo(&app)?;
    repo.list_mcp_servers()
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|row| server_dto(&repo, row))
        .collect()
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn mcp_oauth_start(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: String,
    metadata: Option<OAuthClientMetadata>,
) -> Result<McpOAuthStart, String> {
    let repo = open_repo(&app)?;
    let mut row = require_server(&repo, &id)?;
    let metadata = metadata
        .or_else(|| stored_config(&row).ok().and_then(|config| extension(&config, "oauthClient").ok()))
        .ok_or_else(|| format!("MCP server `{id}` has no OAuth client metadata"))?;
    let nonce = ulid::Ulid::new().to_string();
    let verifier = format!("{}{}", ulid::Ulid::new(), ulid::Ulid::new());
    let started = state
        .mcp
        .lock()
        .map_err(|_| "MCP supervisor state is unavailable".to_string())?
        .oauth
        .start(
            &id,
            metadata.clone(),
            format!("state-{nonce}"),
            verifier,
            &TauriOAuthBrowser { app: &app },
        )
        .map_err(|error| error.to_string())?;
    set_server_extension(
        &mut row,
        "oauthClient",
        serde_json::to_value(metadata).map_err(|error| error.to_string())?,
    )?;
    row.updated_at = Utc::now();
    repo.upsert_mcp_server(&row)
        .map_err(|error| error.to_string())?;
    Ok(started)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) async fn mcp_oauth_callback(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: String,
    returned_state: String,
    code: String,
) -> Result<StoredOAuthTokenRefs, String> {
    let repo = open_repo(&app)?;
    let mut row = require_server(&repo, &id)?;
    let session = state
        .mcp
        .lock()
        .map_err(|_| "MCP supervisor state is unavailable".to_string())?
        .oauth
        .pending(&id)
        .map_err(|error| error.to_string())?;
    let identity = ensure_vault_identity(&app_home(&app)?)?;
    let vault = RepoOAuthVault {
        repo: &repo,
        identity: &identity,
    };
    let transport = ReqwestOAuthTransport::new(session.metadata.clone());
    let stored = complete_oauth(
        &transport,
        &session,
        &returned_state,
        &code,
        &vault,
        &id,
        Utc::now(),
    )
    .await
    .map_err(|error| error.to_string())?;
    set_server_extension(
        &mut row,
        "oauth",
        serde_json::to_value(&stored).map_err(|error| error.to_string())?,
    )?;
    row.updated_at = Utc::now();
    repo.upsert_mcp_server(&row)
        .map_err(|error| error.to_string())?;
    state
        .mcp
        .lock()
        .map_err(|_| "MCP supervisor state is unavailable".to_string())?
        .oauth
        .complete(&id, &session.state)
        .map_err(|error| error.to_string())?;
    Ok(stored)
}

#[tauri::command]
pub(crate) async fn mcp_oauth_refresh(
    app: AppHandle,
    id: String,
) -> Result<StoredOAuthTokenRefs, String> {
    let repo = open_repo(&app)?;
    let mut row = require_server(&repo, &id)?;
    let stored_config = stored_config(&row)?;
    let metadata: OAuthClientMetadata = extension(&stored_config, "oauthClient")?;
    let stored: StoredOAuthTokenRefs = extension(&stored_config, "oauth")?;
    let identity = ensure_vault_identity(&app_home(&app)?)?;
    let vault = RepoOAuthVault {
        repo: &repo,
        identity: &identity,
    };
    let transport = ReqwestOAuthTransport::new(metadata);
    let refreshed = refresh_oauth(&transport, &stored, &vault, &id, Utc::now())
        .await
        .map_err(|error| error.to_string())?;
    set_server_extension(
        &mut row,
        "oauth",
        serde_json::to_value(&refreshed).map_err(|error| error.to_string())?,
    )?;
    row.updated_at = Utc::now();
    repo.upsert_mcp_server(&row)
        .map_err(|error| error.to_string())?;
    Ok(refreshed)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn approve_mcp_schema_change(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: String,
    tool: String,
    release_version: Option<String>,
    schema_hash: String,
) -> Result<McpToolRow, String> {
    let approved = {
        let mut supervisor = state.mcp.lock().map_err(|_| "MCP supervisor state is unavailable".to_string())?;
        let release_version = release_version.or_else(|| supervisor.schemas.registry().pending_tool(&id, &tool).map(|pending| pending.release_version)).ok_or_else(|| format!("no pending schema change for `{id}:{tool}`"))?;
        supervisor.schemas.approve(&id, &tool, &release_version, &schema_hash).map_err(|error| error.to_string())?
    };
    let repo = open_repo(&app)?;
    require_server(&repo, &id)?;
    let now = Utc::now();
    let mut tools = repo
        .list_mcp_tools(&id)
        .map_err(|error| error.to_string())?;
    let row = McpToolRow {
        server_id: id.clone(),
        name: approved.name.clone(),
        description: approved.description,
        input_schema: approved.input_schema,
        output_schema: tools
            .iter()
            .find(|candidate| candidate.name == approved.name)
            .and_then(|candidate| candidate.output_schema.clone()),
        risk: tools
            .iter()
            .find(|candidate| candidate.name == approved.name)
            .map_or_else(|| "L1".into(), |candidate| candidate.risk.clone()),
        enabled: true,
        version_hash: approved.schema_hash,
        discovered_at: now,
    };
    if let Some(existing) = tools
        .iter_mut()
        .find(|candidate| candidate.name == approved.name)
    {
        *existing = row.clone();
    } else {
        tools.push(row.clone());
    }
    repo.replace_mcp_tools(&id, &tools)
        .map_err(|error| error.to_string())?;
    Ok(row)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn preview_mcp_import(
    state: State<'_, DesktopState>,
    source_name: String,
    content: String,
) -> Result<McpImportPreviewDto, String> {
    let batch =
        import_bytes(&source_name, content.as_bytes()).map_err(|error| error.to_string())?;
    let token = ulid::Ulid::new().to_string();
    let preview = preview_from_batch(token.clone(), &batch);
    state
        .pending_mcp_imports
        .lock()
        .map_err(|_| "pending MCP import state is unavailable".to_string())?
        .insert(token, batch);
    Ok(preview)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn apply_mcp_import(
    app: AppHandle,
    state: State<'_, DesktopState>,
    token: String,
    selected_ids: Vec<String>,
    secret_overrides: Vec<SecretOverrideDto>,
) -> Result<Vec<McpServerDto>, String> {
    let batch = state
        .pending_mcp_imports
        .lock()
        .map_err(|_| "pending MCP import state is unavailable".to_string())?
        .remove(&token)
        .ok_or_else(|| "MCP import token is missing or expired".to_string())?;
    let selected = selected_ids.into_iter().collect::<HashSet<_>>();
    let available = batch
        .servers
        .iter()
        .map(|server| server.id.as_str())
        .collect::<HashSet<_>>();
    if let Some(id) = selected.iter().find(|id| !available.contains(id.as_str())) {
        return Err(format!("MCP import server `{id}` not found"));
    }

    let overrides = secret_overrides
        .into_iter()
        .map(|item| ((item.server_id.clone(), item.field_path.clone()), item))
        .collect::<HashMap<_, _>>();
    let mut drafts = batch
        .servers
        .into_iter()
        .filter(|server| selected.contains(&server.id))
        .map(|server| (server.id.clone(), server))
        .collect::<HashMap<_, _>>();
    let mut choices = Vec::new();
    for candidate in batch
        .secrets
        .iter()
        .filter(|secret| selected.contains(&secret.server_id))
    {
        let choice = resolve_secret_choice(
            candidate,
            overrides.get(&(candidate.server_id.clone(), candidate.field_path.clone())),
        )?;
        let vault_ref = match &choice {
            SecretChoice::Store { vault_ref, .. } | SecretChoice::Existing { vault_ref } => {
                vault_ref
            }
        };
        let draft = drafts
            .get_mut(&candidate.server_id)
            .ok_or_else(|| format!("MCP import server `{}` not found", candidate.server_id))?;
        set_vault_ref(&mut draft.transport, &candidate.field_path, vault_ref)?;
        choices.push(choice);
    }

    let home = app_home(&app)?;
    let repo = open_repo(&app)?;
    persist_secret_choices(&repo, &home, choices)?;
    let now = Utc::now();
    let mut rows = Vec::with_capacity(drafts.len());
    for draft in drafts.into_values() {
        let row = server_row_from_draft(draft, now)?;
        repo.upsert_mcp_server(&row)
            .map_err(|error| error.to_string())?;
        rows.push(row);
    }
    rows.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    rows.into_iter().map(|row| server_dto(&repo, row)).collect()
}

#[tauri::command]
pub(crate) async fn test_mcp_server(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: String,
) -> Result<McpTestResultDto, String> {
    let repo = open_repo(&app)?;
    let mut row = require_server(&repo, &id)?;
    supervisor_before_call(&state, &id)?;
    let identity = load_vault_identity(&app_home(&app)?)?;
    match connect_and_list(&repo, identity.as_ref(), &row).await {
        Ok(tools) => {
            supervisor_record(&state, &id, true);
            update_health(&repo, &mut row, "healthy")?;
            Ok(McpTestResultDto {
                id,
                healthy: true,
                tool_count: Some(tools.len()),
                error: None,
            })
        }
        Err(error) => {
            supervisor_record(&state, &id, false);
            update_health(&repo, &mut row, "unhealthy")?;
            Ok(McpTestResultDto {
                id,
                healthy: false,
                tool_count: None,
                error: Some(error),
            })
        }
    }
}

#[tauri::command]
pub(crate) async fn discover_mcp_tools(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: String,
) -> Result<Vec<McpToolRow>, String> {
    let repo = open_repo(&app)?;
    let mut row = require_server(&repo, &id)?;
    if !row.enabled {
        return Err(format!("MCP server `{id}` is disabled"));
    }
    supervisor_before_call(&state, &id)?;
    let identity = load_vault_identity(&app_home(&app)?)?;
    let tools = match connect_and_list(&repo, identity.as_ref(), &row).await {
        Ok(tools) => {
            supervisor_record(&state, &id, true);
            tools
        }
        Err(error) => {
            supervisor_record(&state, &id, false);
            update_health(&repo, &mut row, "unhealthy")?;
            return Err(error);
        }
    };
    let existing = repo
        .list_mcp_tools(&id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|tool| (tool.name.clone(), tool))
        .collect::<HashMap<_, _>>();
    let release = registry_release(&row, &tools)?;
    let event = state
        .mcp
        .lock()
        .map_err(|_| "MCP supervisor state is unavailable".to_string())?
        .schemas
        .discover(release)
        .map_err(|error| error.to_string())?;
    let discovered_at = Utc::now();
    let registry = state
        .mcp
        .lock()
        .map_err(|_| "MCP supervisor state is unavailable".to_string())?;
    let rows = tools
        .into_iter()
        .map(|tool| {
            let previous = existing.get(&tool.name);
            let approved = registry.schemas.registry().visible_tool(&id, &tool.name);
            let pending = registry.schemas.registry().pending_tool(&id, &tool.name);
            match (approved, previous) {
                (Some(approved), _) if approved.input_schema == tool.input_schema => {
                    tool_row_with_hash(
                        &id,
                        tool,
                        previous,
                        discovered_at,
                        approved.schema_hash,
                        true,
                    )
                }
                (_, Some(previous)) => Ok(previous.clone()),
                _ => tool_row_with_hash(
                    &id,
                    tool,
                    None,
                    discovered_at,
                    pending
                        .map(|tool| tool.schema_hash)
                        .ok_or_else(|| "discovered MCP tool is missing from registry".to_string())?,
                    false,
                ),
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    drop(registry);
    repo.replace_mcp_tools(&id, &rows)
        .map_err(|error| error.to_string())?;
    update_health(&repo, &mut row, "healthy")?;
    if !event.changes.is_empty() {
        app.emit("lumo://mcp-schema-drift", &event)
            .map_err(|error| error.to_string())?;
    }
    Ok(rows)
}

#[tauri::command]
pub(crate) async fn call_mcp_tool(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: String,
    tool: String,
    arguments: Value,
) -> Result<Value, String> {
    let repo = open_repo(&app)?;
    let row = require_server(&repo, &id)?;
    if !row.enabled {
        return Err(format!("MCP server `{id}` is disabled"));
    }
    let stored_tool = repo
        .list_mcp_tools(&id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .find(|candidate| candidate.name == tool)
        .ok_or_else(|| format!("MCP tool `{id}:{tool}` not found; discover tools first"))?;
    if !stored_tool.enabled {
        return Err(format!("MCP tool `{id}:{tool}` is disabled"));
    }
    supervisor_before_call(&state, &id)?;
    let identity = load_vault_identity(&app_home(&app)?)?;
    let config = runtime_transport(&repo, identity.as_ref(), &row)?;
    let mut client = McpClient::connect(config, MCP_OPERATION_TIMEOUT)
        .await
        .map_err(|error| error.to_string())?;
    let result = client
        .call_tool(&tool, arguments)
        .await
        .map_err(|error| error.to_string());
    client.close().await;
    supervisor_record(&state, &id, result.is_ok());
    result
}

#[tauri::command]
pub(crate) fn delete_mcp_server(app: AppHandle, id: String) -> Result<bool, String> {
    open_repo(&app)?
        .delete_mcp_server(&id)
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn set_mcp_server_enabled(
    app: AppHandle,
    id: String,
    enabled: bool,
) -> Result<McpServerDto, String> {
    let repo = open_repo(&app)?;
    let mut row = require_server(&repo, &id)?;
    row.enabled = enabled;
    row.updated_at = Utc::now();
    repo.upsert_mcp_server(&row)
        .map_err(|error| error.to_string())?;
    server_dto(&repo, row)
}

#[tauri::command(rename_all = "camelCase")]
pub(crate) fn set_mcp_tool_enabled(
    app: AppHandle,
    id: String,
    tool: String,
    enabled: bool,
) -> Result<McpToolRow, String> {
    let repo = open_repo(&app)?;
    require_server(&repo, &id)?;
    let mut tools = repo
        .list_mcp_tools(&id)
        .map_err(|error| error.to_string())?;
    let updated = tools
        .iter_mut()
        .find(|candidate| candidate.name == tool)
        .ok_or_else(|| format!("MCP tool `{id}:{tool}` not found"))?;
    updated.enabled = enabled;
    let result = updated.clone();
    repo.replace_mcp_tools(&id, &tools)
        .map_err(|error| error.to_string())?;
    Ok(result)
}

fn preview_from_batch(token: String, batch: &McpImportBatch) -> McpImportPreviewDto {
    McpImportPreviewDto {
        token,
        servers: batch
            .servers
            .iter()
            .map(McpServerDraft::redacted_json)
            .collect(),
        secrets: batch
            .secrets
            .iter()
            .map(|secret| SecretPreviewDto {
                server_id: secret.server_id.clone(),
                field_path: secret.field_path.clone(),
                suggested_vault_key: secret.suggested_vault_key.clone(),
            })
            .collect(),
        warnings: batch.warnings.iter().map(warning_dto).collect(),
    }
}

fn warning_dto(warning: &ImportWarning) -> WarningDto {
    WarningDto {
        server_id: warning.server_id.clone(),
        message: warning.message.clone(),
    }
}

fn server_row_from_draft(
    draft: McpServerDraft,
    now: DateTime<Utc>,
) -> Result<McpServerRow, String> {
    let transport = match &draft.transport {
        McpTransportDraft::Stdio { .. } => "stdio",
        McpTransportDraft::StreamableHttp { .. } => "streamableHttp",
        McpTransportDraft::Sse { .. } => "sse",
    };
    let config = serde_json::to_value(StoredMcpConfig {
        transport: draft.transport,
        extensions: draft.extensions,
    })
    .map_err(|error| error.to_string())?;
    Ok(McpServerRow {
        id: draft.id,
        name: draft.name,
        transport: transport.into(),
        config,
        enabled: draft.enabled,
        health: "unknown".into(),
        created_at: now,
        updated_at: now,
    })
}

fn stored_config(row: &McpServerRow) -> Result<StoredMcpConfig, String> {
    serde_json::from_value(row.config.clone())
        .map_err(|error| format!("MCP server `{}` has invalid stored config: {error}", row.id))
}

fn set_server_extension(
    row: &mut McpServerRow,
    key: &str,
    value: Value,
) -> Result<(), String> {
    let mut stored = stored_config(row)?;
    stored.extensions.insert(key.into(), value);
    row.config = serde_json::to_value(stored).map_err(|error| error.to_string())?;
    Ok(())
}

fn extension<T: for<'de> Deserialize<'de>>(
    stored: &StoredMcpConfig,
    key: &str,
) -> Result<T, String> {
    let value = stored
        .extensions
        .get(key)
        .cloned()
        .ok_or_else(|| format!("MCP configuration extension `{key}` is missing"))?;
    serde_json::from_value(value).map_err(|error| format!("invalid MCP `{key}` metadata: {error}"))
}

fn require_server(repo: &Repo, id: &str) -> Result<McpServerRow, String> {
    repo.get_mcp_server(id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("MCP server `{id}` not found"))
}

fn server_dto(repo: &Repo, row: McpServerRow) -> Result<McpServerDto, String> {
    let tools = repo
        .list_mcp_tools(&row.id)
        .map_err(|error| error.to_string())?;
    Ok(McpServerDto {
        id: row.id,
        name: row.name,
        transport: row.transport,
        config: row.config,
        enabled: row.enabled,
        health: row.health,
        created_at: row.created_at,
        updated_at: row.updated_at,
        tools,
    })
}

fn resolve_secret_choice(
    candidate: &SecretCandidate,
    override_value: Option<&SecretOverrideDto>,
) -> Result<SecretChoice, String> {
    if let Some(item) = override_value {
        if let Some(value) = item.value.as_ref() {
            if value.is_empty() {
                return Err(format!(
                    "secret `{}` for MCP server `{}` is empty",
                    candidate.field_path, candidate.server_id
                ));
            }
            return Ok(SecretChoice::Store {
                vault_ref: item
                    .vault_key
                    .clone()
                    .unwrap_or_else(|| candidate.suggested_vault_key.clone()),
                value: value.clone(),
            });
        }
        if let Some(vault_ref) = item.vault_key.as_ref().filter(|value| !value.is_empty()) {
            return Ok(SecretChoice::Existing {
                vault_ref: vault_ref.clone(),
            });
        }
    }
    if candidate.value.is_empty() {
        return Err(format!(
            "secret `{}` for MCP server `{}` must be supplied or mapped to a vault key",
            candidate.field_path, candidate.server_id
        ));
    }
    Ok(SecretChoice::Store {
        vault_ref: candidate.suggested_vault_key.clone(),
        value: candidate.value.clone(),
    })
}

fn set_vault_ref(
    transport: &mut McpTransportDraft,
    field_path: &str,
    vault_ref: &str,
) -> Result<(), String> {
    let (prefix, field) = field_path
        .split_once('.')
        .ok_or_else(|| format!("invalid MCP secret field `{field_path}`"))?;
    let values = match (transport, prefix) {
        (McpTransportDraft::Stdio { env, .. }, "env") => env,
        (McpTransportDraft::StreamableHttp { headers, .. }, "headers")
        | (McpTransportDraft::Sse { headers, .. }, "headers") => headers,
        _ => {
            return Err(format!(
                "MCP secret field `{field_path}` does not match transport"
            ))
        }
    };
    let value = values
        .get_mut(field)
        .ok_or_else(|| format!("MCP secret field `{field_path}` not found"))?;
    *value = ConfigValue::VaultRef(vault_ref.to_string());
    Ok(())
}

fn persist_secret_choices(
    repo: &Repo,
    home: &Path,
    choices: Vec<SecretChoice>,
) -> Result<(), String> {
    if choices.is_empty() {
        return Ok(());
    }
    let identity_path = vault_identity_path(home);
    let needs_store = choices
        .iter()
        .any(|choice| matches!(choice, SecretChoice::Store { .. }));
    let identity = if identity_path.exists() {
        VaultIdentity::load(&identity_path).map_err(|error| error.to_string())?
    } else if needs_store {
        let identity = VaultIdentity::generate();
        identity
            .save(&identity_path)
            .map_err(|error| error.to_string())?;
        identity
    } else {
        return Err(format!(
            "vault identity not found at {}; cannot verify mapped MCP secrets",
            identity_path.display()
        ));
    };
    let vault = Vault::new(repo, &identity);
    let mut namespaces: HashMap<String, BTreeMap<String, String>> = HashMap::new();
    for choice in choices {
        match choice {
            SecretChoice::Store { vault_ref, value } => {
                let (name, key) = split_vault_ref(&vault_ref)?;
                if !namespaces.contains_key(&name) {
                    namespaces.insert(
                        name.clone(),
                        vault
                            .get(&name)
                            .map_err(|error| error.to_string())?
                            .unwrap_or_default(),
                    );
                }
                namespaces
                    .get_mut(&name)
                    .expect("namespace inserted above")
                    .insert(key, value);
            }
            SecretChoice::Existing { vault_ref } => {
                let (name, key) = split_vault_ref(&vault_ref)?;
                let fields = vault
                    .get(&name)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| format!("vault key `{vault_ref}` is missing"))?;
                if !fields.contains_key(&key) {
                    return Err(format!("vault key `{vault_ref}` is missing"));
                }
            }
        }
    }
    for (name, fields) in namespaces {
        vault
            .put(&name, &fields)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn ensure_vault_identity(home: &Path) -> Result<VaultIdentity, String> {
    let path = vault_identity_path(home);
    if path.exists() {
        VaultIdentity::load(&path).map_err(|error| error.to_string())
    } else {
        let identity = VaultIdentity::generate();
        identity.save(&path).map_err(|error| error.to_string())?;
        Ok(identity)
    }
}

pub(super) fn runtime_transport(
    repo: &Repo,
    identity: Option<&VaultIdentity>,
    row: &McpServerRow,
) -> Result<McpTransportConfig, String> {
    let stored: StoredMcpConfig = serde_json::from_value(row.config.clone())
        .map_err(|error| format!("MCP server `{}` has invalid stored config: {error}", row.id))?;
    match stored.transport {
        McpTransportDraft::Stdio { command, args, env } => Ok(McpTransportConfig::Stdio {
            command,
            args,
            env: resolve_config_values(repo, identity, env)?,
        }),
        McpTransportDraft::StreamableHttp { url, headers } => {
            Ok(McpTransportConfig::StreamableHttp {
                url,
                headers: resolve_config_values(repo, identity, headers)?,
            })
        }
        McpTransportDraft::Sse { .. } => Err(format!(
            "MCP server `{}` uses legacy SSE transport; legacy SSE transport is not supported for calls yet",
            row.id
        )),
    }
}

fn resolve_config_values(
    repo: &Repo,
    identity: Option<&VaultIdentity>,
    values: BTreeMap<String, ConfigValue>,
) -> Result<BTreeMap<String, String>, String> {
    values
        .into_iter()
        .map(|(name, value)| {
            let resolved = match value {
                ConfigValue::Plain(value) => value,
                ConfigValue::VaultRef(vault_ref) => resolve_vault_ref(repo, identity, &vault_ref)?,
            };
            Ok((name, resolved))
        })
        .collect()
}

fn resolve_vault_ref(
    repo: &Repo,
    identity: Option<&VaultIdentity>,
    vault_ref: &str,
) -> Result<String, String> {
    let (name, key) = split_vault_ref(vault_ref)?;
    let env_key = if key.is_empty() {
        format!("LUMO_VAULT_{}", sanitize_env(&name))
    } else {
        format!("LUMO_VAULT_{}_{}", sanitize_env(&name), sanitize_env(&key))
    };
    if let Ok(value) = std::env::var(&env_key) {
        return Ok(value);
    }
    let identity = identity.ok_or_else(|| {
        format!("vault identity is required to resolve MCP vault key `{vault_ref}`")
    })?;
    let fields = Vault::new(repo, identity)
        .get(&name)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("vault key `{vault_ref}` is missing"))?;
    fields
        .get(&key)
        .cloned()
        .ok_or_else(|| format!("vault key `{vault_ref}` is missing"))
}

fn split_vault_ref(vault_ref: &str) -> Result<(String, String), String> {
    let mut parts = vault_ref.split('.').filter(|part| !part.is_empty());
    let name = parts
        .next()
        .ok_or_else(|| format!("invalid vault key `{vault_ref}`"))?;
    let key = parts.collect::<Vec<_>>().join("_");
    if key.is_empty() {
        return Err(format!("invalid vault key `{vault_ref}`"));
    }
    Ok((name.to_string(), key))
}

fn sanitize_env(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn vault_identity_path(home: &Path) -> PathBuf {
    std::env::var_os("LUMO_VAULT_IDENTITY")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("age-identity.txt"))
}

pub(super) fn load_vault_identity(home: &Path) -> Result<Option<VaultIdentity>, String> {
    let path = vault_identity_path(home);
    if path.exists() {
        VaultIdentity::load(&path)
            .map(Some)
            .map_err(|error| error.to_string())
    } else {
        Ok(None)
    }
}

async fn connect_and_list(
    repo: &Repo,
    identity: Option<&VaultIdentity>,
    row: &McpServerRow,
) -> Result<Vec<McpTool>, String> {
    let config = runtime_transport(repo, identity, row)?;
    let mut client = McpClient::connect(config, MCP_OPERATION_TIMEOUT)
        .await
        .map_err(|error| error.to_string())?;
    let result = client.list_tools().await.map_err(|error| error.to_string());
    client.close().await;
    result
}

fn update_health(repo: &Repo, row: &mut McpServerRow, health: &str) -> Result<(), String> {
    row.health = health.to_string();
    row.updated_at = Utc::now();
    repo.upsert_mcp_server(row)
        .map_err(|error| error.to_string())
}

fn supervisor_before_call(state: &State<'_, DesktopState>, id: &str) -> Result<(), String> {
    let now = Instant::now();
    state
        .mcp
        .lock()
        .map_err(|_| "MCP supervisor state is unavailable".to_string())?
        .health_mut(id, now)
        .before_call(now)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn supervisor_record(state: &State<'_, DesktopState>, id: &str, success: bool) {
    let now = Instant::now();
    let Ok(mut runtime) = state.mcp.lock() else {
        return;
    };
    let health = runtime.health_mut(id, now);
    if success {
        health.record_success();
    } else {
        health.record_failure(now);
    }
}

fn registry_release(row: &McpServerRow, tools: &[McpTool]) -> Result<McpRelease, String> {
    let stored = stored_config(row)?;
    let version = stored
        .extensions
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("unversioned")
        .to_string();
    let publisher = stored
        .extensions
        .get("publisher")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(|error| format!("invalid MCP publisher metadata: {error}"))?
        .unwrap_or_else(|| McpPublisherMetadata {
            id: format!("local.{}", row.id),
            display_name: row.name.clone(),
            website: None,
            signature: None,
        });
    Ok(McpRelease {
        server_id: row.id.clone(),
        version,
        publisher,
        tools: tools
            .iter()
            .map(|tool| McpToolDefinition {
                name: tool.name.clone(),
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
            .collect(),
    })
}

fn tool_row_with_hash(
    server_id: &str,
    tool: McpTool,
    existing: Option<&McpToolRow>,
    discovered_at: DateTime<Utc>,
    version_hash: String,
    enabled: bool,
) -> Result<McpToolRow, String> {
    Ok(McpToolRow {
        server_id: server_id.to_string(),
        name: tool.name,
        description: tool.description,
        input_schema: tool.input_schema,
        output_schema: tool.output_schema,
        risk: existing.map_or_else(|| "L1".to_string(), |row| row.risk.clone()),
        enabled,
        version_hash,
        discovered_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lumo_agent::{
        ConfigValue, McpImportBatch, McpServerDraft, McpTransportDraft, SecretCandidate,
    };
    use lumo_storage::{McpServerRow, Repo};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn imported_batch(secret_value: &str) -> McpImportBatch {
        McpImportBatch {
            servers: vec![McpServerDraft {
                id: "demo".into(),
                name: "Demo".into(),
                transport: McpTransportDraft::Stdio {
                    command: "demo-mcp".into(),
                    args: vec!["--stdio".into()],
                    env: BTreeMap::from([
                        ("PUBLIC".into(), ConfigValue::Plain("visible".into())),
                        (
                            "API_TOKEN".into(),
                            ConfigValue::VaultRef("mcp.demo.api-token".into()),
                        ),
                    ]),
                },
                enabled: true,
                extensions: serde_json::Map::new(),
            }],
            secrets: vec![SecretCandidate {
                server_id: "demo".into(),
                field_path: "env.API_TOKEN".into(),
                suggested_vault_key: "mcp.demo.api-token".into(),
                value: secret_value.into(),
            }],
            warnings: Vec::new(),
        }
    }

    #[test]
    fn preview_dto_never_serializes_secret_candidate_value() {
        let dto = preview_from_batch("pending-token".into(), &imported_batch("super-secret"));
        let encoded = serde_json::to_string(&dto).unwrap();
        assert!(!encoded.contains("super-secret"));
        assert!(encoded.contains("mcp.demo.api-token"));
    }

    #[test]
    fn stored_profile_round_trips_to_runtime_transport_without_plaintext_secrets() {
        let draft = imported_batch("super-secret").servers.remove(0);
        let row = server_row_from_draft(draft, chrono::Utc::now()).unwrap();
        let encoded = serde_json::to_string(&row.config).unwrap();
        assert!(!encoded.contains("super-secret"));

        let repo = Repo::open_in_memory().unwrap();
        let identity = VaultIdentity::generate();
        Vault::new(&repo, &identity)
            .put(
                "mcp",
                &BTreeMap::from([("demo_api-token".into(), "super-secret".into())]),
            )
            .unwrap();
        let runtime = runtime_transport(&repo, Some(&identity), &row).unwrap();
        let McpTransportConfig::Stdio { command, args, env } = runtime else {
            panic!("stored stdio profile converted to a different transport");
        };
        assert_eq!(command, "demo-mcp");
        assert_eq!(args, ["--stdio"]);
        assert!(matches!(
            env.get("API_TOKEN"),
            Some(value) if value == "super-secret"
        ));
    }

    #[test]
    fn loading_a_missing_server_is_an_explicit_error() {
        let repo = Repo::open_in_memory().unwrap();
        let error = require_server(&repo, "missing").unwrap_err();
        assert_eq!(error, "MCP server `missing` not found");
    }

    #[test]
    fn empty_imported_secret_without_override_is_rejected() {
        let batch = imported_batch("");
        let error = match resolve_secret_choice(&batch.secrets[0], None) {
            Ok(_) => panic!("empty secret unexpectedly accepted"),
            Err(error) => error,
        };
        assert!(error.contains("env.API_TOKEN"));
    }

    #[test]
    fn legacy_sse_profile_returns_an_unsupported_transport_error() {
        let row = McpServerRow {
            id: "legacy".into(),
            name: "Legacy".into(),
            transport: "sse".into(),
            config: json!({
                "transport": {
                    "kind": "sse",
                    "url": "https://example.test/sse",
                    "headers": {}
                }
            }),
            enabled: true,
            health: "unknown".into(),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let repo = Repo::open_in_memory().unwrap();
        let error = runtime_transport(&repo, None, &row).unwrap_err();
        assert!(
            error.contains("legacy SSE transport is not supported"),
            "unexpected error: {error}"
        );
    }
}
