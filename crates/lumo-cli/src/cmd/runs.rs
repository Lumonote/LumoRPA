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
    Show {
        run_id: String,
        /// Print persisted step outputs below the table
        #[arg(long)]
        outputs: bool,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
    /// Show AI token / USD cost for one run, or aggregated for all
    Cost {
        /// Specific run id; omit to aggregate across all runs
        run_id: Option<String>,
        /// Emit machine-readable JSON
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let repo = Repo::open(home.join("lumo.db"))?;
    match args.sub {
        Sub::List { limit } => list(&repo, limit),
        Sub::Show {
            run_id,
            outputs,
            json,
        } => show(&repo, &run_id, outputs, json),
        Sub::Cost { run_id, json } => cost(&repo, run_id.as_deref(), json),
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
                    .and_then(|f| {
                        r.started_at
                            .map(|s| (f - s).num_milliseconds().to_string() + "ms")
                    })
                    .unwrap_or_default(),
            ),
        ]);
    }
    println!("{t}");
    Ok(())
}

fn show(repo: &Repo, run_id: &str, outputs: bool, json: bool) -> anyhow::Result<()> {
    let r = repo
        .get_run(run_id)?
        .ok_or_else(|| anyhow::anyhow!("run not found"))?;
    let steps = repo.list_steps(run_id)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "run": r,
                "steps": steps,
            }))?
        );
        return Ok(());
    }
    println!("run_id: {}", r.id);
    println!("flow:   {} @ {}", r.flow_id, r.flow_version);
    println!("state:  {}", r.state);
    println!("trigger: {}", r.trigger_kind);
    println!(
        "inputs: {}",
        serde_json::to_string(&r.inputs).unwrap_or_else(|_| "{}".into())
    );
    if let Some(outputs) = &r.outputs {
        println!(
            "outputs: {}",
            serde_json::to_string(outputs).unwrap_or_else(|_| "null".into())
        );
    }

    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .set_header(vec!["seq", "path", "state", "attempt", "ms", "error"]);
    for s in &steps {
        let dur = s
            .finished_at
            .and_then(|f| s.started_at.map(|st| (f - st).num_milliseconds()))
            .unwrap_or(0);
        let path = format!("{}{}", "  ".repeat(s.depth.max(0) as usize), s.path);
        t.add_row(vec![
            Cell::new(s.seq),
            Cell::new(path),
            Cell::new(&s.state),
            Cell::new(s.attempt),
            Cell::new(dur),
            Cell::new(s.error.clone().unwrap_or_default()),
        ]);
    }
    println!("{t}");
    if outputs {
        for s in &steps {
            if let Some(output) = &s.output_json {
                println!();
                println!("{}:", s.path);
                println!("{}", serde_json::to_string_pretty(output)?);
            }
        }
    }
    Ok(())
}

fn cost(repo: &Repo, run_id: Option<&str>, json: bool) -> anyhow::Result<()> {
    match run_id {
        Some(id) => cost_for_run(repo, id, json),
        None => cost_summary(repo, json),
    }
}

fn cost_for_run(repo: &Repo, run_id: &str, json: bool) -> anyhow::Result<()> {
    let calls = repo.list_ai_calls(run_id)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&calls)?);
        return Ok(());
    }
    if calls.is_empty() {
        println!("(no AI calls recorded for run {run_id})");
        return Ok(());
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL).set_header(vec![
        "#", "step", "provider", "model", "in", "out", "ms", "USD",
    ]);
    let mut total_in: i64 = 0;
    let mut total_out: i64 = 0;
    let mut total_micro: i64 = 0;
    for (i, c) in calls.iter().enumerate() {
        total_in += c.input_tokens;
        total_out += c.output_tokens;
        total_micro += c.cost_usd_micro;
        t.add_row(vec![
            Cell::new(i + 1),
            Cell::new(c.step_id.clone().unwrap_or_default()),
            Cell::new(&c.provider),
            Cell::new(&c.model),
            Cell::new(c.input_tokens),
            Cell::new(c.output_tokens),
            Cell::new(c.latency_ms),
            Cell::new(usd(c.cost_usd_micro)),
        ]);
    }
    println!("{t}");
    println!(
        "total: in={total_in} · out={total_out} · {} tokens · {}",
        total_in + total_out,
        usd(total_micro),
    );
    Ok(())
}

fn cost_summary(repo: &Repo, json: bool) -> anyhow::Result<()> {
    let runs = repo.list_runs(200)?;
    let mut rows = Vec::new();
    let mut grand_tokens: i64 = 0;
    let mut grand_micro: i64 = 0;
    for r in &runs {
        if r.cost_token == 0 && r.cost_usd_micro == 0 {
            continue;
        }
        grand_tokens += r.cost_token;
        grand_micro += r.cost_usd_micro;
        rows.push((r.id.clone(), r.flow_id.clone(), r.cost_token, r.cost_usd_micro));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "runs": rows.iter().map(|(id, flow, tok, usd)| serde_json::json!({
                    "run_id": id, "flow_id": flow, "tokens": tok, "usd_micro": usd,
                })).collect::<Vec<_>>(),
                "total_tokens": grand_tokens,
                "total_usd_micro": grand_micro,
            }))?
        );
        return Ok(());
    }
    if rows.is_empty() {
        println!("(no costed runs)");
        return Ok(());
    }
    let mut t = Table::new();
    t.load_preset(UTF8_FULL)
        .set_header(vec!["run_id", "flow", "tokens", "USD"]);
    for (id, flow, tok, micro) in &rows {
        t.add_row(vec![
            Cell::new(id),
            Cell::new(flow),
            Cell::new(tok),
            Cell::new(usd(*micro)),
        ]);
    }
    println!("{t}");
    println!(
        "{} runs · {} tokens · {}",
        rows.len(),
        grand_tokens,
        usd(grand_micro),
    );
    Ok(())
}

fn usd(micro: i64) -> String {
    let dollars = micro as f64 / 1_000_000.0;
    format!("${dollars:.4}")
}
