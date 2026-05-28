use clap::Args as ClapArgs;
use comfy_table::{presets::UTF8_FULL, Table};
use std::path::PathBuf;

use super::build_action_registry;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Show schema JSON for one action id
    #[arg(long)]
    pub show: Option<String>,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let registry = build_action_registry(&home, None);

    if let Some(id) = args.show {
        let action = registry
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("action `{id}` not found"))?;
        println!("{}", serde_json::to_string_pretty(action.schema())?);
        return Ok(());
    }

    let mut t = Table::new();
    t.load_preset(UTF8_FULL).set_header(vec!["id", "summary"]);
    let mut ids: Vec<_> = registry.iter_ids().collect();
    ids.sort();
    for id in ids {
        let a = registry.get(&id).unwrap();
        t.add_row(vec![id, a.summary().to_string()]);
    }
    println!("{t}");
    Ok(())
}
