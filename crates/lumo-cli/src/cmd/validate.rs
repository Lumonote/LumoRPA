use clap::Args as ClapArgs;
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Flow YAML file
    pub flow: PathBuf,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(&args.flow)?;
    lumo_dsl::validate(&flow)?;
    println!(
        "OK  flow id={} version={} steps={}",
        flow.metadata.id,
        flow.metadata.version,
        flow.spec.steps.len()
    );
    Ok(())
}
