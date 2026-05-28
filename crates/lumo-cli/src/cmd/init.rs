use clap::Args as ClapArgs;
use std::path::PathBuf;

#[derive(Debug, ClapArgs)]
pub struct Args {
    /// Project directory to create
    pub path: PathBuf,
    /// Project name
    #[arg(long)]
    pub name: Option<String>,
}

pub async fn run(args: Args) -> anyhow::Result<()> {
    let dir = &args.path;
    if dir.exists() && std::fs::read_dir(dir)?.next().is_some() {
        anyhow::bail!("directory {} is not empty", dir.display());
    }
    std::fs::create_dir_all(dir.join("flows"))?;
    std::fs::create_dir_all(dir.join("skills"))?;
    std::fs::create_dir_all(dir.join("templates"))?;
    let name = args
        .name
        .clone()
        .or_else(|| dir.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-flow".to_string());

    let hello = include_str!("../../../../examples/hello-world.lumoflow.yaml");
    std::fs::write(
        dir.join("flows/hello.lumoflow.yaml"),
        hello.replace("hello-world", &name),
    )?;

    std::fs::write(
        dir.join("README.md"),
        format!(
            "# {name}\n\nLumoRPA project.\n\n## Run\n\n```bash\nlumo validate flows/hello.lumoflow.yaml\nlumo run flows/hello.lumoflow.yaml\nlumo run flows/hello.lumoflow.yaml --input who=world\n```\n\n## Local State\n\nRun history is stored under `$LUMO_HOME` or `~/.lumorpa` by default. Use `--home .lumorpa` to keep state local to this project:\n\n```bash\nlumo --home .lumorpa run flows/hello.lumoflow.yaml\nlumo --home .lumorpa runs list\n```\n\n## AI Providers\n\nFlows that use `ai.chat` need provider config and explicit network opt-in:\n\n```bash\nlumo --home .lumorpa providers init\nLUMO_ALLOW_LLM_NETWORK=1 lumo --home .lumorpa run flows/my-ai-flow.lumoflow.yaml\n```\n"
        ),
    )?;
    std::fs::write(
        dir.join(".gitignore"),
        ".lumorpa/\nout/\n*.log\n.DS_Store\n",
    )?;
    std::fs::write(dir.join("skills/.gitkeep"), "")?;
    println!("Initialized LumoRPA project at {}", dir.display());
    Ok(())
}
