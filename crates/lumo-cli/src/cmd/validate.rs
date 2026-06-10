use clap::Args as ClapArgs;
use std::path::PathBuf;

use super::{build_action_registry, load_skill_registry};

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Flow YAML file
    pub flow: PathBuf,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let flow = lumo_dsl::parse_file(&args.flow)?;
    lumo_dsl::validate(&flow)?;
    let registry = build_action_registry(&home, Some(&args.flow));
    let skills = load_skill_registry(&home, Some(&args.flow));
    // P1-7:步级校验(action 存在性 + schema + capability 声明 + skill 引用)
    // 上移到 lumo-core 的单一实现,desktop 与 CLI 共用,行为不再漂移。
    lumo_core::validate_steps(
        &flow.spec.steps,
        &flow.spec.capabilities,
        &registry,
        &|name| skills.get(name).is_some(),
    )?;
    println!(
        "OK  flow id={} version={} steps={}",
        flow.metadata.id,
        flow.metadata.version,
        flow.spec.steps.len()
    );
    Ok(())
}
