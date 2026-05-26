use clap::{Args as ClapArgs, Subcommand};
use comfy_table::{presets::UTF8_FULL, Cell, Color, Table};
use lumo_storage::Repo;
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    sub: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// List recent runs
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show one run in detail
    Show { run_id: String },
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let repo = Repo::open(home.join("lumo.db"))?;
    match args.sub {
        Sub::List { limit } => list(&repo, limit),
        Sub::Show { run_id } => show(&repo, &run_id),
    }
}

fn list(repo: &Repo, limit: u32) -> anyhow::Result<()> {
    let runs = repo.list_runs(limit)?;
    let mut t = Table::new();
    t.load_preset(UTF8_FULL).set_header(vec![
        "run_id", "flow", "state", "trigger", "started", "duration",
    ]);
    for r in runs {
        let state_cell = match r.state.as_str() {
            "ok" => Cell::new(&r.state).fg(Color::Green),
            "failed" => Cell::new(&r.state).fg(Color::Red),
            _ => Cell::new(&r.state).fg(Color::Yellow),
        };
        t.add_row(vec![
            Cell::new(&r.id),
            Cell::new(&r.flow_id),
            state_cell,
            Cell::new(&r.trigger_kind),
            Cell::new(r.started_at.map(|t| t.to_rfc3339()).unwrap_or_default()),
            Cell::new(
                r.finished_at
                    .and_then(|f| r.started_at.map(|s| (f - s).num_milliseconds().to_string() + "ms"))
                    .unwrap_or_default(),
            ),
        ]);
    }
    println!("{t}");
    Ok(())
}

fn show(repo: &Repo, run_id: &str) -> anyhow::Result<()> {
    let r = repo.get_run(run_id)?.ok_or_else(|| anyhow::anyhow!("run not found"))?;
    println!("run_id: {}", r.id);
    println!("flow:   {} @ {}", r.flow_id, r.flow_version);
    println!("state:  {}", r.state);
    println!("trigger: {}", r.trigger_kind);

    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .set_header(vec!["idx", "step_id", "state", "attempt", "ms", "error"]);
    for s in repo.list_steps(run_id)? {
        let dur = s
            .finished_at
            .and_then(|f| s.started_at.map(|st| (f - st).num_milliseconds()))
            .unwrap_or(0);
        t.add_row(vec![
            Cell::new(s.idx),
            Cell::new(&s.step_id),
            Cell::new(&s.state),
            Cell::new(s.attempt),
            Cell::new(dur),
            Cell::new(s.error.unwrap_or_default()),
        ]);
    }
    println!("{t}");
    Ok(())
}
