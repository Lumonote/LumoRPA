//! `lumo lint` — best-practice + capability + variable reference checks.
//!
//! Catches the long tail of "works at parse-time but bites you at runtime":
//! references to undeclared inputs, action ids not in the registry, network /
//! fs / llm capability omissions, dead retry policies, etc.

use clap::{Args as ClapArgs, ValueEnum};
use colored::Colorize;
use lumo_dsl::{lint_flow, LintSeverity};
use std::path::PathBuf;

use super::build_action_registry;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Flow YAML file to lint
    pub flow: PathBuf,
    /// Output format
    #[arg(long, value_enum, default_value_t = Format::Pretty)]
    pub format: Format,
    /// Exit non-zero when at least one warning is present (default: only errors)
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Format {
    Pretty,
    Json,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(&args.flow)?;
    let registry = build_action_registry(&home, Some(&args.flow));
    let known: Vec<String> = registry.iter_ids().collect();
    let known_refs: Vec<&str> = known.iter().map(String::as_str).collect();
    let issues = lint_flow(&flow, &known_refs);

    match args.format {
        Format::Json => println!("{}", serde_json::to_string_pretty(&issues)?),
        Format::Pretty => print_pretty(&issues),
    }

    let has_error = issues
        .iter()
        .any(|i| matches!(i.severity, LintSeverity::Error));
    let has_warn = issues
        .iter()
        .any(|i| matches!(i.severity, LintSeverity::Warn));
    if has_error || (args.strict && has_warn) {
        std::process::exit(1);
    }
    Ok(())
}

fn print_pretty(issues: &[lumo_dsl::LintIssue]) {
    if issues.is_empty() {
        println!("{}", "lint: clean".green());
        return;
    }
    let errs = issues
        .iter()
        .filter(|i| matches!(i.severity, LintSeverity::Error))
        .count();
    let warns = issues
        .iter()
        .filter(|i| matches!(i.severity, LintSeverity::Warn))
        .count();
    let infos = issues
        .iter()
        .filter(|i| matches!(i.severity, LintSeverity::Info))
        .count();

    for i in issues {
        let tag = match i.severity {
            LintSeverity::Error => "ERROR".red().bold(),
            LintSeverity::Warn => "WARN ".yellow().bold(),
            LintSeverity::Info => "INFO ".cyan().bold(),
        };
        let step = i
            .step
            .as_deref()
            .map(|s| format!(" step=`{s}`"))
            .unwrap_or_default();
        println!("{tag}  [{}]{step}  {}", i.code, i.message);
    }
    println!();
    println!(
        "lint: {} error{} · {} warning{} · {} info",
        errs,
        if errs == 1 { "" } else { "s" },
        warns,
        if warns == 1 { "" } else { "s" },
        infos,
    );
}
