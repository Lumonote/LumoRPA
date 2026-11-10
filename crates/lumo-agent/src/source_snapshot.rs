use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Component, Path, PathBuf},
};

use lumo_core::{
    source_snapshot::{absolute_path, hash_file, MAX_SOURCE_BYTES, MAX_SOURCE_FILES},
    SourceSnapshot,
};
use lumo_dsl::{Flow, Step};
use lumo_skills::SkillRegistry;

use crate::{AgentPlan, CapabilityCatalog, CapabilitySource};

pub const SOURCE_SNAPSHOTS_KEY: &str = "sourceSnapshots";

/// Capture the dependency universe for dynamic subflow/skill selection as well
/// as literal references. Execution never falls back to a newly installed file.
pub fn capture_source_snapshot(
    root: &Path,
    skills: &SkillRegistry,
) -> Result<SourceSnapshot, String> {
    let root = absolute_path(root)?;
    let flow_base = root
        .parent()
        .ok_or("flow has no parent directory")?
        .to_path_buf();
    let mut snapshot = SourceSnapshot {
        root: root.clone(),
        flow_base: flow_base.clone(),
        sources: BTreeMap::new(),
        skills: BTreeMap::new(),
        dependencies: BTreeMap::new(),
    };
    let mut pending = BTreeSet::from([root]);
    for skill in skills.all() {
        let path = absolute_path(&skill.source)?;
        snapshot
            .skills
            .insert(skill.name().to_string(), path.clone());
        if path == snapshot.root {
            pending.insert(path);
        }
    }
    let mut source_bytes = 0;
    let mut dynamic_flows_collected = false;
    while let Some(path) = pending.pop_first() {
        if snapshot.sources.len() >= MAX_SOURCE_FILES {
            return Err("source snapshot exceeds 1024 files or 8 MiB".into());
        }
        let bytes = read_bounded_source(&path, MAX_SOURCE_BYTES - source_bytes)?
            .ok_or("source snapshot exceeds 1024 files or 8 MiB")?;
        let text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        let flow = parse_source(&path, &text)?;
        source_bytes += text.len();
        snapshot.sources.insert(path.clone(), text);
        for dependency in &flow.spec.dependencies {
            let full = confined_path(path.parent().ok_or("source has no parent")?, dependency)?;
            snapshot
                .dependencies
                .insert(full.clone(), hash_file(&full)?);
        }
        collect_references(
            &flow.spec.steps,
            &flow_base,
            skills,
            &mut pending,
            &snapshot.sources,
            &mut dynamic_flows_collected,
        )?;
    }
    // The registry is the lookup universe, while the persisted skill map must
    // describe only captured sources. Unrelated installed Skills must not make
    // an otherwise independent static flow fail snapshot validation.
    snapshot
        .skills
        .retain(|_, path| snapshot.sources.contains_key(path));
    snapshot.validate()?;
    Ok(snapshot)
}

