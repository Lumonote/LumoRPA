//! `lumo providers` subcommand — cc-switch style provider profile manager.

use clap::{Args as ClapArgs, Subcommand};
use colored::Colorize;
use comfy_table::{presets::UTF8_FULL, Cell, Color, Table};
use lumo_ai::{
    config::{ProviderProfile, ProvidersConfig},
    provider::{ChatMessage, ChatRequest, Role},
    AiRouter,
};
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    sub: Sub,
}

#[derive(Debug, Subcommand)]
enum Sub {
    /// List configured provider profiles
    List,
    /// Show one profile's details (API key redacted)
    Show { name: String },
    /// Switch the active profile (used when a flow doesn't pin a model)
    Use { name: String },
    /// Add or update a profile (full upsert)
    Add {
        name: String,
        /// "openai" | "anthropic"
        #[arg(long, default_value = "openai")]
        kind: String,
        /// For kind=openai: "chat" | "responses" (default: chat)
        #[arg(long)]
        wire_api: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long)]
        default_model: Option<String>,
        /// Reasoning effort hint (passed to Responses API only)
        #[arg(long)]
        reasoning_effort: Option<String>,
        /// Extra header KEY=VALUE (repeatable)
        #[arg(long = "header", value_parser = parse_kv)]
        headers: Vec<(String, String)>,
        /// Mark as active immediately
        #[arg(long)]
        activate: bool,
    },
    /// Update one or more fields of an existing profile
    Set {
        name: String,
        #[arg(long)]
        wire_api: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long)]
        default_model: Option<String>,
        #[arg(long)]
        reasoning_effort: Option<String>,
        /// Extra header KEY=VALUE (repeatable, merged into existing)
        #[arg(long = "header", value_parser = parse_kv)]
        headers: Vec<(String, String)>,
        /// Clear a single header by name
        #[arg(long = "unset-header")]
        unset_headers: Vec<String>,
    },
    /// Remove a profile
    Remove { name: String },
    /// Reset to the seeded default config (openai + anthropic + deepseek + ollama)
    Init {
        #[arg(long)]
        force: bool,
    },
    /// Print resolved config file path
    Path,
    /// Send a tiny "ping" prompt to test the profile end-to-end
    /// (requires LUMO_ALLOW_LLM_NETWORK=1)
    Test {
        name: String,
        #[arg(long, default_value = "Reply with one word: pong")]
        prompt: String,
    },
}

fn parse_kv(s: &str) -> Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE, got `{s}`"))?;
    Ok((k.to_string(), v.to_string()))
}

