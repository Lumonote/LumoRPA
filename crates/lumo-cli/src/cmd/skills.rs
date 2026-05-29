//! `lumo skills` subcommand — manage and run reusable Skills.

use clap::{Args as ClapArgs, Subcommand};
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL, Cell, Table};
use lumo_core::{FlowVm, RunOptions};
use lumo_skills::{loader::load_skill_file, register_skill_actions, SkillRegistry};
use std::path::PathBuf;
use std::sync::Arc;

use super::{build_action_registry, run::merge_cli_inputs, skills_root};

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    sub: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// List installed skills
    List,
    /// Show one skill's frontmatter + flow
    Show { name: String },
    /// Install a skill from a local SKILL.md (copied into ~/.lumorpa/skills/<name>/)
    Install { source: PathBuf },
    /// Remove an installed skill by name
    Remove { name: String },
    /// Print the resolved skills directory
    Path,
    /// Run a skill end-to-end (its flow will be executed once)
    Run {
        name: String,
        #[arg(short = 'i', long = "input", value_parser = parse_kv)]
        inputs: Vec<(String, String)>,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))?;
    Ok((k.into(), v.into()))
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let root = skills_root(&home);

    match args.sub {
        Sub::Path => {
            println!("{}", root.display());
            Ok(())
        }

        Sub::List => {
            let reg = SkillRegistry::new();
            let n = reg.load_dir(&root).unwrap_or(0);
            if n == 0 {
                println!("(no skills installed at {})", root.display());
                return Ok(());
            }
            let mut t = Table::new();
            t.load_preset(UTF8_FULL)
                .set_header(vec!["name", "description", "triggers", "steps"]);
            for s in reg.all() {
                let fm = &s.frontmatter;
                t.add_row(vec![
                    Cell::new(&fm.name),
                    Cell::new(fm.description.clone().unwrap_or_default()),
                    Cell::new(fm.triggers.join(", ")),
                    Cell::new(s.flow.spec.steps.len()),
                ]);
            }
            println!("root: {}", root.display());
            println!("{t}");
            Ok(())
        }

        Sub::Show { name } => {
            let reg = SkillRegistry::new();
            let _ = reg.load_dir(&root);
            let s = reg
                .get(&name)
                .ok_or_else(|| anyhow::anyhow!("not found: {name}"))?;
            println!("# {} @ {}", s.name(), s.source.display());
            if let Some(d) = s.description() {
                println!("\n{d}");
            }
            println!("\n--- flow ---");
            let yaml = serde_yaml::to_string(&s.flow).unwrap_or_default();
            println!("{yaml}");
            Ok(())
        }

        Sub::Install { source } => {
            let skill = load_skill_file(&source)?;
            let dest_dir = root.join(skill.name());
            std::fs::create_dir_all(&dest_dir)?;
            let dest = dest_dir.join("SKILL.md");
            std::fs::copy(&source, &dest)?;
            println!(
                "{} installed `{}` → {}",
                "✓".green().bold(),
                skill.name(),
                dest.display()
            );
            Ok(())
        }

        Sub::Remove { name } => {
            let dir = root.join(&name);
            if !dir.exists() {
                anyhow::bail!("not found: {name}");
            }
            std::fs::remove_dir_all(&dir)?;
            println!("removed: {name}");
            Ok(())
        }

        Sub::Run { name, inputs } => {
            let skill_reg = Arc::new(SkillRegistry::new());
            skill_reg
                .load_dir(&root)
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let s = skill_reg
                .get(&name)
                .ok_or_else(|| anyhow::anyhow!("not found: {name}"))?;

            let mut action_reg = build_action_registry(&home, None);
            register_skill_actions(&mut action_reg, skill_reg.clone());
            let inputs_json = merge_cli_inputs(None, None, inputs)?;

            let vm = super::attach_ai_hooks(FlowVm::new(action_reg, None), &home, &s.flow);
            let report = vm
                .run(
                    &s.flow,
                    RunOptions {
                        inputs: serde_json::Value::Object(inputs_json),
                        trigger_kind: "skill-cli".into(),
                    },
                )
                .await?;

            println!(
                "{} skill={} state={} steps_ok={} executed={} declared={} skipped={} retried={} caught={} failed={} duration={}ms",
                "✓".green().bold(),
                name,
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
    }
}
