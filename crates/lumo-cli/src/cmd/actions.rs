use clap::Args as ClapArgs;
use comfy_table::{presets::UTF8_FULL, Table};
use lumo_core::ActionRegistry;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Show schema JSON for one action id
    #[arg(long)]
    pub show: Option<String>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let mut registry = ActionRegistry::new();
    lumo_actions::register_all(&mut registry);

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