/// Bound the read itself, including files which grow after their initial stat.
/// Non-regular sources (for example FIFOs) must be rejected before open/read.
fn read_bounded_source(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, String> {
    let describe = |error| format!("cannot snapshot `{}`: {error}", path.display());
    let metadata = std::fs::metadata(path).map_err(describe)?;
    if !metadata.is_file() {
        return Err(format!(
            "source `{}` must be a regular file",
            path.display()
        ));
    }
    if metadata.len() > limit as u64 {
        return Ok(None);
    }
    let file = std::fs::File::open(path).map_err(describe)?;
    if !file.metadata().map_err(describe)?.is_file() {
        return Err(format!(
            "source `{}` must be a regular file",
            path.display()
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(describe)?;
    Ok((bytes.len() <= limit).then_some(bytes))
}

fn enqueue_source(
    path: PathBuf,
    pending: &mut BTreeSet<PathBuf>,
    captured: &BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    if !captured.contains_key(&path) {
        pending.insert(path);
        if pending.len() + captured.len() > MAX_SOURCE_FILES {
            return Err("too many source dependencies (maximum 1024 unique files)".into());
        }
    }
    Ok(())
}

fn parse_source(path: &Path, source: &str) -> Result<Flow, String> {
    if let Ok(flow) = lumo_dsl::parse_str(source) {
        lumo_dsl::validate(&flow).map_err(|e| e.to_string())?;
        Ok(flow)
    } else {
        lumo_skills::loader::load_skill_text(path, source)
            .map(|skill| skill.flow)
            .map_err(|e| format!("invalid dependency `{}`: {e}", path.display()))
    }
}

fn confined_path(base: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(format!(
            "dependency `{relative}` must stay inside its source directory"
        ));
    }
    let path = absolute_path(&base.join(path))?;
    let canonical_base = base.canonicalize().map_err(|e| e.to_string())?;
    if !path
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(canonical_base)
    {
        return Err(format!(
            "dependency `{relative}` escapes its source directory"
        ));
    }
    Ok(path)
}

fn collect_flows(
    base: &Path,
    directory: &Path,
    depth: usize,
    paths: &mut BTreeSet<PathBuf>,
    captured: &BTreeMap<PathBuf, String>,
) -> Result<(), String> {
    if depth > 16 {
        return Err("flow dependency directory nesting exceeds 16".into());
    }
    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            continue;
        }
        let path = entry.path();
        if ty.is_dir() {
            if matches!(
                entry.file_name().to_str(),
                Some(".git" | "node_modules" | "target" | "models" | "artifacts" | "skills")
            ) {
                continue;
            }
            collect_flows(base, &path, depth + 1, paths, captured)?;
        } else if ty.is_file()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".yaml") || n.ends_with(".yml"))
        {
            // Dynamic flow.call accepts ordinary .yaml files too. Freeze all
            // valid flow candidates, without promoting unrelated configs or
            // unfinished editor drafts into executable dependencies.
            let Some(bytes) = read_bounded_source(&path, MAX_SOURCE_BYTES)? else {
                continue;
            };
            let Ok(candidate) = String::from_utf8(bytes) else {
                continue;
            };
            if !lumo_dsl::parse_str(&candidate).is_ok_and(|flow| lumo_dsl::validate(&flow).is_ok())
            {
                continue;
            }
            enqueue_source(
                confined_path(
                    base,
                    &path
                        .strip_prefix(base)
                        .map_err(|e| e.to_string())?
                        .to_string_lossy(),
                )?,
                paths,
                captured,
            )?;
        }
    }
    Ok(())
}

fn collect_references(
    steps: &[Step],
    base: &Path,
    skills: &SkillRegistry,
    pending: &mut BTreeSet<PathBuf>,
    captured: &BTreeMap<PathBuf, String>,
    dynamic_flows_collected: &mut bool,
) -> Result<(), String> {
    for step in steps {
        let (key, is_flow) = match step.action.as_str() {
            "flow.call" => ("path", true),
            "skill.invoke" => ("name", false),
            _ => {
                for children in step.children() {
                    collect_references(
                        children,
                        base,
                        skills,
                        pending,
                        captured,
                        dynamic_flows_collected,
                    )?;
                }
                continue;
            }
        };
        if let Some(value) = step.with.get(key).and_then(serde_yaml::Value::as_str) {
            if !value.contains("{{") {
                if is_flow {
                    enqueue_source(confined_path(base, value)?, pending, captured)?;
                } else {
                    let skill = skills
                        .get(value)
                        .ok_or_else(|| format!("unknown dependency skill `{value}`"))?;
                    enqueue_source(absolute_path(&skill.source)?, pending, captured)?;
                }
            } else if is_flow {
                if !*dynamic_flows_collected {
                    collect_flows(base, base, 0, pending, captured)?;
                    *dynamic_flows_collected = true;
                }
            } else {
                for skill in skills.all() {
                    enqueue_source(absolute_path(&skill.source)?, pending, captured)?;
                }
            }
        }
        for children in step.children() {
            collect_references(
                children,
                base,
                skills,
                pending,
                captured,
                dynamic_flows_collected,
            )?;
        }
    }
    Ok(())
}

pub fn pin_plan_sources(
    plan: &mut AgentPlan,
    catalog: &CapabilityCatalog,
    skills: &SkillRegistry,
) -> Result<(), String> {
    let mut snapshots = BTreeMap::new();
    for node in &plan.nodes {
        if snapshots.contains_key(&node.capability_id) {
            continue;
        }
        let capability = catalog
            .get(&node.capability_id)
            .ok_or("unknown capability while freezing sources")?;
        let path = match &capability.source {
            CapabilitySource::Flow { path } => Path::new(path),
            CapabilitySource::Skill { source, .. } => Path::new(source),
            CapabilitySource::Mcp { .. } => continue,
        };
        let snapshot = capture_source_snapshot(path, skills)?;
        if !capability.source_content_matches(snapshot.source(path)?.as_bytes()) {
            return Err("source changed while freezing the plan; refresh and retry".into());
        }
        snapshots.insert(node.capability_id.clone(), snapshot);
    }
    if !snapshots.is_empty() {
        plan.metadata.insert(
            SOURCE_SNAPSHOTS_KEY.into(),
            serde_json::to_value(snapshots).map_err(|e| e.to_string())?,
        );
    }
    Ok(())
}
