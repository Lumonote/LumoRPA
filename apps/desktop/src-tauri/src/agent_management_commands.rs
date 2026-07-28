use super::{app_home, AppHandle};
use chrono::Utc;
use lumo_agent::{ContentOrigin, ImprovementProposal, ImprovementTarget};
use lumo_storage::Repo;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};
use tauri::command;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AgentProfileDto {
    id: String,
    name: String,
    model: String,
    budgets: Value,
    is_default: bool,
    config: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ImprovementProposalDto {
    id: String,
    target: String,
    target_id: String,
    patch: Value,
    patch_hash: String,
    rationale: String,
    status: String,
    base_version_hash: String,
    evaluation: Option<Value>,
}

#[derive(Debug, Clone)]
pub(super) struct ImprovementOverlay {
    pub(super) target_kind: String,
    pub(super) target_id: String,
    pub(super) content: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImprovementArtifactDocument {
    target_kind: String,
    target_id: String,
    active_version_hash: String,
    versions: BTreeMap<String, ImprovementArtifactVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImprovementArtifactVersion {
    content: Value,
    previous_version_hash: Option<String>,
    proposal_id: Option<String>,
    approver: Option<String>,
    created_at: i64,
}

fn open_repo(app: &AppHandle) -> Result<Repo, String> {
    Repo::open(app_home(app)?.join("lumo.db")).map_err(|error| error.to_string())
}

#[command]
pub(super) fn list_agent_profiles(app: AppHandle) -> Result<Vec<AgentProfileDto>, String> {
    list_agent_profiles_at(&open_repo(&app)?)
}

fn list_agent_profiles_at(repo: &Repo) -> Result<Vec<AgentProfileDto>, String> {
    let stored = repo.with_raw(|connection| {
        let mut statement = connection.prepare(
            "SELECT id,name,config_json,is_default FROM agent_profiles ORDER BY is_default DESC,updated_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
    .map_err(|error| error.to_string())?;
    let rows = if stored.is_empty() {
        let draft = lumo_agent::AgentProfileDraft::default();
        vec![(
            draft.id.clone(),
            draft.name.clone(),
            serde_json::to_string(&draft).map_err(|error| error.to_string())?,
            true,
        )]
    } else {
        stored
    };
    rows.into_iter()
        .map(|(id, name, raw, is_default)| {
            let config: Value = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
            let budgets = json!({
                "tools": config.get("maxSteps").cloned().unwrap_or(json!(0)),
                "tokens": config.get("maxTokens").cloned().unwrap_or(json!(0)),
                "runtimeMs": config.get("maxRuntimeMs").cloned().unwrap_or(json!(0)),
                "costUsdMicro": config.get("maxCostUsdMicro").cloned().unwrap_or(json!(0)),
            });
            Ok(AgentProfileDto {
                id,
                name,
                model: config
                    .get("plannerProvider")
                    .and_then(Value::as_str)
                    .unwrap_or("default")
                    .into(),
                budgets,
                is_default,
                config,
            })
        })
        .collect()
}

#[command]
pub(super) fn list_improvement_proposals(
    app: AppHandle,
) -> Result<Vec<ImprovementProposalDto>, String> {
    list_improvement_proposals_at(&open_repo(&app)?)
}

fn list_improvement_proposals_at(repo: &Repo) -> Result<Vec<ImprovementProposalDto>, String> {
    repo.with_raw(|connection| {
        let mut statement = connection.prepare(
            "SELECT id,target_kind,target_id,patch_json,rationale,status,base_version_hash,evaluation_json FROM improvement_proposals ORDER BY updated_at DESC",
        )?;
        let rows = statement.query_map([], |row| {
            (0..8).map(|index| row.get::<_, Option<String>>(index)).collect::<rusqlite::Result<Vec<_>>>()
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
    })
    .map_err(|error| error.to_string())?
    .into_iter()
    .map(|values| {
        let text = |index: usize| values[index].clone().unwrap_or_default();
        let patch: Value = serde_json::from_str(&text(3)).map_err(|error| error.to_string())?;
        Ok(ImprovementProposalDto {
            id: text(0),
            target: text(1),
            target_id: text(2),
            patch_hash: hash_patch(&patch),
            patch,
            rationale: text(4),
            status: text(5),
            base_version_hash: text(6),
            evaluation: values[7].as_deref().map(serde_json::from_str).transpose().map_err(|error| error.to_string())?,
        })
    })
    .collect()
}

#[command]
pub(super) fn evaluate_improvement(app: AppHandle, proposal_id: String) -> Result<Value, String> {
    let repo = open_repo(&app)?;
    let proposal = load_proposal(&repo, &proposal_id)?;
    validate_proposal(&proposal)?;
    let report = evaluate_proposal_at(&app_home(&app)?, &proposal)?;
    update_proposal(&repo, &proposal_id, "evaluated", Some(&report))?;
    Ok(report)
}

#[command]
pub(super) fn approve_improvement(
    app: AppHandle,
    proposal_id: String,
    patch_hash: String,
) -> Result<(), String> {
    let repo = open_repo(&app)?;
    let proposal = load_proposal(&repo, &proposal_id)?;
    if proposal.status != "evaluated" {
        return Err("improvement proposal must be evaluated before approval".into());
    }
    if proposal
        .evaluation
        .as_ref()
        .and_then(|report| report.get("passed"))
        .and_then(Value::as_bool)
        != Some(true)
    {
        return Err("improvement proposal did not pass evaluation".into());
    }
    let expected = hash_patch(&proposal.patch);
    if patch_hash != expected {
        return Err("improvement approval patch hash is stale".into());
    }
    validate_proposal(&proposal)?;
    let home = app_home(&app)?;
    let (artifact_path, previous_artifact) =
        apply_improvement_artifact(&home, &proposal, &expected)?;
    let now = Utc::now().timestamp_millis();
    let persisted = repo.with_raw(|connection| {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO improvement_approvals(proposal_id,patch_hash,base_version_hash,approver,decision,created_at) VALUES(?,?,?,?,?,?)",
            rusqlite::params![proposal_id, expected, proposal.base_version_hash, "desktop-user", "approved", now],
        )?;
        transaction.execute("UPDATE improvement_proposals SET status='applied',updated_at=? WHERE id=?", rusqlite::params![now, proposal.id])?;
        transaction.commit()?;
        Ok(())
    }).map_err(|error| error.to_string());
    if let Err(error) = persisted {
        restore_artifact(&artifact_path, previous_artifact.as_deref())?;
        return Err(error);
    }
    Ok(())
}

#[command]
pub(super) fn reject_improvement(app: AppHandle, proposal_id: String) -> Result<(), String> {
    update_proposal(&open_repo(&app)?, &proposal_id, "rejected", None)
}

#[command]
pub(super) fn rollback_improvement(app: AppHandle, proposal_id: String) -> Result<(), String> {
    let repo = open_repo(&app)?;
    let proposal = load_proposal(&repo, &proposal_id)?;
    if !matches!(proposal.status.as_str(), "approved" | "applied") {
        return Err("only approved or applied proposals can be rolled back".into());
    }
    let home = app_home(&app)?;
    let (artifact_path, previous_artifact) = rollback_improvement_artifact(&home, &proposal)?;
    if let Err(error) = update_proposal(&repo, &proposal_id, "rolled_back", None) {
        restore_artifact(&artifact_path, previous_artifact.as_deref())?;
        return Err(error);
    }
    Ok(())
}

pub(super) fn load_active_improvement_overlays(
    home: &Path,
) -> Result<Vec<ImprovementOverlay>, String> {
    let root = home.join("improvements").join("artifacts");
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut overlays = Vec::new();
    for entry in fs::read_dir(&root).map_err(|error| error.to_string())? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let document: ImprovementArtifactDocument = serde_json::from_slice(
            &fs::read(&path).map_err(|error| error.to_string())?,
        )
        .map_err(|error| format!("invalid improvement artifact `{}`: {error}", path.display()))?;
        let active = document
            .versions
            .get(&document.active_version_hash)
            .ok_or_else(|| {
                format!(
                    "improvement artifact `{}` has no active version",
                    path.display()
                )
            })?;
        overlays.push(ImprovementOverlay {
            target_kind: document.target_kind,
            target_id: document.target_id,
            content: active.content.clone(),
        });
    }
    Ok(overlays)
}

fn apply_improvement_artifact(
    home: &Path,
    proposal: &StoredProposal,
    patch_hash: &str,
) -> Result<(PathBuf, Option<Vec<u8>>), String> {
    let path = improvement_artifact_path(home, &proposal.target_kind, &proposal.target_id);
    let previous = fs::read(&path).ok();
    let mut document = match previous.as_deref() {
        Some(bytes) => serde_json::from_slice::<ImprovementArtifactDocument>(bytes)
            .map_err(|error| error.to_string())?,
        None => ImprovementArtifactDocument {
            target_kind: proposal.target_kind.clone(),
            target_id: proposal.target_id.clone(),
            active_version_hash: proposal.base_version_hash.clone(),
            versions: BTreeMap::from([(
                proposal.base_version_hash.clone(),
                ImprovementArtifactVersion {
                    content: json!({}),
                    previous_version_hash: None,
                    proposal_id: None,
                    approver: None,
                    created_at: Utc::now().timestamp_millis(),
                },
            )]),
        },
    };
    if document.active_version_hash != proposal.base_version_hash {
        return Err(format!(
            "improvement base is stale: expected `{}`, active `{}`",
            proposal.base_version_hash, document.active_version_hash
        ));
    }
    let mut content = document
        .versions
        .get(&document.active_version_hash)
        .ok_or_else(|| "active improvement artifact version is missing".to_string())?
        .content
        .clone();
    merge_patch(&mut content, &proposal.patch);
    let seed = json!([
        document.active_version_hash,
        proposal.id,
        patch_hash,
        content
    ]);
    let version_hash = format!("{:x}", Sha256::digest(seed.to_string().as_bytes()));
    document.versions.insert(
        version_hash.clone(),
        ImprovementArtifactVersion {
            content,
            previous_version_hash: Some(document.active_version_hash.clone()),
            proposal_id: Some(proposal.id.clone()),
            approver: Some("desktop-user".into()),
            created_at: Utc::now().timestamp_millis(),
        },
    );
    document.active_version_hash = version_hash;
    write_artifact(&path, &document)?;
    Ok((path, previous))
}

fn rollback_improvement_artifact(
    home: &Path,
    proposal: &StoredProposal,
) -> Result<(PathBuf, Option<Vec<u8>>), String> {
    let path = improvement_artifact_path(home, &proposal.target_kind, &proposal.target_id);
    let previous =
        fs::read(&path).map_err(|_| "applied improvement artifact is missing".to_string())?;
    let mut document: ImprovementArtifactDocument =
        serde_json::from_slice(&previous).map_err(|error| error.to_string())?;
    let active = document
        .versions
        .get(&document.active_version_hash)
        .ok_or_else(|| "active improvement artifact version is missing".to_string())?;
    if active.proposal_id.as_deref() != Some(proposal.id.as_str()) {
        return Err("improvement is no longer the active artifact version".into());
    }
    document.active_version_hash = active
        .previous_version_hash
        .clone()
        .ok_or_else(|| "improvement artifact has no rollback version".to_string())?;
    write_artifact(&path, &document)?;
    Ok((path, Some(previous)))
}

fn improvement_artifact_path(home: &Path, target_kind: &str, target_id: &str) -> PathBuf {
    let key = format!(
        "{:x}",
        Sha256::digest(format!("{target_kind}\0{target_id}").as_bytes())
    );
    home.join("improvements")
        .join("artifacts")
        .join(format!("{key}.json"))
}

fn write_artifact(path: &Path, document: &ImprovementArtifactDocument) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "improvement artifact path has no parent".to_string())?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.tmp");
    fs::write(
        &temporary,
        serde_json::to_vec_pretty(document).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn restore_artifact(path: &Path, previous: Option<&[u8]>) -> Result<(), String> {
    match previous {
        Some(bytes) => fs::write(path, bytes).map_err(|error| error.to_string()),
        None if path.exists() => fs::remove_file(path).map_err(|error| error.to_string()),
        None => Ok(()),
    }
}

fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(patch) => {
            if !target.is_object() {
                *target = json!({});
            }
            let target = target.as_object_mut().expect("object initialized above");
            for (key, value) in patch {
                if value.is_null() {
                    target.remove(key);
                } else {
                    merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
                }
            }
        }
        replacement => *target = replacement.clone(),
    }
}

struct StoredProposal {
    id: String,
    target_kind: String,
    target_id: String,
    patch: Value,
    rationale: String,
    status: String,
    base_version_hash: String,
    evaluation: Option<Value>,
}

fn load_proposal(repo: &Repo, id: &str) -> Result<StoredProposal, String> {
    let row = repo.with_raw(|connection| {
        connection.query_row(
            "SELECT id,target_kind,target_id,patch_json,rationale,status,base_version_hash,evaluation_json FROM improvement_proposals WHERE id=?",
            [id],
            |row| Ok((row.get::<_, String>(0)?,row.get::<_, String>(1)?,row.get::<_, String>(2)?,row.get::<_, String>(3)?,row.get::<_, String>(4)?,row.get::<_, String>(5)?,row.get::<_, String>(6)?,row.get::<_, Option<String>>(7)?)),
        ).optional()
    }).map_err(|error| error.to_string())?.ok_or_else(|| format!("improvement proposal `{id}` was not found"))?;
    Ok(StoredProposal {
        id: row.0,
        target_kind: row.1,
        target_id: row.2,
        patch: serde_json::from_str(&row.3).map_err(|error| error.to_string())?,
        rationale: row.4,
        status: row.5,
        base_version_hash: row.6,
        evaluation: row
            .7
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|error| error.to_string())?,
    })
}

fn evaluate_proposal_at(home: &Path, proposal: &StoredProposal) -> Result<Value, String> {
    let mut failures = Vec::<String>::new();
    let object = proposal
        .patch
        .as_object()
        .ok_or_else(|| "improvement patch must be an object".to_string())?;
    let allowed: &[&str] = match proposal.target_kind.as_str() {
        "alias" => &["aliases"],
        "router_example" => &["examples"],
        "prompt_template" => &[
            "systemPrompt",
            "template",
            "model",
            "temperature",
            "maxTokens",
        ],
        "flow_patch" | "skill_patch" => &[
            "aliases",
            "examples",
            "description",
            "enabled",
            "inputSchema",
            "outputSchema",
            "risk",
        ],
        _ => &[],
    };
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            failures.push(format!(
                "patch key `{key}` is not allowed for {}",
                proposal.target_kind
            ));
        }
    }
    for key in ["aliases", "examples"] {
        if let Some(value) = object.get(key) {
            let valid = value.as_array().is_some_and(|items| {
                !items.is_empty()
                    && items
                        .iter()
                        .all(|item| item.as_str().is_some_and(|text| !text.trim().is_empty()))
            });
            if !valid {
                failures.push(format!("`{key}` must be a non-empty string array"));
            }
        }
    }
    if proposal.target_kind == "prompt_template" {
        let prompt = object
            .get("systemPrompt")
            .or_else(|| object.get("template"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        if prompt.trim().is_empty() {
            failures.push("prompt template must provide non-empty systemPrompt or template".into());
        }
        if prompt.len() > 32_768 {
            failures.push("prompt template exceeds 32 KiB".into());
        }
    }
    let risk_delta = object
        .get("risk")
        .and_then(Value::as_str)
        .map(|risk| match risk.to_ascii_uppercase().as_str() {
            "L0" => 0,
            "L1" => 1,
            "L2" => 2,
            "L3" => 3,
            _ => 99,
        })
        .unwrap_or(0);
    if risk_delta > 1 {
        failures.push("permission expansion above L1 requires a separate security grant".into());
    }
    if object
        .get("inputSchema")
        .is_some_and(|value| !value.is_object())
        || object
            .get("outputSchema")
            .is_some_and(|value| !value.is_object() && !value.is_null())
    {
        failures.push("schema patches must be JSON objects".into());
    }
    let artifact_path = improvement_artifact_path(home, &proposal.target_kind, &proposal.target_id);
    let base_fresh = if artifact_path.exists() {
        let document: ImprovementArtifactDocument =
            serde_json::from_slice(&fs::read(&artifact_path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        document.active_version_hash == proposal.base_version_hash
    } else {
        true
    };
    if !base_fresh {
        failures.push("proposal base version is stale".into());
    }
    Ok(json!({
        "passed": failures.is_empty(),
        "mode": "structural_policy_and_base",
        "failures": failures,
        "baseFresh": base_fresh,
        "riskDelta": risk_delta,
        "permissionDelta": { "added": if risk_delta > 1 { vec![format!("risk:L{risk_delta}")] } else { Vec::<String>::new() }, "removed": [] },
        "successDelta": 0.0,
        "latencyDeltaMs": 0
    }))
}

fn validate_proposal(stored: &StoredProposal) -> Result<(), String> {
    let target = ImprovementTarget::from_kind(&stored.target_kind, stored.target_id.clone())
        .map_err(|error| error.to_string())?;
    ImprovementProposal::new(
        &stored.id,
        vec!["desktop-evaluation".into()],
        target,
        stored.patch.clone(),
        &stored.rationale,
        &stored.base_version_hash,
        ContentOrigin::Trace,
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn update_proposal(
    repo: &Repo,
    id: &str,
    status: &str,
    evaluation: Option<&Value>,
) -> Result<(), String> {
    let changed = repo.with_raw(|connection| connection.execute(
        "UPDATE improvement_proposals SET status=?,evaluation_json=COALESCE(?,evaluation_json),updated_at=? WHERE id=?",
        rusqlite::params![status, evaluation.map(Value::to_string), Utc::now().timestamp_millis(), id],
    )).map_err(|error| error.to_string())?;
    if changed == 0 {
        Err(format!("improvement proposal `{id}` was not found"))
    } else {
        Ok(())
    }
}

fn hash_patch(patch: &Value) -> String {
    format!("{:x}", Sha256::digest(patch.to_string().as_bytes()))
}

use rusqlite::OptionalExtension;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_profiles_and_governs_proposals_from_storage() {
        let temp = tempfile::tempdir().unwrap();
        let repo = Repo::open(temp.path().join("lumo.db")).unwrap();
        let now = Utc::now().timestamp_millis();
        repo.with_raw(|connection| {
            connection.execute("INSERT INTO agent_profiles(id,name,config_json,is_default,updated_at) VALUES(?,?,?,?,?)", rusqlite::params!["safe","Safe",json!({"plannerProvider":"local","maxSteps":20,"maxTokens":1000,"maxRuntimeMs":10000,"maxCostUsdMicro":0}).to_string(),1,now])?;
            connection.execute("INSERT INTO improvement_proposals(id,target_kind,target_id,patch_json,rationale,status,base_version_hash,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?)", rusqlite::params!["p1","alias","flow.demo",json!({"aliases":["demo"]}).to_string(),"trace improvement","pending","base",now,now])?;
            Ok(())
        }).unwrap();
        assert_eq!(list_agent_profiles_at(&repo).unwrap().len(), 1);
        let proposal = load_proposal(&repo, "p1").unwrap();
        validate_proposal(&proposal).unwrap();
        update_proposal(&repo, "p1", "evaluated", Some(&json!({"passed":true}))).unwrap();
        assert_eq!(
            list_improvement_proposals_at(&repo).unwrap()[0].status,
            "evaluated"
        );
    }

    #[test]
    fn improvement_artifact_applies_and_rolls_back_durably() {
        let temp = tempfile::tempdir().unwrap();
        let proposal = StoredProposal {
            id: "p-overlay".into(),
            target_kind: "alias".into(),
            target_id: "flow.demo".into(),
            patch: json!({"aliases":["演示流程"]}),
            rationale: "improve routing".into(),
            status: "evaluated".into(),
            base_version_hash: "base-v1".into(),
            evaluation: Some(json!({"passed":true})),
        };
        let patch_hash = hash_patch(&proposal.patch);
        apply_improvement_artifact(temp.path(), &proposal, &patch_hash).unwrap();
        let overlays = load_active_improvement_overlays(temp.path()).unwrap();
        assert_eq!(overlays[0].content, json!({"aliases":["演示流程"]}));

        rollback_improvement_artifact(temp.path(), &proposal).unwrap();
        let overlays = load_active_improvement_overlays(temp.path()).unwrap();
        assert_eq!(overlays[0].content, json!({}));
    }

    #[test]
    fn improvement_evaluation_rejects_permission_expansion() {
        let temp = tempfile::tempdir().unwrap();
        let proposal = StoredProposal {
            id: "p-risk".into(),
            target_kind: "skill_patch".into(),
            target_id: "demo".into(),
            patch: json!({"risk":"L3"}),
            rationale: "raise risk".into(),
            status: "pending".into(),
            base_version_hash: "base".into(),
            evaluation: None,
        };
        let report = evaluate_proposal_at(temp.path(), &proposal).unwrap();
        assert_eq!(report["passed"], false);
        assert!(report["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.as_str().unwrap().contains("permission expansion")));
    }
}
