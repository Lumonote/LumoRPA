//! `lumo copilot` — NL → Flow generator (A-13).
//!
//! Thin CLI wrapper over `lumo_ai::copilot`: loads providers, picks a model,
//! delegates NL→Flow generation, then handles file output. The generation core
//! (prompt, extract, validate, retry) lives in `lumo_ai::copilot` so the desktop
//! Magic Prompt panel (F-18) shares the exact same logic.

use clap::Args as ClapArgs;
use lumo_ai::{AiRouter, ProvidersConfig};
use std::path::PathBuf;
use std::sync::Arc;

use super::providers_path;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Natural-language description of the flow to generate.
    pub prompt: String,
    /// Output YAML file. Defaults to `./flows/<slug>.lumoflow.yaml`.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Optional model override (e.g. `anthropic/claude-sonnet-4-6`). If absent,
    /// the active provider's default model is used.
    #[arg(long)]
    pub model: Option<String>,
    /// Print the generated YAML to stdout without writing a file.
    #[arg(long)]
    pub dry_run: bool,
}

pub async fn run(home: PathBuf, args: Args) -> anyhow::Result<()> {
    let providers_cfg = ProvidersConfig::load(providers_path(&home)).unwrap_or_default();
    let router = Arc::new(AiRouter::from_config(&providers_cfg));
    if router.provider_names().is_empty() {
        anyhow::bail!(
            "no LLM provider configured. Run `lumo providers add` first or set LUMO_PROVIDERS_PATH."
        );
    }
    let model = pick_model(&router, args.model.as_deref())?;

    let yaml = lumo_ai::copilot::generate_flow(&router, &model, &args.prompt, 2)
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    if args.dry_run {
        println!("{yaml}");
        return Ok(());
    }

    let out = args.out.unwrap_or_else(|| {
        let slug = lumo_ai::copilot::slug_from_yaml(&yaml).unwrap_or_else(|| "copilot".to_string());
        PathBuf::from("./flows").join(format!("{slug}.lumoflow.yaml"))
    });
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&out, &yaml)?;
    println!("✔ wrote {} ({} bytes)", out.display(), yaml.len());
    Ok(())
}

fn pick_model(router: &AiRouter, override_model: Option<&str>) -> anyhow::Result<String> {
    if let Some(m) = override_model {
        return Ok(m.to_string());
    }
    let active = router
        .active()
        .ok_or_else(|| anyhow::anyhow!("no active provider — run `lumo providers use <name>`"))?;
    // Format expected by router.chat: `<provider>/<model>`.
    Ok(format!("{active}/default"))
}
