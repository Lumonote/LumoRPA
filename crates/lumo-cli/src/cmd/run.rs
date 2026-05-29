use clap::Args as ClapArgs;
use colored::Colorize;
use lumo_ai::ProvidersConfig;
use lumo_core::{FlowVm, RunOptions};
use lumo_dsl::Step;
use lumo_storage::Repo;
use std::path::PathBuf;

use super::{build_action_registry, providers_path};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Flow YAML file
    pub flow: PathBuf,
    /// Inline input KEY=VALUE (repeatable)
    #[arg(short = 'i', long = "input", value_parser = parse_kv)]
    pub inputs: Vec<(String, String)>,
    /// Merge inputs from a JSON object string
    #[arg(long)]
    pub input_json: Option<String>,
    /// Merge inputs from a JSON file
    #[arg(long)]
    pub input_file: Option<PathBuf>,
    /// Don't persist run history
    #[arg(long)]
    pub no_store: bool,
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))?;
    Ok((k.to_string(), v.to_string()))
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    std::fs::create_dir_all(&home)?;
    let flow = lumo_dsl::parse_file(&args.flow)?;
    lumo_dsl::validate(&flow)?;

    let uses_ai = flow_uses_action(&flow.spec.steps, "ai.chat");
    let providers_cfg = ProvidersConfig::load(providers_path(&home)).unwrap_or_default();
    if uses_ai && providers_cfg.profiles.is_empty() {
        eprintln!(
            "  ! No provider profiles configured. \
             Run `lumo providers init` to seed defaults, \
             then `lumo providers list`."
        );
    }
    let registry = build_action_registry(&home, Some(&args.flow));

    let repo: Option<Repo> = if args.no_store {
        None
    } else {
        Some(Repo::open(home.join("lumo.db"))?)
    };

    let inputs = merge_cli_inputs(args.input_json, args.input_file, args.inputs)?;

    let opts = RunOptions {
        inputs: serde_json::Value::Object(inputs),
        trigger_kind: "manual".into(),
    };

    let vm = super::attach_ai_hooks(FlowVm::new(registry, repo), &home, &flow);
    let report = vm.run(&flow, opts).await?;

    println!();
    println!(
        "{} run={} state={} steps_ok={} executed={} declared={} skipped={} retried={} caught={} failed={} duration={}ms",
        "✓".green().bold(),
        report.run_id,
        if report.success {
            "ok".green()
        } else {
            "failed".red()
        },
        report.steps_ok,
        report.steps_executed,
        report.steps_total,
        report.steps_skipped,
        report.steps_retried,
        report.steps_caught,
        report.steps_failed,
        report.duration_ms,
    );
    if let Some(out) = report.outputs {
        if !out.is_null() {
            println!("outputs: {}", serde_json::to_string_pretty(&out)?);
        }
    }
    Ok(())
}

pub(crate) fn merge_cli_inputs(
    input_json: Option<String>,
    input_file: Option<PathBuf>,
    pairs: Vec<(String, String)>,
) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
    let mut out = serde_json::Map::new();
    if let Some(path) = input_file {
        let raw = std::fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("input file {} is not JSON: {e}", path.display()))?;
        merge_input_object(&mut out, value, "--input-file")?;
    }
    if let Some(raw) = input_json {
        let value: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("--input-json is not JSON: {e}"))?;
        merge_input_object(&mut out, value, "--input-json")?;
    }
    for (k, v) in pairs {
        out.insert(k, parse_input_scalar(&v));
    }
    Ok(out)
}

fn merge_input_object(
    out: &mut serde_json::Map<String, serde_json::Value>,
    value: serde_json::Value,
    source: &str,
) -> anyhow::Result<()> {
    let serde_json::Value::Object(map) = value else {
        anyhow::bail!("{source} must be a JSON object");
    };
    out.extend(map);
    Ok(())
}

fn parse_input_scalar(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).unwrap_or_else(|_| serde_json::Value::String(raw.to_string()))
}

fn flow_uses_action(steps: &[Step], action_id: &str) -> bool {
    steps.iter().any(|step| {
        step.action == action_id
            || step
                .children()
                .into_iter()
                .any(|children| flow_uses_action(children, action_id))
    })
}
