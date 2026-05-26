use clap::Args as ClapArgs;
use colored::Colorize;
use lumo_ai::{ChatAction, ProvidersConfig, AiRouter};
use lumo_core::{ActionRegistry, FlowVm, RunOptions};
use lumo_skills::{register_skill_actions, SkillRegistry};
use lumo_storage::Repo;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Flow YAML file
    pub flow: PathBuf,
    /// Inline input KEY=VALUE (repeatable)
    #[arg(short = 'i', long = "input", value_parser = parse_kv)]
    pub inputs: Vec<(String, String)>,
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

    // ── Build AI router from ~/.lumorpa/providers.toml.
    let providers_cfg = ProvidersConfig::load(ProvidersConfig::default_path())
        .unwrap_or_default();
    if providers_cfg.profiles.is_empty() {
        eprintln!(
            "  ! No provider profiles configured. \
             Run `lumo providers init` to seed defaults, \
             then `lumo providers list`."
        );
    }
    let router = Arc::new(AiRouter::from_config(&providers_cfg));

    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);
    registry.register(ChatAction::new(router));

    // Auto-load installed skills so flows can call `skill.invoke`.
    let skill_reg = Arc::new(SkillRegistry::new());
    let _ = skill_reg.load_dir(SkillRegistry::default_root());
    register_skill_actions(&mut registry, skill_reg);

    let repo: Option<Repo> = if args.no_store {
        None
    } else {
        Some(Repo::open(home.join("lumo.db"))?)
    };

    let inputs: serde_json::Map<String, serde_json::Value> = args
        .inputs
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();

    let opts = RunOptions {
        inputs: serde_json::Value::Object(inputs),
        trigger_kind: "manual".into(),
    };

    let vm = FlowVm::new(registry, repo);
    let report = vm.run(&flow, opts).await?;

    println!();
    println!(
        "{} run={} state={} steps={}/{} duration={}ms",
        "✓".green().bold(),
        report.run_id,
        if report.success { "ok".green() } else { "failed".red() },
        report.steps_ok,
        report.steps_total,
        report.duration_ms,
    );
    if let Some(out) = report.outputs {
        if !out.is_null() {
            println!("outputs: {}", serde_json::to_string_pretty(&out)?);
        }
    }
    Ok(())
}