pub async fn run(_home: PathBuf, args: Args) -> anyhow::Result<()> {
    let path = ProvidersConfig::default_path();

    match args.sub {
        Sub::Path => { println!("{}", path.display()); Ok(()) }

        Sub::Init { force } => {
            if path.exists() && !force {
                anyhow::bail!(
                    "{} already exists. Use --force to overwrite.",
                    path.display()
                );
            }
            let cfg = ProvidersConfig::seed_default();
            cfg.save(&path)?;
            println!(
                "{} initialized provider config at {}",
                "✓".green().bold(),
                path.display()
            );
            println!("  active = {}", cfg.active.unwrap_or_default());
            Ok(())
        }

        Sub::List => {
            let cfg = ProvidersConfig::load(&path)?;
            if cfg.profiles.is_empty() {
                println!("(no providers configured — run `lumo providers init`)");
                return Ok(());
            }
            let mut t = Table::new();
            t.load_preset(UTF8_FULL).set_header(vec![
                "active", "name", "kind", "base_url", "key", "default_model",
            ]);
            for p in &cfg.profiles {
                let is_active = cfg.active.as_deref() == Some(p.name.as_str());
                let active_cell = if is_active {
                    Cell::new("●").fg(Color::Green)
                } else {
                    Cell::new("")
                };
                let key_cell = if p.api_key.is_some() {
                    Cell::new("inline")
                } else if let Some(env) = &p.api_key_env {
                    let resolved = std::env::var(env).is_ok();
                    Cell::new(format!(
                        "{} ({})",
                        env,
                        if resolved { "set".to_string() } else { "missing".to_string() }
                    ))
                    .fg(if resolved { Color::Green } else { Color::Yellow })
                } else {
                    Cell::new("-")
                };
                t.add_row(vec![
                    active_cell,
                    Cell::new(&p.name),
                    Cell::new(&p.kind),
                    Cell::new(p.base_url.as_deref().unwrap_or("-")),
                    key_cell,
                    Cell::new(p.default_model.as_deref().unwrap_or("-")),
                ]);
            }
            println!("config: {}", path.display());
            println!("{t}");
            Ok(())
        }

        Sub::Show { name } => {
            let cfg = ProvidersConfig::load(&path)?;
            let p = cfg.get(&name).ok_or_else(|| anyhow::anyhow!("not found: {name}"))?;
            let red = p.redacted();
            println!("{}", toml::to_string_pretty(&red)?);
            Ok(())
        }

        Sub::Use { name } => {
            let mut cfg = ProvidersConfig::load(&path)?;
            cfg.use_(&name)?;
            cfg.save(&path)?;
            println!("active = {}", name.green().bold());
            Ok(())
        }

        Sub::Add { name, kind, wire_api, base_url, api_key, api_key_env, default_model, reasoning_effort, headers, activate } => {
            let mut cfg = ProvidersConfig::load(&path)?;
            let mut hmap = std::collections::BTreeMap::new();
            for (k, v) in headers { hmap.insert(k, v); }
            let p = ProviderProfile {
                name: name.clone(),
                kind,
                wire_api,
                base_url,
                api_key,
                api_key_env,
                default_model,
                models: vec![],
                headers: hmap,
                reasoning_effort,
                notes: None,
            };
            cfg.upsert(p);
            if activate || cfg.active.is_none() {
                let _ = cfg.use_(&name);
            }
            cfg.save(&path)?;
            println!("{} {} saved", "✓".green().bold(), name);
            Ok(())
        }

        Sub::Set { name, wire_api, base_url, api_key, api_key_env, default_model, reasoning_effort, headers, unset_headers } => {
            let mut cfg = ProvidersConfig::load(&path)?;
            {
                let prof = cfg.profiles.iter_mut().find(|p| p.name == name)
                    .ok_or_else(|| anyhow::anyhow!("not found: {name}"))?;
                if let Some(v) = wire_api          { prof.wire_api          = Some(v); }
                if let Some(v) = base_url          { prof.base_url          = Some(v); }
                if let Some(v) = api_key           { prof.api_key           = Some(v); }
                if let Some(v) = api_key_env       { prof.api_key_env       = Some(v); }
                if let Some(v) = default_model     { prof.default_model     = Some(v); }
                if let Some(v) = reasoning_effort  { prof.reasoning_effort  = Some(v); }
                for (k, v) in headers              { prof.headers.insert(k, v); }
                for k in unset_headers             { prof.headers.remove(&k); }
            }
            cfg.save(&path)?;
            println!("{} {} updated", "✓".green().bold(), name);
            Ok(())
        }

        Sub::Remove { name } => {
            let mut cfg = ProvidersConfig::load(&path)?;
            cfg.remove(&name)?;
            cfg.save(&path)?;
            println!("removed: {name}");
            Ok(())
        }

        Sub::Test { name, prompt } => {
            let cfg = ProvidersConfig::load(&path)?;
            let prof = cfg.get(&name).ok_or_else(|| anyhow::anyhow!("not found: {name}"))?;
            let model = prof.default_model.clone()
                .ok_or_else(|| anyhow::anyhow!("profile `{name}` has no default_model"))?;
            let one_off = ProvidersConfig {
                active: Some(name.clone()),
                profiles: vec![prof.clone()],
            };
            let router = AiRouter::from_config(&one_off);
            let req = ChatRequest {
                model: format!("{name}/{model}"),
                messages: vec![ChatMessage { role: Role::User, content: prompt }],
                temperature: Some(0.0),
                max_tokens: Some(64),
                system: None,
            };
            match router.chat(req).await {
                Ok(r) => {
                    println!("{} model={} provider={} tokens={}↑/{}↓",
                        "✓".green().bold(), r.model, r.provider,
                        r.input_tokens, r.output_tokens);
                    println!("---");
                    println!("{}", r.content);
                    Ok(())
                }
                Err(e) => {
                    eprintln!("{} test failed: {e}", "✗".red().bold());
                    Err(anyhow::anyhow!("provider test failed"))
                }
            }
        }
    }
}
