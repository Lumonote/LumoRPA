//! `lumo` CLI entry. See `lumo --help`.

mod cmd;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "lumo",
    version,
    about = "LumoRPA - open-source, AI-native RPA platform",
    long_about = None,
)]
struct Cli {
    /// Path to lumo data directory (default: ~/.lumorpa)
    #[arg(long, env = "LUMO_HOME", global = true)]
    home: Option<PathBuf>,

    /// Increase verbosity (-v, -vv, -vvv)
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Initialize a new flow project in the given directory
    Init(cmd::init::Args),
    /// Validate a flow YAML file
    Validate(cmd::validate::Args),
    /// Run a flow file once
    Run(cmd::run::Args),
    /// Inspect previous runs
    Runs(cmd::runs::Args),
    /// Show available actions
    Actions(cmd::actions::Args),
    /// Manage LLM provider profiles (cc-switch style)
    Providers(cmd::providers::Args),
    /// Manage reusable Skills (Claude-Code-style SKILL.md)
    Skills(cmd::skills::Args),
    /// Start a webhook HTTP server that dispatches POSTs to flows
    Serve(cmd::serve::Args),
    /// Run as a Model Context Protocol (MCP) server over stdio
    Mcp(cmd::mcp::Args),
    /// Generate a flow YAML draft from a natural-language prompt
    Copilot(cmd::copilot::Args),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let home = cli
        .home
        .clone()
        .or_else(dirs_home)
        .unwrap_or_else(|| PathBuf::from(".lumorpa"));

    match cli.cmd {
        Cmd::Init(a) => cmd::init::run(a).await,
        Cmd::Validate(a) => cmd::validate::run(home, a).await,
        Cmd::Run(a) => cmd::run::run(home, a).await,
        Cmd::Runs(a) => cmd::runs::run(home, a).await,
        Cmd::Actions(a) => cmd::actions::run(home, a).await,
        Cmd::Providers(a) => cmd::providers::run(home, a).await,
        Cmd::Skills(a) => cmd::skills::run(home, a).await,
        Cmd::Serve(a) => cmd::serve::run(home, a).await,
        Cmd::Mcp(a) => cmd::mcp::run(home, a).await,
        Cmd::Copilot(a) => cmd::copilot::run(home, a).await,
    }
}

fn init_tracing(verbose: u8) {
    use tracing_subscriber::{fmt, EnvFilter};
    let level = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("lumo_={level},warn")));
    let _ = fmt().with_env_filter(filter).with_target(false).try_init();
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| PathBuf::from(h).join(".lumorpa"))
}
